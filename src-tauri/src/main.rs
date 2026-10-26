// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auth;
mod commands;
mod config;
mod format;
mod gcal;
mod navigation;
mod quick_add;
mod reminders;
mod state;
mod sync;
mod tray;
mod windows;

use tauri::{AppHandle, Manager, RunEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::ShortcutState;
use tauri_plugin_log::{Target, TargetKind};
use tauri_plugin_window_state::StateFlags;

use crate::config::Config;
use crate::state::AppState;

/// Passed by the autostart entry so we know the launch came from login.
const AUTOSTART_ARG: &str = "--autostart";

fn main() {
    tauri::Builder::default()
        // Must be first: a second launch just focuses the running app.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            windows::show_main(app);
            open_calendar_arg(app, &args);
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::LogDir { file_name: None }),
                ])
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // Visibility is ours to decide (start minimised, close to tray).
                .with_state_flags(StateFlags::all() - StateFlags::VISIBLE)
                .with_filter(|label| label == windows::MAIN || label == windows::SETTINGS)
                .build(),
        )
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        windows::toggle_quick_add(app);
                    }
                })
                .build(),
        )
        .setup(|app| {
            setup(app.handle())?;
            Ok(())
        })
        .on_window_event(windows::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::connect_google,
            commands::disconnect_google,
            commands::quick_add_info,
            commands::quick_add,
            commands::hide_quick_add,
            commands::open_settings,
            commands::open_event,
            commands::open_link,
            commands::sync_now,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Calshell")
        .run(|_app, event| {
            // Closing the last window keeps Calshell in the tray; only an
            // explicit exit (tray Quit, close with close_to_tray off) quits.
            if let RunEvent::ExitRequested { api, code: None, .. } = event {
                api.prevent_exit();
            }
        });
}

fn setup(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = Config::path(app)?;
    let config = Config::load(&config_path);
    let reminders_path = app.path().app_local_data_dir()?.join("fired-reminders.json");
    log::info!(
        "Calshell {} starting, config at {}",
        app.package_info().version,
        config_path.display()
    );

    app.manage(AppState::new(config.clone(), config_path, reminders_path));

    tray::create(app)?;

    let args: Vec<String> = std::env::args().collect();
    let from_login = args.iter().any(|a| a == AUTOSTART_ARG);
    windows::create_main(app, !(from_login && config.start_minimized))?;
    windows::create_quick_add(app)?;
    open_calendar_arg(app, &args);

    if let Err(err) = windows::register_hotkey(app, &config.hotkey, None) {
        log::warn!("{err}");
    }

    // Keep the OS autostart entry in line with config. Skipped in dev so
    // `tauri dev` never registers a debug binary to run at login.
    if !tauri::is_dev() {
        let enabled = app.autolaunch().is_enabled().unwrap_or(false);
        if enabled != config.launch_on_startup {
            if let Err(err) = commands::apply_autostart(app, config.launch_on_startup) {
                log::warn!("{err}");
            }
        }
    }

    tauri::async_runtime::spawn(sync::run(app.clone()));
    tauri::async_runtime::spawn(reminders::run(app.clone()));
    Ok(())
}

/// `calshell.exe https://calendar.google.com/...` opens that page in the app.
fn open_calendar_arg(app: &AppHandle, args: &[String]) {
    let link = args.iter().skip(1).find_map(|arg| {
        let url = url::Url::parse(arg).ok()?;
        navigation::is_calendar(&url).then(|| arg.clone())
    });
    if let Some(link) = link {
        windows::open_event(app, Some(&link));
    }
}
