//! Tray icon: next events with Join/Open actions, plus a countdown tooltip.
//! The menu is only rebuilt when its content changes; the tooltip updates
//! on every tick.

use chrono::Utc;
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::format;
use crate::state::AppState;
use crate::sync::Upcoming;
use crate::windows;

const TRAY_ID: &str = "main";
const TITLE_MAX: usize = 40;

struct TrayEntry {
    html_link: Option<String>,
    join_url: Option<String>,
}

#[derive(Default)]
pub struct TrayModel {
    signature: Option<String>,
    entries: Vec<TrayEntry>,
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let menu = MenuBuilder::new(app)
        .text("open", "Open calendar")
        .separator()
        .text("quit", "Quit")
        .build()?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Calshell")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                windows::show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    refresh(app);
    Ok(())
}

/// Windows treats `&` in menu labels as a mnemonic marker.
fn menu_label(text: &str) -> String {
    text.replace('&', "&&")
}

fn event_label(ev: &Upcoming, use_24h: bool) -> String {
    let now = Utc::now();
    let when = if ev.start <= now {
        "Now".to_string()
    } else {
        format::day_and_time(ev.start, ev.all_day, use_24h, now)
    };
    menu_label(&format!("{when} · {}", format::truncate(&ev.title, TITLE_MAX)))
}

fn tooltip(connected: bool, next: Option<&Upcoming>) -> String {
    let text = match (connected, next) {
        (false, _) => "Calshell · not connected".to_string(),
        (true, None) => "Calshell · no upcoming events".to_string(),
        (true, Some(ev)) => {
            let countdown = format::countdown(ev.start, Utc::now());
            format!("{} {countdown}", format::truncate(&ev.title, 60))
        }
    };
    // Windows truncates tray tooltips at 127 characters.
    format::truncate(&text, 120)
}

pub fn refresh(app: &AppHandle) {
    let state = app.state::<AppState>();
    let config = state.config();
    let connected = state.auth.is_connected();
    let now = Utc::now();
    // The tray lists timed events only; all-day items (holidays, birthdays) are noise here.
    let events: Vec<Upcoming> = state
        .store()
        .upcoming(&config, now)
        .into_iter()
        .filter(|e| !e.all_day)
        .take(config.tray_event_count)
        .collect();

    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let _ = tray.set_tooltip(Some(tooltip(connected, events.first())));

    let labels: Vec<String> = events.iter().map(|e| event_label(e, config.use_24h_clock)).collect();
    let signature = format!(
        "{connected}|{}|{}",
        labels.join("\n"),
        events
            .iter()
            .map(|e| e.join_url.is_some().to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    if state.tray().signature.as_deref() == Some(signature.as_str()) {
        return;
    }

    match build_menu(app, connected, &events, &labels) {
        Ok(menu) => {
            if let Err(err) = tray.set_menu(Some(menu)) {
                log::warn!("could not update tray menu: {err}");
                return;
            }
            let mut model = state.tray();
            model.signature = Some(signature);
            model.entries = events
                .into_iter()
                .map(|e| TrayEntry {
                    html_link: e.html_link,
                    join_url: e.join_url,
                })
                .collect();
        }
        Err(err) => log::warn!("could not build tray menu: {err}"),
    }
}

fn build_menu(app: &AppHandle, connected: bool, events: &[Upcoming], labels: &[String]) -> tauri::Result<Menu<Wry>> {
    let mut menu = MenuBuilder::new(app);
    if !connected {
        menu = menu.text("connect", "Connect Google Calendar…");
    } else if events.is_empty() {
        menu = menu.item(
            &MenuItemBuilder::with_id("none", "No upcoming events")
                .enabled(false)
                .build(app)?,
        );
    } else {
        for (i, (ev, label)) in events.iter().zip(labels).enumerate() {
            let mut sub = SubmenuBuilder::new(app, label);
            if ev.join_url.is_some() {
                sub = sub.text(format!("event:{i}:join"), "Join meeting");
            }
            sub = sub.text(format!("event:{i}:open"), "Open in calendar");
            menu = menu.item(&sub.build()?);
        }
    }
    menu.separator()
        .text("new", "New event…")
        .text("open", "Open calendar")
        .text("sync", "Sync now")
        .text("settings", "Settings")
        .separator()
        .text("quit", "Quit Calshell")
        .build()
}

fn handle_menu(app: &AppHandle, id: &str) {
    match id {
        "open" => windows::show_main(app),
        "new" => windows::show_quick_add(app),
        "settings" | "connect" => windows::show_settings(app),
        "sync" => app.state::<AppState>().sync_now.notify_one(),
        "quit" => app.exit(0),
        other => {
            let mut parts = other.split(':');
            let (Some("event"), Some(index), Some(action)) = (parts.next(), parts.next(), parts.next()) else {
                return;
            };
            let Ok(index) = index.parse::<usize>() else { return };
            let (html_link, join_url) = {
                let state = app.state::<AppState>();
                let model = state.tray();
                let Some(entry) = model.entries.get(index) else { return };
                (entry.html_link.clone(), entry.join_url.clone())
            };
            match (action, join_url) {
                ("join", Some(url)) => windows::open_external(app, &url),
                _ => windows::open_event(app, html_link.as_deref()),
            }
        }
    }
}
