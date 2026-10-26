//! Background sync: `events.list` with a `syncToken` per calendar, so after
//! the first full fetch each interval only downloads what changed.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use crate::auth::{self, AuthError};
use crate::config::Config;
use crate::gcal::{ApiError, CalendarListEntry, Event};
use crate::state::AppState;
use crate::tray;

/// Calendar list (names, colours, default reminders) changes rarely.
const CALENDAR_LIST_TTL: Duration = Duration::from_secs(15 * 60);
pub const STATE_CHANGED: &str = "calshell://state-changed";

pub struct CalendarState {
    pub entry: CalendarListEntry,
    pub sync_token: Option<String>,
    pub events: HashMap<String, Event>,
}

#[derive(Default)]
pub struct SyncStore {
    pub calendars: HashMap<String, CalendarState>,
    pub calendar_order: Vec<String>,
    pub calendars_fetched_at: Option<Instant>,
    pub last_success: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

/// A flattened, display-ready event used by the tray and the reminder scheduler.
#[derive(Debug, Clone)]
pub struct Upcoming {
    pub calendar_id: String,
    pub calendar_name: String,
    pub event_id: String,
    pub title: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub html_link: Option<String>,
    pub join_url: Option<String>,
    pub location: Option<String>,
    pub reminder_minutes: Vec<i64>,
}

impl SyncStore {
    /// Events that have not ended yet, soonest first, skipping cancelled,
    /// declined and disabled-calendar events.
    pub fn upcoming(&self, config: &Config, now: DateTime<Utc>) -> Vec<Upcoming> {
        let mut out: Vec<Upcoming> = self
            .calendars
            .values()
            .filter(|cal| config.calendar_enabled(&cal.entry.id))
            .flat_map(|cal| {
                cal.events.values().filter_map(move |ev| {
                    if ev.is_cancelled() || ev.declined_by_me() {
                        return None;
                    }
                    let start = ev.start_utc()?;
                    let end = ev.end_utc().unwrap_or(start);
                    if end <= now {
                        return None;
                    }
                    Some(Upcoming {
                        calendar_id: cal.entry.id.clone(),
                        calendar_name: cal.entry.name().to_string(),
                        event_id: ev.id.clone(),
                        title: ev.title(),
                        start,
                        end,
                        all_day: ev.is_all_day(),
                        html_link: ev.html_link.clone(),
                        join_url: ev.join_url(),
                        location: ev.location.clone(),
                        reminder_minutes: ev.popup_reminders(&cal.entry.default_reminders),
                    })
                })
            })
            .collect();
        out.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.title.cmp(&b.title)));
        out
    }

    fn set_calendars(&mut self, entries: Vec<CalendarListEntry>) {
        let mut old = std::mem::take(&mut self.calendars);
        self.calendar_order = entries.iter().map(|e| e.id.clone()).collect();
        for entry in entries {
            let state = match old.remove(&entry.id) {
                Some(mut existing) => {
                    existing.entry = entry;
                    existing
                }
                None => CalendarState {
                    entry,
                    sync_token: None,
                    events: HashMap::new(),
                },
            };
            self.calendars.insert(state.entry.id.clone(), state);
        }
        self.calendars_fetched_at = Some(Instant::now());
    }

    pub fn ordered_calendars(&self) -> impl Iterator<Item = &CalendarListEntry> {
        self.calendar_order
            .iter()
            .filter_map(|id| self.calendars.get(id).map(|c| &c.entry))
    }

    pub fn primary_calendar(&self) -> Option<&CalendarListEntry> {
        self.calendars.values().map(|c| &c.entry).find(|e| e.primary)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Api(#[from] ApiError),
}

/// Runs forever: sync, wait for the interval (or a "sync now" nudge), repeat.
pub async fn run(app: AppHandle) {
    let state = app.state::<AppState>();
    loop {
        if state.auth.is_connected() {
            let result = sync_once(&app).await;
            {
                let mut store = state.store();
                match &result {
                    Ok(()) => {
                        store.last_success = Some(Utc::now());
                        store.last_error = None;
                    }
                    Err(err) => {
                        log::warn!("sync failed: {err}");
                        store.last_error = Some(err.to_string());
                    }
                }
            }
            if let Err(SyncError::Api(err)) = &result {
                if err.is_unauthorized() {
                    state.auth.invalidate().await;
                }
            }
            let _ = app.emit(STATE_CHANGED, ());
        }
        tray::refresh(&app);

        let interval = Duration::from_secs(state.config().sync_interval_seconds);
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = state.sync_now.notified() => {}
        }
    }
}

async fn sync_once(app: &AppHandle) -> Result<(), SyncError> {
    let state = app.state::<AppState>();
    let config = state.config();
    let client = auth::credentials(&config).ok_or(AuthError::NoClient)?;
    let token = state.auth.access_token(&client).await?;

    let stale = state
        .store()
        .calendars_fetched_at
        .is_none_or(|at| at.elapsed() > CALENDAR_LIST_TTL);
    if stale {
        let entries = state.gcal.calendar_list(&token).await?;
        state.store().set_calendars(entries);
    }

    let targets: Vec<(String, Option<String>)> = state
        .store()
        .calendars
        .values()
        .filter(|c| config.calendar_enabled(&c.entry.id))
        .map(|c| (c.entry.id.clone(), c.sync_token.clone()))
        .collect();

    let now = Utc::now();
    let time_min = now - ChronoDuration::days(1);
    let mut first_error = None;

    for (calendar_id, sync_token) in targets {
        let mut result = state
            .gcal
            .list_events(&token, &calendar_id, sync_token.as_deref(), time_min)
            .await;
        let mut full = sync_token.is_none();
        if matches!(result, Err(ApiError::SyncTokenExpired)) {
            log::info!("sync token expired for {calendar_id}, doing a full resync");
            full = true;
            result = state.gcal.list_events(&token, &calendar_id, None, time_min).await;
        }

        match result {
            Ok(page) => {
                let mut store = state.store();
                if let Some(cal) = store.calendars.get_mut(&calendar_id) {
                    if full {
                        cal.events.clear();
                    }
                    for ev in page.events {
                        if ev.is_cancelled() {
                            cal.events.remove(&ev.id);
                        } else {
                            cal.events.insert(ev.id.clone(), ev);
                        }
                    }
                    // Past events never matter again; keep memory bounded.
                    cal.events
                        .retain(|_, ev| ev.end_utc().or(ev.start_utc()).is_none_or(|end| end > time_min));
                    cal.sync_token = page.next_sync_token;
                }
            }
            Err(err) if err.is_unauthorized() => return Err(err.into()),
            Err(err) => {
                log::warn!("sync failed for {calendar_id}: {err}");
                first_error.get_or_insert(err);
            }
        }
    }

    match first_error {
        Some(err) => Err(err.into()),
        None => Ok(()),
    }
}
