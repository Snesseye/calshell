//! OAuth 2.0 for installed apps: PKCE + loopback redirect.
//!
//! The refresh token lives in Windows Credential Manager (service `calshell`,
//! account `google-refresh-token`). Access tokens are only ever held in memory.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use url::Url;

use crate::config::Config;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const SCOPES: &str =
    "https://www.googleapis.com/auth/calendar.events https://www.googleapis.com/auth/calendar.readonly";
const KEYRING_SERVICE: &str = "calshell";
const KEYRING_USER: &str = "google-refresh-token";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Google Calendar is not connected. Connect it in Settings.")]
    NotConnected,
    #[error("No Google OAuth client ID is set. Add one in Settings.")]
    NoClient,
    #[error("A sign-in is already in progress.")]
    InProgress,
    #[error("Sign-in timed out.")]
    TimedOut,
    #[error("Google rejected the sign-in: {0}")]
    Rejected(String),
    #[error("Network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Credential Manager error: {0}")]
    Keyring(#[from] keyring::Error),
    #[error("Could not open the browser: {0}")]
    Browser(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct ClientCredentials {
    pub id: String,
    pub secret: Option<String>,
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(String::from)
}

/// Client ID compiled in from `GOOGLE_CLIENT_ID` (env or `src-tauri/.env`).
pub fn builtin_client() -> Option<ClientCredentials> {
    Some(ClientCredentials {
        id: non_empty(option_env!("GOOGLE_CLIENT_ID"))?,
        secret: non_empty(option_env!("GOOGLE_CLIENT_SECRET")),
    })
}

/// A client set in config.json wins over the compiled-in one.
pub fn credentials(config: &Config) -> Option<ClientCredentials> {
    match non_empty(config.google_client_id.as_deref()) {
        Some(id) => Some(ClientCredentials {
            id,
            secret: non_empty(config.google_client_secret.as_deref()),
        }),
        None => builtin_client(),
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    error_description: Option<String>,
}

impl TokenError {
    fn message(&self) -> String {
        match &self.error_description {
            Some(desc) => format!("{} ({desc})", self.error),
            None => self.error.clone(),
        }
    }
}

struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

pub struct Auth {
    http: reqwest::Client,
    cached: Mutex<Option<CachedToken>>,
    sign_in: Mutex<()>,
    connected: AtomicBool,
}

impl Auth {
    pub fn new(http: reqwest::Client) -> Self {
        let connected = match read_refresh_token() {
            Ok(token) => token.is_some(),
            Err(err) => {
                log::warn!("could not read Credential Manager: {err}");
                false
            }
        };
        Self {
            http,
            cached: Mutex::new(None),
            sign_in: Mutex::new(()),
            connected: AtomicBool::new(connected),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Forgets the in-memory access token, e.g. after a 401.
    pub async fn invalidate(&self) {
        *self.cached.lock().await = None;
    }

    /// Returns a valid access token, refreshing it silently when needed.
    pub async fn access_token(&self, client: &ClientCredentials) -> Result<String, AuthError> {
        let mut cached = self.cached.lock().await;
        if let Some(token) = cached.as_ref() {
            if token.expires_at > Instant::now() + Duration::from_secs(60) {
                return Ok(token.access_token.clone());
            }
        }

        let refresh_token = read_refresh_token()?.ok_or(AuthError::NotConnected)?;
        let mut form = vec![
            ("client_id", client.id.as_str()),
            ("refresh_token", refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];
        if let Some(secret) = &client.secret {
            form.push(("client_secret", secret));
        }

        let response = self.http.post(TOKEN_URL).form(&form).send().await?;
        if !response.status().is_success() {
            let err: TokenError = response.json().await?;
            if err.error == "invalid_grant" {
                // Revoked, expired (Testing mode) or password changed: start over.
                log::warn!("refresh token rejected: {}", err.message());
                delete_refresh_token()?;
                self.connected.store(false, Ordering::Relaxed);
                return Err(AuthError::NotConnected);
            }
            return Err(AuthError::Rejected(err.message()));
        }

        let token: TokenResponse = response.json().await?;
        *cached = Some(CachedToken {
            access_token: token.access_token.clone(),
            expires_at: Instant::now() + Duration::from_secs(token.expires_in),
        });
        Ok(token.access_token)
    }

    /// Runs the full browser sign-in and stores the refresh token.
    pub async fn connect(&self, app: &AppHandle, client: &ClientCredentials) -> Result<(), AuthError> {
        let _guard = self.sign_in.try_lock().map_err(|_| AuthError::InProgress)?;

        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let redirect_uri = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        let verifier = random_token(32);
        let state = random_token(16);

        let mut auth_url = Url::parse(AUTH_URL).expect("valid auth url");
        auth_url
            .query_pairs_mut()
            .append_pair("client_id", &client.id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", SCOPES)
            .append_pair("code_challenge", &pkce_challenge(&verifier))
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("access_type", "offline")
            // Forces a refresh token even if this account consented before.
            .append_pair("prompt", "consent");

        app.opener()
            .open_url(auth_url.as_str(), None::<&str>)
            .map_err(|e| AuthError::Browser(e.to_string()))?;

        let code = tokio::time::timeout(SIGN_IN_TIMEOUT, wait_for_code(&listener, &state))
            .await
            .map_err(|_| AuthError::TimedOut)??;

        let mut form = vec![
            ("client_id", client.id.as_str()),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ];
        if let Some(secret) = &client.secret {
            form.push(("client_secret", secret));
        }
        let response = self.http.post(TOKEN_URL).form(&form).send().await?;
        if !response.status().is_success() {
            let err: TokenError = response.json().await?;
            return Err(AuthError::Rejected(err.message()));
        }
        let token: TokenResponse = response.json().await?;
        let refresh_token = token
            .refresh_token
            .ok_or_else(|| AuthError::Rejected("no refresh token was returned".into()))?;

        store_refresh_token(&refresh_token)?;
        *self.cached.lock().await = Some(CachedToken {
            access_token: token.access_token,
            expires_at: Instant::now() + Duration::from_secs(token.expires_in),
        });
        self.connected.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Revokes the grant at Google (best effort) and deletes the local token.
    pub async fn disconnect(&self) -> Result<(), AuthError> {
        if let Ok(Some(token)) = read_refresh_token() {
            let revoke = self
                .http
                .post(REVOKE_URL)
                .form(&[("token", token.as_str())])
                .send()
                .await;
            if let Err(err) = revoke {
                log::warn!("token revoke failed: {err}");
            }
        }
        delete_refresh_token()?;
        *self.cached.lock().await = None;
        self.connected.store(false, Ordering::Relaxed);
        Ok(())
    }
}

/// Serves the loopback redirect until Google sends `?code=` (or an error).
async fn wait_for_code(listener: &TcpListener, expected_state: &str) -> Result<String, AuthError> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let mut len = 0;
        while len < buf.len() {
            let n = stream.read(&mut buf[len..]).await?;
            if n == 0 {
                break;
            }
            len += n;
            if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8_lossy(&buf[..len]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");

        let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
            respond(&mut stream, "400 Bad Request", "Bad request.").await;
            continue;
        };
        let param = |name: &str| url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned());

        if let Some(error) = param("error") {
            respond(&mut stream, "200 OK", "Sign-in was cancelled. You can close this tab.").await;
            return Err(AuthError::Rejected(error));
        }
        match (param("code"), param("state")) {
            (Some(code), Some(state)) if state == expected_state => {
                respond(&mut stream, "200 OK", "Calshell is connected. You can close this tab.").await;
                return Ok(code);
            }
            (Some(_), _) => {
                respond(&mut stream, "400 Bad Request", "State mismatch. Please try again.").await;
                return Err(AuthError::Rejected("state mismatch".into()));
            }
            // favicon.ico and other stray requests
            _ => respond(&mut stream, "404 Not Found", "Not found.").await,
        }
    }
}

async fn respond(stream: &mut tokio::net::TcpStream, status: &str, message: &str) {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Calshell</title></head>\
         <body style=\"font-family:'Segoe UI',system-ui,sans-serif;display:grid;place-items:center;\
         height:100vh;margin:0;background:#f3f3f3;color:#1b1b1b\"><p style=\"font-size:18px\">{message}</p></body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn random_token(bytes: usize) -> String {
    let data: Vec<u8> = (0..bytes).map(|_| rand::random::<u8>()).collect();
    URL_SAFE_NO_PAD.encode(data)
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn entry() -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
}

fn read_refresh_token() -> Result<Option<String>, keyring::Error> {
    match entry()?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err),
    }
}

fn store_refresh_token(token: &str) -> Result<(), keyring::Error> {
    entry()?.set_password(token)
}

fn delete_refresh_token() -> Result<(), keyring::Error> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_rfc_7636_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifier_has_valid_length() {
        let verifier = random_token(32);
        assert!((43..=128).contains(&verifier.len()));
    }

    #[test]
    fn config_client_overrides_builtin() {
        let config = Config {
            google_client_id: Some("custom.apps.googleusercontent.com".into()),
            ..Config::default()
        };
        let creds = credentials(&config).unwrap();
        assert_eq!(creds.id, "custom.apps.googleusercontent.com");
        assert_eq!(creds.secret, None);
    }
}
