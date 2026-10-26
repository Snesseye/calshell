//! User settings, stored as JSON in `%APPDATA%\calshell\config.json`.
//! Tokens never live here; see `auth.rs`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkey: String,
    pub default_calendar_id: String,
    pub sync_interval_seconds: u64,
    pub launch_on_startup: bool,
    pub start_minimized: bool,
    pub close_to_tray: bool,
    pub tray_event_count: usize,
    pub default_event_duration_minutes: u32,
    pub notifications_enabled: bool,
    pub use_24h_clock: bool,
    /// Calendars excluded from sync, the tray and notifications.
    pub disabled_calendar_ids: Vec<String>,
    /// Overrides the WebView2 user agent if Google ever blocks the embedded sign-in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    /// Bring-your-own OAuth client, for builds without one compiled in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub google_client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub google_client_secret: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: "Ctrl+Shift+Space".into(),
            default_calendar_id: "primary".into(),
            sync_interval_seconds: 60,
            launch_on_startup: true,
            start_minimized: true,
            close_to_tray: true,
            tray_event_count: 3,
            default_event_duration_minutes: 30,
            notifications_enabled: true,
            use_24h_clock: true,
            disabled_calendar_ids: Vec::new(),
            user_agent: None,
            google_client_id: None,
            google_client_secret: None,
        }
    }
}

impl Config {
    pub fn path(app: &AppHandle) -> tauri::Result<PathBuf> {
        Ok(app.path().app_config_dir()?.join("config.json"))
    }

    /// Loads the config, writing defaults on first run. A corrupt file is
    /// kept as `config.json.bak` so a typo never silently wipes settings.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(raw) => match serde_json::from_str::<Config>(&raw) {
                Ok(config) => config.normalized(),
                Err(err) => {
                    log::warn!("config.json is invalid ({err}), using defaults");
                    let _ = std::fs::rename(path, path.with_extension("json.bak"));
                    Self::default()
                }
            },
            Err(_) => {
                let config = Self::default();
                if let Err(err) = config.save(path) {
                    log::warn!("could not write default config: {err}");
                }
                config
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }

    pub fn normalized(mut self) -> Self {
        self.sync_interval_seconds = self.sync_interval_seconds.clamp(30, 3600);
        self.tray_event_count = self.tray_event_count.clamp(1, 10);
        self.default_event_duration_minutes = self.default_event_duration_minutes.clamp(5, 24 * 60);
        if self.default_calendar_id.trim().is_empty() {
            self.default_calendar_id = "primary".into();
        }
        for field in [
            &mut self.user_agent,
            &mut self.google_client_id,
            &mut self.google_client_secret,
        ] {
            if field.as_deref().is_some_and(|v| v.trim().is_empty()) {
                *field = None;
            }
        }
        self
    }

    pub fn calendar_enabled(&self, calendar_id: &str) -> bool {
        !self.disabled_calendar_ids.iter().any(|id| id == calendar_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_fill_in_defaults() {
        let config: Config = serde_json::from_str(r#"{ "hotkey": "Alt+Space" }"#).unwrap();
        assert_eq!(config.hotkey, "Alt+Space");
        assert_eq!(config.sync_interval_seconds, 60);
        assert!(config.close_to_tray);
    }

    #[test]
    fn normalized_clamps_and_drops_blanks() {
        let config = Config {
            sync_interval_seconds: 1,
            tray_event_count: 0,
            google_client_id: Some("  ".into()),
            ..Config::default()
        }
        .normalized();
        assert_eq!(config.sync_interval_seconds, 30);
        assert_eq!(config.tray_event_count, 1);
        assert_eq!(config.google_client_id, None);
    }
}
