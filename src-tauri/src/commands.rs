//! Commands exposed to the bundled UI (quick-add and settings windows).
//!
//! The main window and popups load google.com, so every command checks the
//! calling webview's label and refuses anything that is not our own UI.

use chrono::{Duration as ChronoDuration, Utc};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Webview};
use tauri_plugin_autostart::ManagerExt;

use crate::auth::{self, AuthError};
use crate::config::Config;
use crate::format;
use crate::quick_add;
use crate::state::AppState;
use crate::sync::STATE_CHANGED;
use crate::tray;
use crate::windows::{self, QUICK_ADD, SETTINGS};

type CmdResult<T> = Result<T, String>;

fn ensure_trusted(webview: &Webview) -> CmdResult<()> {
    match webview.label() {
        QUICK_ADD | SETTINGS => Ok(()),
        other => {
            log::warn!("blocked command from untrusted webview {other}");
            Err("Not allowed".into())
        }
    }
}

#[derive(Serialize)]
pub struct CalendarView {
    id: String,
    name: String,
    color: Option<String>,
    primary: bool,
    writable: bool,
    enabled: bool,
}

#[derive(Serialize)]
pub struct SettingsView {
    config: Config,
    connected: bool,
    account: Option<String>,
    calendars: Vec<CalendarView>,
    builtin_client: bool,
    hotkey_error: Option<String>,
    last_sync: Option<String>,
    last_error: Option<String>,
    config_path: String,
    version: String,
}

fn settings_view(app: &AppHandle) -> SettingsView {
    let state = app.state::<AppState>();
    let config = state.config();
    let hotkey_error = state.hotkey_error().clone();
    let store = state.store();
    let now = Utc::now();
    let calendars = store
        .ordered_calendars()
        .map(|c| CalendarView {
            id: c.id.clone(),
            name: c.name().to_string(),
            color: c.background_color.clone(),
            primary: c.primary,
            writable: c.writable(),
            enabled: config.calendar_enabled(&c.id),
        })
        .collect();
    SettingsView {
        connected: state.auth.is_connected(),
        // The primary calendar's id is the account's email address.
        account: store.primary_calendar().map(|c| c.id.clone()),
        calendars,
        builtin_client: auth::builtin_client().is_some(),
        hotkey_error,
        last_sync: store
            .last_success
            .map(|t| format::day_and_time(t, false, config.use_24h_clock, now)),
        last_error: store.last_error.clone(),
        config_path: state.config_path.display().to_string(),
        version: app.package_info().version.to_string(),
        config,
    }
}

fn notify_changed(app: &AppHandle) {
    let _ = app.emit(STATE_CHANGED, ());
}

#[tauri::command]
pub fn get_settings(app: AppHandle, webview: Webview) -> CmdResult<SettingsView> {
    ensure_trusted(&webview)?;
    Ok(settings_view(&app))
}

#[tauri::command]
pub fn save_settings(app: AppHandle, webview: Webview, config: Config) -> CmdResult<SettingsView> {
    ensure_trusted(&webview)?;
    let state = app.state::<AppState>();
    let old = state.config();
    let new = config.normalized();

    if new.hotkey != old.hotkey {
        windows::register_hotkey(&app, &new.hotkey, Some(&old.hotkey))?;
    }
    if new.launch_on_startup != old.launch_on_startup {
        apply_autostart(&app, new.launch_on_startup)?;
    }
    let client_changed =
        new.google_client_id != old.google_client_id || new.google_client_secret != old.google_client_secret;
    let resync = client_changed
        || new.disabled_calendar_ids != old.disabled_calendar_ids
        || new.sync_interval_seconds != old.sync_interval_seconds;

    state
        .set_config(new)
        .map_err(|err| format!("Could not save settings: {err}"))?;

    if resync {
        state.sync_now.notify_one();
    }
    tray::refresh(&app);
    notify_changed(&app);
    Ok(settings_view(&app))
}

pub fn apply_autostart(app: &AppHandle, enabled: bool) -> CmdResult<()> {
    let launcher = app.autolaunch();
    let result = if enabled { launcher.enable() } else { launcher.disable() };
    result.map_err(|err| format!("Could not change launch on startup: {err}"))
}

#[tauri::command]
pub async fn connect_google(app: AppHandle, webview: Webview) -> CmdResult<SettingsView> {
    ensure_trusted(&webview)?;
    let state = app.state::<AppState>();
    let client = auth::credentials(&state.config()).ok_or_else(|| AuthError::NoClient.to_string())?;
    state.auth.connect(&app, &client).await.map_err(|e| e.to_string())?;

    // Back from the browser: bring settings forward and load calendars now.
    if let Some(window) = app.get_webview_window(SETTINGS) {
        let _ = window.set_focus();
    }
    state.sync_now.notify_one();
    tray::refresh(&app);
    notify_changed(&app);
    Ok(settings_view(&app))
}

#[tauri::command]
pub async fn disconnect_google(app: AppHandle, webview: Webview) -> CmdResult<SettingsView> {
    ensure_trusted(&webview)?;
    let state = app.state::<AppState>();
    state.auth.disconnect().await.map_err(|e| e.to_string())?;
    *state.store() = Default::default();
    tray::refresh(&app);
    notify_changed(&app);
    Ok(settings_view(&app))
}

#[derive(Serialize)]
pub struct QuickAddInfo {
    connected: bool,
    calendar: Option<String>,
}

#[tauri::command]
pub fn quick_add_info(app: AppHandle, webview: Webview) -> CmdResult<QuickAddInfo> {
    ensure_trusted(&webview)?;
    let state = app.state::<AppState>();
    let config = state.config();
    let store = state.store();
    let calendar = match config.default_calendar_id.as_str() {
        "primary" => store.primary_calendar(),
        id => store.calendars.get(id).map(|c| &c.entry),
    }
    .map(|c| c.name().to_string());
    Ok(QuickAddInfo {
        connected: state.auth.is_connected(),
        calendar,
    })
}

#[derive(Serialize)]
pub struct QuickAddResult {
    title: String,
    when: String,
    html_link: Option<String>,
}

#[tauri::command]
pub async fn quick_add(app: AppHandle, webview: Webview, text: String) -> CmdResult<QuickAddResult> {
    ensure_trusted(&webview)?;
    let text = text.trim();
    if text.is_empty() {
        return Err("Type an event first.".into());
    }
    let state = app.state::<AppState>();
    let config = state.config();
    let client = auth::credentials(&config).ok_or_else(|| AuthError::NoClient.to_string())?;
    let token = state.auth.access_token(&client).await.map_err(|e| e.to_string())?;
    let calendar_id = config.default_calendar_id.as_str();

    let mut event = state
        .gcal
        .quick_add(&token, calendar_id, text)
        .await
        .map_err(|e| e.to_string())?;

    // quickAdd defaults to 1 hour; apply the configured length instead.
    let wanted = i64::from(config.default_event_duration_minutes);
    if wanted != 60 && !quick_add::has_explicit_length(text) {
        if let (Some(start), Some(end)) = (
            event.start.as_ref().and_then(|s| s.date_time),
            event.end.as_ref().and_then(|e| e.date_time),
        ) {
            if end - start == ChronoDuration::minutes(60) {
                let time_zone = event.end.as_ref().and_then(|e| e.time_zone.clone());
                match state
                    .gcal
                    .set_event_end(
                        &token,
                        calendar_id,
                        &event.id,
                        start + ChronoDuration::minutes(wanted),
                        time_zone.as_deref(),
                    )
                    .await
                {
                    Ok(updated) => event = updated,
                    Err(err) => log::warn!("could not apply default duration: {err}"),
                }
            }
        }
    }

    state.sync_now.notify_one();
    let when = event
        .start_utc()
        .map(|start| format::day_and_time(start, event.is_all_day(), config.use_24h_clock, Utc::now()))
        .unwrap_or_default();
    Ok(QuickAddResult {
        title: event.title(),
        when,
        html_link: event.html_link,
    })
}

#[tauri::command]
pub fn hide_quick_add(app: AppHandle, webview: Webview) -> CmdResult<()> {
    ensure_trusted(&webview)?;
    windows::hide_quick_add(&app);
    Ok(())
}

#[tauri::command]
pub fn open_settings(app: AppHandle, webview: Webview) -> CmdResult<()> {
    ensure_trusted(&webview)?;
    windows::hide_quick_add(&app);
    windows::show_settings(&app);
    Ok(())
}

#[tauri::command]
pub fn open_event(app: AppHandle, webview: Webview, url: Option<String>) -> CmdResult<()> {
    ensure_trusted(&webview)?;
    windows::hide_quick_add(&app);
    windows::open_event(&app, url.as_deref());
    Ok(())
}

#[tauri::command]
pub fn open_link(app: AppHandle, webview: Webview, url: String) -> CmdResult<()> {
    ensure_trusted(&webview)?;
    if !url.starts_with("https://") {
        return Err("Only https links can be opened".into());
    }
    windows::open_external(&app, &url);
    Ok(())
}

#[tauri::command]
pub fn sync_now(app: AppHandle, webview: Webview) -> CmdResult<()> {
    ensure_trusted(&webview)?;
    app.state::<AppState>().sync_now.notify_one();
    Ok(())
}
