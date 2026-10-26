//! Fires native toasts at each event's popup reminder time, independent of
//! the main window. Fired reminders are persisted so a restart does not
//! repeat them.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Manager};

use crate::format;
use crate::state::AppState;
use crate::sync::Upcoming;
use crate::tray;

const TICK: Duration = Duration::from_secs(15);
/// Reminders missed by more than this (sleep, app closed) are dropped.
const MAX_LATENESS_MINUTES: i64 = 60;

/// Keys are `calendar|event|start|minutes`, values the event start (for pruning).
#[derive(Default)]
pub struct FiredReminders {
    path: Option<PathBuf>,
    fired: HashMap<String, DateTime<Utc>>,
}

impl FiredReminders {
    pub fn load(path: PathBuf) -> Self {
        let fired = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        Self {
            path: Some(path),
            fired,
        }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Err(err) = write_json(path, &self.fired) {
            log::warn!("could not save fired reminders: {err}");
        }
    }

    /// Returns events with a reminder due now, each paired with the offset to
    /// announce. If several offsets for one event are due (e.g. the app was
    /// closed), only the latest is shown.
    pub fn take_due(&mut self, upcoming: &[Upcoming], now: DateTime<Utc>) -> Vec<(Upcoming, i64)> {
        let mut due = Vec::new();
        let before = self.fired.len();
        for ev in upcoming {
            if ev.start < now - ChronoDuration::minutes(1) {
                continue;
            }
            let mut pending: Vec<(String, i64)> = Vec::new();
            for &minutes in &ev.reminder_minutes {
                let fire_at = ev.start - ChronoDuration::minutes(minutes);
                let lateness = now - fire_at;
                if lateness < ChronoDuration::zero() || lateness > ChronoDuration::minutes(MAX_LATENESS_MINUTES) {
                    continue;
                }
                let key = format!("{}|{}|{}|{minutes}", ev.calendar_id, ev.event_id, ev.start.timestamp());
                if !self.fired.contains_key(&key) {
                    pending.push((key, minutes));
                }
            }
            if let Some(&(_, minutes)) = pending.iter().min_by_key(|(_, m)| *m) {
                for (key, _) in pending {
                    self.fired.insert(key, ev.start);
                }
                due.push((ev.clone(), minutes));
            }
        }
        self.fired.retain(|_, start| *start > now - ChronoDuration::days(1));
        if self.fired.len() != before || !due.is_empty() {
            self.save();
        }
        due
    }
}

fn write_json(path: &Path, value: &impl serde::Serialize) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, serde_json::to_vec(value)?)
}

/// Ticks every 15s: fires due reminders and keeps the tray countdown fresh.
pub async fn run(app: AppHandle) {
    let state = app.state::<AppState>();
    loop {
        tokio::time::sleep(TICK).await;
        let config = state.config();
        let now = Utc::now();
        let upcoming = state.store().upcoming(&config, now);
        let due = state.reminders().take_due(&upcoming, now);
        if config.notifications_enabled {
            for (ev, _minutes) in due {
                show(&app, &ev, config.use_24h_clock, now);
            }
        }
        tray::refresh(&app);
    }
}

fn when_line(ev: &Upcoming, use_24h: bool, now: DateTime<Utc>) -> String {
    if ev.all_day {
        return format!("{}, all day", format::day_and_time(ev.start, true, use_24h, now));
    }
    let countdown = format::countdown(ev.start, now);
    let countdown = if countdown == "now" {
        "Starting now".to_string()
    } else {
        capitalize(&countdown)
    };
    format!(
        "{countdown} · {}–{}",
        format::clock(ev.start, use_24h),
        format::clock(ev.end, use_24h)
    )
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(windows)]
fn show(app: &AppHandle, ev: &Upcoming, use_24h: bool, now: DateTime<Utc>) {
    use tauri_winrt_notification::{Duration as ToastDuration, Sound, Toast};

    // Installed builds use the AUMID the NSIS installer puts on the Start
    // menu shortcut (the bundle identifier). Dev builds have no shortcut.
    let app_id = if tauri::is_dev() {
        Toast::POWERSHELL_APP_ID.to_string()
    } else {
        app.config().identifier.clone()
    };
    let detail = ev.location.clone().unwrap_or_else(|| ev.calendar_name.clone());

    let mut toast = Toast::new(&app_id)
        .title(&ev.title)
        .text1(&when_line(ev, use_24h, now))
        .text2(&format::truncate(&detail, 120))
        .sound(Some(Sound::Reminder))
        .duration(ToastDuration::Long);
    if ev.join_url.is_some() {
        toast = toast.add_button("Join", "join");
    }
    toast = toast.add_button("Open", "open");

    let handle = app.clone();
    let join_url = ev.join_url.clone();
    let html_link = ev.html_link.clone();
    toast = toast.on_activated(move |action| {
        match (action.as_deref(), &join_url) {
            (Some("join"), Some(url)) => crate::windows::open_external(&handle, url),
            _ => crate::windows::open_event(&handle, html_link.as_deref()),
        }
        Ok(())
    });

    if let Err(err) = toast.show() {
        log::warn!("could not show toast: {err}");
    }
}

#[cfg(not(windows))]
fn show(app: &AppHandle, ev: &Upcoming, use_24h: bool, now: DateTime<Utc>) {
    use tauri_plugin_notification::NotificationExt;
    let result = app
        .notification()
        .builder()
        .title(&ev.title)
        .body(when_line(ev, use_24h, now))
        .show();
    if let Err(err) = result {
        log::warn!("could not show notification: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upcoming(start: DateTime<Utc>, reminders: Vec<i64>) -> Upcoming {
        Upcoming {
            calendar_id: "primary".into(),
            calendar_name: "Me".into(),
            event_id: "e1".into(),
            title: "Standup".into(),
            start,
            end: start + ChronoDuration::minutes(15),
            all_day: false,
            html_link: None,
            join_url: None,
            location: None,
            reminder_minutes: reminders,
        }
    }

    #[test]
    fn fires_once_at_reminder_time() {
        let now = Utc::now();
        let mut fired = FiredReminders::default();
        let events = vec![upcoming(now + ChronoDuration::minutes(10), vec![10])];
        assert_eq!(fired.take_due(&events, now).len(), 1);
        assert!(fired.take_due(&events, now).is_empty());
    }

    #[test]
    fn not_due_yet() {
        let now = Utc::now();
        let mut fired = FiredReminders::default();
        let events = vec![upcoming(now + ChronoDuration::minutes(30), vec![10])];
        assert!(fired.take_due(&events, now).is_empty());
    }

    #[test]
    fn missed_reminders_collapse_into_the_latest() {
        let now = Utc::now();
        let mut fired = FiredReminders::default();
        let events = vec![upcoming(now + ChronoDuration::minutes(5), vec![30, 10])];
        let due = fired.take_due(&events, now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1, 10);
        assert!(fired.take_due(&events, now).is_empty());
    }

    #[test]
    fn stale_reminders_are_dropped() {
        let now = Utc::now();
        let mut fired = FiredReminders::default();
        let events = vec![upcoming(now + ChronoDuration::hours(20), vec![24 * 60])];
        assert!(fired.take_due(&events, now).is_empty());
    }

    #[test]
    fn rescheduled_events_fire_again() {
        let now = Utc::now();
        let mut fired = FiredReminders::default();
        let first = vec![upcoming(now + ChronoDuration::minutes(10), vec![10])];
        assert_eq!(fired.take_due(&first, now).len(), 1);
        let moved = vec![upcoming(now + ChronoDuration::minutes(9), vec![10])];
        assert_eq!(fired.take_due(&moved, now).len(), 1);
    }
}
