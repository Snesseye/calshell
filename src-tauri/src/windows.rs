//! Window lifecycle: the Google Calendar main window, in-app popups, the
//! quick-add panel, the settings window, and the global hotkey.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{
    AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window, WindowEvent,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
use tauri_plugin_opener::OpenerExt;
use url::Url;

use crate::navigation::{self, NewWindow, Route};
use crate::state::AppState;

pub const MAIN: &str = "main";
pub const QUICK_ADD: &str = "quick-add";
pub const SETTINGS: &str = "settings";
const POPUP_PREFIX: &str = "popup-";
const CALENDAR_URL: &str = "https://calendar.google.com/calendar/r";
pub const QUICK_ADD_SHOWN: &str = "calshell://quick-add-shown";

static POPUP_COUNTER: AtomicU32 = AtomicU32::new(0);

pub fn open_external(app: &AppHandle, url: &str) {
    if let Err(err) = app.opener().open_url(url, None::<&str>) {
        log::warn!("could not open {url}: {err}");
    }
}

fn route_external<R: Runtime>(app: &AppHandle<R>, url: &Url) {
    if let Err(err) = app.opener().open_url(url.as_str(), None::<&str>) {
        log::warn!("could not open {url}: {err}");
    }
}

pub fn create_main(app: &AppHandle, visible: bool) -> tauri::Result<WebviewWindow> {
    let config = app.state::<AppState>().config();
    // False while the user is on a sign-in page, so SSO redirects to
    // third-party identity providers are not bounced to the browser.
    let on_calendar = Arc::new(AtomicBool::new(false));

    let nav_app = app.clone();
    let nav_flag = on_calendar.clone();
    let load_flag = on_calendar.clone();
    let popup_app = app.clone();

    let mut builder = WebviewWindowBuilder::new(
        app,
        MAIN,
        WebviewUrl::External(Url::parse(CALENDAR_URL).expect("valid calendar url")),
    )
    .title("Calshell")
    .inner_size(1280.0, 860.0)
    .min_inner_size(720.0, 520.0)
    .center()
    .visible(false)
    .on_navigation(move |url| {
        if navigation::is_signed_out_landing(url) {
            // Navigating from inside this handler can deadlock WebView2, so
            // swap in the sign-in page from another thread.
            let app = nav_app.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(window) = app.get_webview_window(MAIN) {
                    let sign_in = Url::parse(navigation::SIGN_IN_URL).expect("valid sign-in url");
                    if let Err(err) = window.navigate(sign_in) {
                        log::warn!("could not open Google sign-in: {err}");
                    }
                }
            });
            return false;
        }
        if !nav_flag.load(Ordering::Relaxed) {
            return true;
        }
        match navigation::classify(url) {
            Route::Internal => true,
            Route::External(target) => {
                route_external(&nav_app, &target);
                false
            }
        }
    })
    .on_page_load(move |_window, payload| {
        if payload.event() == PageLoadEvent::Started {
            load_flag.store(navigation::is_calendar(payload.url()), Ordering::Relaxed);
        }
    })
    .on_new_window(move |url, features| handle_new_window(&popup_app, url, features));

    if let Some(user_agent) = &config.user_agent {
        builder = builder.user_agent(user_agent);
    }
    let window = builder.build()?;
    if visible {
        window.show()?;
        window.set_focus()?;
    }
    Ok(window)
}

fn handle_new_window(
    app: &AppHandle,
    url: Url,
    features: tauri::webview::NewWindowFeatures,
) -> NewWindowResponse<tauri::Wry> {
    match navigation::classify_new_window(&url, features.size().is_some()) {
        NewWindow::Browser(target) => {
            route_external(app, &target);
            NewWindowResponse::Deny
        }
        NewWindow::Popup => match create_popup(app, &url, features) {
            Ok(window) => NewWindowResponse::Create { window },
            Err(err) => {
                log::warn!("could not open popup for {url}: {err}");
                NewWindowResponse::Deny
            }
        },
    }
}

/// In-app child window sharing the opener's WebView2 environment, so
/// `window.opener`, cookies and print previews keep working.
fn create_popup(
    app: &AppHandle,
    url: &Url,
    features: tauri::webview::NewWindowFeatures,
) -> tauri::Result<WebviewWindow> {
    let label = format!("{POPUP_PREFIX}{}", POPUP_COUNTER.fetch_add(1, Ordering::Relaxed));
    let (width, height) = features
        .size()
        .map(|s| (s.width.max(400.0), s.height.max(300.0)))
        .unwrap_or((1000.0, 760.0));
    let nested_app = app.clone();

    let mut builder = WebviewWindowBuilder::new(app, label, WebviewUrl::External(url.clone()))
        .title("Calshell")
        .inner_size(width, height)
        .center()
        .on_document_title_changed(|window, title| {
            let _ = window.set_title(&title);
        })
        // Popups from popups (rare) are not worth nesting: send them out.
        .on_new_window(move |url, _| {
            if matches!(url.scheme(), "http" | "https") {
                route_external(&nested_app, &url);
            }
            NewWindowResponse::Deny
        })
        .window_features(features);
    if let Some(user_agent) = &app.state::<AppState>().config().user_agent {
        builder = builder.user_agent(user_agent);
    }
    builder.build()
}

pub fn show_main(app: &AppHandle) {
    let window = match app.get_webview_window(MAIN) {
        Some(window) => window,
        None => match create_main(app, true) {
            Ok(window) => window,
            Err(err) => {
                log::error!("could not create main window: {err}");
                return;
            }
        },
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// Shows the main window on a specific event (tray item, toast click).
pub fn open_event(app: &AppHandle, html_link: Option<&str>) {
    show_main(app);
    let Some(link) = html_link.and_then(|l| Url::parse(l).ok()) else {
        return;
    };
    if navigation::classify(&link) != Route::Internal {
        return;
    }
    if let Some(window) = app.get_webview_window(MAIN) {
        if let Err(err) = window.navigate(link) {
            log::warn!("could not open event: {err}");
        }
    }
}

pub fn create_quick_add(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, QUICK_ADD, WebviewUrl::App("index.html".into()))
        .title("Quick add")
        .inner_size(640.0, 104.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(true)
        .center()
        .visible(false)
        .build()
}

pub fn show_quick_add(app: &AppHandle) {
    let window = match app.get_webview_window(QUICK_ADD) {
        Some(window) => window,
        None => match create_quick_add(app) {
            Ok(window) => window,
            Err(err) => {
                log::error!("could not create quick-add window: {err}");
                return;
            }
        },
    };
    let _ = window.center();
    let _ = window.show();
    let _ = window.set_focus();
    let _ = window.emit(QUICK_ADD_SHOWN, ());
}

pub fn hide_quick_add(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(QUICK_ADD) {
        let _ = window.hide();
    }
}

pub fn toggle_quick_add(app: &AppHandle) {
    let visible = app
        .get_webview_window(QUICK_ADD)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if visible {
        hide_quick_add(app);
    } else {
        show_quick_add(app);
    }
}

pub fn show_settings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(SETTINGS) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let result = WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App("index.html".into()))
        .title("Calshell settings")
        .inner_size(600.0, 760.0)
        .min_inner_size(480.0, 520.0)
        .center()
        .build();
    if let Err(err) = result {
        log::error!("could not open settings: {err}");
    }
}

pub fn on_window_event(window: &Window, event: &WindowEvent) {
    let app = window.app_handle();
    match (window.label(), event) {
        (MAIN, WindowEvent::CloseRequested { api, .. }) => {
            if app.state::<AppState>().config().close_to_tray {
                api.prevent_close();
                let _ = window.hide();
            } else {
                app.exit(0);
            }
        }
        (QUICK_ADD, WindowEvent::Focused(false)) => {
            let _ = window.hide();
        }
        _ => {}
    }
}

/// Swaps the registered global hotkey. On failure the previous one is
/// restored and a user-facing message is returned.
pub fn register_hotkey(app: &AppHandle, hotkey: &str, previous: Option<&str>) -> Result<(), String> {
    let shortcut = Shortcut::from_str(hotkey).map_err(|_| format!("\"{hotkey}\" is not a valid shortcut."))?;
    let manager = app.global_shortcut();
    let previous = previous.and_then(|p| Shortcut::from_str(p).ok());
    if let Some(old) = previous {
        if old == shortcut && manager.is_registered(old) {
            return Ok(());
        }
        let _ = manager.unregister(old);
    }
    match manager.register(shortcut) {
        Ok(()) => {
            *app.state::<AppState>().hotkey_error() = None;
            Ok(())
        }
        Err(err) => {
            log::warn!("could not register {hotkey}: {err}");
            if let Some(old) = previous {
                let _ = manager.register(old);
            }
            let message = format!("{hotkey} is already used by another app. Pick a different shortcut.");
            *app.state::<AppState>().hotkey_error() = Some(message.clone());
            Err(message)
        }
    }
}
