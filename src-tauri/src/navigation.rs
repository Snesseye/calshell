//! Decides what stays inside Calshell and what goes to the default browser.
//!
//! The main window keeps Google Calendar and Google sign-in. Everything else
//! (Meet, Zoom, Drive attachments, any other site) opens externally. While
//! the user is signing in, every navigation is allowed so Workspace SSO
//! through third-party identity providers keeps working.

use url::Url;

#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    Internal,
    External(Url),
}

/// Hosts that make up the calendar app and Google sign-in.
fn is_app_url(url: &Url) -> bool {
    let Some(host) = url.host_str() else { return false };
    let host = host.to_ascii_lowercase();
    match host.as_str() {
        "calendar.google.com" | "accounts.google.com" | "accounts.youtube.com" => true,
        // Workspace sign-in (google.com/a/...) and legacy calendar paths.
        "www.google.com" | "google.com" => ["/calendar", "/a/", "/accounts"]
            .iter()
            .any(|prefix| url.path().starts_with(prefix)),
        // Country sign-in redirects such as accounts.google.co.uk
        _ => host.starts_with("accounts.google."),
    }
}

/// Unwraps Google's `https://www.google.com/url?q=<target>` link redirector.
fn unwrap_redirect(url: &Url) -> Option<Url> {
    let host = url.host_str()?;
    if !(host == "www.google.com" || host == "google.com") || url.path() != "/url" {
        return None;
    }
    url.query_pairs()
        .find(|(k, _)| k == "q" || k == "url")
        .and_then(|(_, v)| Url::parse(&v).ok())
}

pub fn classify(url: &Url) -> Route {
    match url.scheme() {
        "about" | "data" | "blob" | "javascript" => return Route::Internal,
        "http" | "https" => {}
        // mailto:, tel:, zoommtg:, msteams: and friends
        _ => return Route::External(url.clone()),
    }
    if let Some(target) = unwrap_redirect(url) {
        return Route::External(target);
    }
    if is_app_url(url) {
        Route::Internal
    } else {
        Route::External(url.clone())
    }
}

/// Google Calendar's own sign-in entry point (`service=cl`); it returns to
/// the calendar once the user is signed in.
pub const SIGN_IN_URL: &str = "https://accounts.google.com/ServiceLogin?service=cl&passive=1209600&osid=1\
&continue=https://calendar.google.com/calendar/r&followup=https://calendar.google.com/calendar/r";

/// Signed-out visitors are sometimes sent to the Workspace marketing page,
/// which offers sign-up and the Admin console but no obvious sign-in.
pub fn is_signed_out_landing(url: &Url) -> bool {
    match url.host_str() {
        Some("workspace.google.com") => url.path().contains("/products/calendar"),
        Some("www.google.com" | "google.com") => url.path().starts_with("/calendar/about"),
        _ => false,
    }
}

pub fn is_calendar(url: &Url) -> bool {
    url.host_str() == Some("calendar.google.com")
}

/// What to do with a `window.open` / `target=_blank` request.
#[derive(Debug, PartialEq, Eq)]
pub enum NewWindow {
    /// Open an in-app popup that keeps `window.opener` (print, add-on auth,
    /// "open in new window").
    Popup,
    Browser(Url),
}

/// `scripted` is true when the page asked for a sized popup, which is how
/// add-ons run their sign-in flows; those must stay attached to the opener.
pub fn classify_new_window(url: &Url, scripted: bool) -> NewWindow {
    if url.scheme() == "about" {
        return NewWindow::Popup;
    }
    // Camera and mic in WebView2 are unreliable; Meet always gets the browser.
    if url.host_str() == Some("meet.google.com") {
        return NewWindow::Browser(url.clone());
    }
    match classify(url) {
        Route::Internal => NewWindow::Popup,
        Route::External(target) if scripted && matches!(target.scheme(), "http" | "https") && target == *url => {
            NewWindow::Popup
        }
        Route::External(target) => NewWindow::Browser(target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn calendar_and_sign_in_stay_inside() {
        for u in [
            "https://calendar.google.com/calendar/r/week",
            "https://accounts.google.com/ServiceLogin?continue=x",
            "https://accounts.google.co.uk/accounts/SetSID",
            "https://www.google.com/a/example.com/ServiceLogin",
            "https://www.google.com/calendar/event?eid=abc",
            "about:blank",
        ] {
            assert_eq!(classify(&url(u)), Route::Internal, "{u}");
        }
    }

    #[test]
    fn everything_else_goes_to_the_browser() {
        for u in [
            "https://meet.google.com/abc-defg-hij",
            "https://zoom.us/j/123",
            "https://docs.google.com/document/d/1",
            "https://example.com/",
            "mailto:sam@example.com",
        ] {
            assert!(matches!(classify(&url(u)), Route::External(_)), "{u}");
        }
    }

    #[test]
    fn marketing_landing_is_detected() {
        for u in [
            "https://workspace.google.com/products/calendar/",
            "https://workspace.google.com/intl/nl/products/calendar/?utm_source=x",
            "https://www.google.com/calendar/about/",
        ] {
            assert!(is_signed_out_landing(&url(u)), "{u}");
        }
        for u in [
            "https://workspace.google.com/pricing",
            "https://calendar.google.com/calendar/r",
            "https://www.google.com/calendar/event?eid=abc",
        ] {
            assert!(!is_signed_out_landing(&url(u)), "{u}");
        }
        assert!(Url::parse(SIGN_IN_URL).unwrap().host_str() == Some("accounts.google.com"));
    }

    #[test]
    fn google_redirector_is_unwrapped() {
        let wrapped = url("https://www.google.com/url?q=https%3A%2F%2Fexample.com%2Fdoc&sa=D");
        assert_eq!(classify(&wrapped), Route::External(url("https://example.com/doc")));
    }

    #[test]
    fn scripted_popups_stay_in_app() {
        let addon = url("https://zoom.us/oauth/authorize?client_id=x");
        assert_eq!(classify_new_window(&addon, true), NewWindow::Popup);
        assert_eq!(classify_new_window(&addon, false), NewWindow::Browser(addon.clone()));
        let meet = url("https://meet.google.com/abc");
        assert_eq!(classify_new_window(&meet, false), NewWindow::Browser(meet.clone()));
        assert_eq!(classify_new_window(&meet, true), NewWindow::Browser(meet.clone()));
        assert_eq!(classify_new_window(&url("about:blank"), false), NewWindow::Popup);
    }
}
