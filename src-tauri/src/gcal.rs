//! Minimal Google Calendar API v3 client: calendarList, events.list
//! (incremental via syncToken), events.quickAdd and events.patch.

use chrono::{DateTime, FixedOffset, Local, NaiveDate, TimeZone, Utc};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::Duration;
use url::Url;

const API: &str = "https://www.googleapis.com/calendar/v3/";
const MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("sync token expired")]
    SyncTokenExpired,
    #[error("Google Calendar returned {status}: {message}")]
    Status { status: u16, message: String },
}

impl ApiError {
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, ApiError::Status { status: 401, .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reminder {
    pub method: String,
    pub minutes: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarListEntry {
    pub id: String,
    #[serde(default)]
    pub summary: String,
    pub summary_override: Option<String>,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub access_role: String,
    #[serde(default)]
    pub default_reminders: Vec<Reminder>,
    pub background_color: Option<String>,
    #[serde(default)]
    pub deleted: bool,
}

impl CalendarListEntry {
    pub fn name(&self) -> &str {
        self.summary_override.as_deref().unwrap_or(&self.summary)
    }

    pub fn writable(&self) -> bool {
        matches!(self.access_role.as_str(), "owner" | "writer")
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventTime {
    pub date_time: Option<DateTime<FixedOffset>>,
    pub date: Option<NaiveDate>,
    pub time_zone: Option<String>,
}

impl EventTime {
    pub fn to_utc(&self) -> Option<DateTime<Utc>> {
        if let Some(dt) = self.date_time {
            return Some(dt.with_timezone(&Utc));
        }
        // All-day events start at local midnight.
        let midnight = self.date?.and_hms_opt(0, 0, 0)?;
        Local
            .from_local_datetime(&midnight)
            .earliest()
            .map(|dt| dt.with_timezone(&Utc))
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventReminders {
    #[serde(default)]
    pub use_default: bool,
    #[serde(default)]
    pub overrides: Vec<Reminder>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryPoint {
    pub entry_point_type: String,
    pub uri: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConferenceData {
    #[serde(default)]
    pub entry_points: Vec<EntryPoint>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attendee {
    #[serde(default, rename = "self")]
    pub is_self: bool,
    pub response_status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: String,
    #[serde(default)]
    pub status: String,
    pub summary: Option<String>,
    pub location: Option<String>,
    pub description: Option<String>,
    pub html_link: Option<String>,
    pub hangout_link: Option<String>,
    pub conference_data: Option<ConferenceData>,
    pub start: Option<EventTime>,
    pub end: Option<EventTime>,
    pub reminders: Option<EventReminders>,
    #[serde(default)]
    pub attendees: Vec<Attendee>,
}

/// Hosts we treat as "join a call" links when they appear in location or description.
const MEETING_HOSTS: [&str; 6] = [
    "meet.google.com",
    "zoom.us",
    "teams.microsoft.com",
    "teams.live.com",
    "webex.com",
    "whereby.com",
];

impl Event {
    pub fn title(&self) -> String {
        match self.summary.as_deref().map(str::trim) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => "(No title)".to_string(),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.status == "cancelled"
    }

    pub fn declined_by_me(&self) -> bool {
        self.attendees
            .iter()
            .any(|a| a.is_self && a.response_status.as_deref() == Some("declined"))
    }

    pub fn is_all_day(&self) -> bool {
        self.start.as_ref().is_some_and(|s| s.date_time.is_none())
    }

    pub fn start_utc(&self) -> Option<DateTime<Utc>> {
        self.start.as_ref()?.to_utc()
    }

    pub fn end_utc(&self) -> Option<DateTime<Utc>> {
        self.end.as_ref()?.to_utc()
    }

    /// Popup reminder offsets in minutes, resolving `useDefault`. Calendar
    /// default reminders only apply to timed events.
    pub fn popup_reminders(&self, calendar_defaults: &[Reminder]) -> Vec<i64> {
        let use_default = self.reminders.as_ref().is_none_or(|r| r.use_default);
        let source: &[Reminder] = match (&self.reminders, use_default) {
            (_, true) if self.is_all_day() => &[],
            (_, true) => calendar_defaults,
            (Some(r), false) => &r.overrides,
            (None, false) => &[],
        };
        source
            .iter()
            .filter(|r| r.method == "popup")
            .map(|r| r.minutes)
            .collect()
    }

    pub fn join_url(&self) -> Option<String> {
        if let Some(link) = &self.hangout_link {
            return Some(link.clone());
        }
        if let Some(conf) = &self.conference_data {
            if let Some(video) = conf.entry_points.iter().find(|e| e.entry_point_type == "video") {
                return Some(video.uri.clone());
            }
        }
        [&self.location, &self.description]
            .into_iter()
            .flatten()
            .find_map(|text| find_meeting_url(text))
    }
}

fn find_meeting_url(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '(' | ')'))
        .filter(|token| token.starts_with("https://"))
        .find_map(|token| {
            let url = Url::parse(token.trim_end_matches(['.', ',', ';'])).ok()?;
            let host = url.host_str()?;
            MEETING_HOSTS
                .iter()
                .any(|h| host == *h || host.ends_with(&format!(".{h}")))
                .then(|| url.to_string())
        })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CalendarListPage {
    #[serde(default)]
    items: Vec<CalendarListEntry>,
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventsPage {
    #[serde(default)]
    items: Vec<Event>,
    next_page_token: Option<String>,
    next_sync_token: Option<String>,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    #[serde(default)]
    message: String,
    #[serde(default)]
    errors: Vec<ErrorReason>,
}

#[derive(Deserialize)]
struct ErrorReason {
    #[serde(default)]
    reason: String,
}

pub struct EventsResult {
    pub events: Vec<Event>,
    pub next_sync_token: Option<String>,
}

#[derive(Clone)]
pub struct GcalClient {
    http: reqwest::Client,
}

fn endpoint(segments: &[&str]) -> Url {
    let mut url = Url::parse(API).expect("valid api url");
    url.path_segments_mut()
        .expect("http url")
        .pop_if_empty()
        .extend(segments);
    url
}

impl GcalClient {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// Sends a request, retrying on 429 and rate-limit 403s while honouring
    /// `Retry-After`.
    async fn send<T: DeserializeOwned>(&self, build: impl Fn() -> reqwest::RequestBuilder) -> Result<T, ApiError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let response = build().send().await?;
            let status = response.status();
            if status.is_success() {
                return Ok(response.json().await?);
            }
            if status.as_u16() == 410 {
                return Err(ApiError::SyncTokenExpired);
            }

            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let body = response.text().await.unwrap_or_default();
            let detail = serde_json::from_str::<ErrorBody>(&body).ok().map(|b| b.error);
            let rate_limited = status.as_u16() == 429
                || (status.as_u16() == 403
                    && detail.as_ref().is_some_and(|d| {
                        d.errors
                            .iter()
                            .any(|e| e.reason == "rateLimitExceeded" || e.reason == "userRateLimitExceeded")
                    }));

            if rate_limited && attempt < MAX_ATTEMPTS {
                let wait = retry_after.unwrap_or(2u64.pow(attempt)).min(60);
                log::warn!("rate limited, retrying in {wait}s");
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            return Err(ApiError::Status {
                status: status.as_u16(),
                message: detail.map(|d| d.message).unwrap_or(body),
            });
        }
    }

    pub async fn calendar_list(&self, token: &str) -> Result<Vec<CalendarListEntry>, ApiError> {
        let url = endpoint(&["users", "me", "calendarList"]);
        let mut entries = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let page: CalendarListPage = self
                .send(|| {
                    let mut req = self
                        .http
                        .get(url.clone())
                        .bearer_auth(token)
                        .query(&[("maxResults", "250")]);
                    if let Some(p) = &page_token {
                        req = req.query(&[("pageToken", p)]);
                    }
                    req
                })
                .await?;
            entries.extend(page.items.into_iter().filter(|c| !c.deleted));
            match page.next_page_token {
                Some(next) => page_token = Some(next),
                None => return Ok(entries),
            }
        }
    }

    /// Full sync when `sync_token` is `None` (from `time_min`), otherwise
    /// only the changes since the token was issued.
    pub async fn list_events(
        &self,
        token: &str,
        calendar_id: &str,
        sync_token: Option<&str>,
        time_min: DateTime<Utc>,
    ) -> Result<EventsResult, ApiError> {
        let url = endpoint(&["calendars", calendar_id, "events"]);
        let time_min = time_min.to_rfc3339();
        let mut events = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let page: EventsPage = self
                .send(|| {
                    let mut req = self
                        .http
                        .get(url.clone())
                        .bearer_auth(token)
                        .query(&[("singleEvents", "true"), ("maxResults", "2500")]);
                    req = match sync_token {
                        Some(sync) => req.query(&[("syncToken", sync)]),
                        None => req.query(&[("timeMin", time_min.as_str())]),
                    };
                    if let Some(p) = &page_token {
                        req = req.query(&[("pageToken", p)]);
                    }
                    req
                })
                .await?;
            events.extend(page.items);
            match page.next_page_token {
                Some(next) => page_token = Some(next),
                None => {
                    return Ok(EventsResult {
                        events,
                        next_sync_token: page.next_sync_token,
                    })
                }
            }
        }
    }

    pub async fn quick_add(&self, token: &str, calendar_id: &str, text: &str) -> Result<Event, ApiError> {
        let url = endpoint(&["calendars", calendar_id, "events", "quickAdd"]);
        self.send(|| self.http.post(url.clone()).bearer_auth(token).query(&[("text", text)]))
            .await
    }

    pub async fn set_event_end(
        &self,
        token: &str,
        calendar_id: &str,
        event_id: &str,
        end: DateTime<FixedOffset>,
        time_zone: Option<&str>,
    ) -> Result<Event, ApiError> {
        let url = endpoint(&["calendars", calendar_id, "events", event_id]);
        let mut end_json = serde_json::json!({ "dateTime": end.to_rfc3339() });
        if let Some(tz) = time_zone {
            end_json["timeZone"] = tz.into();
        }
        let body = serde_json::json!({ "end": end_json });
        self.send(|| self.http.patch(url.clone()).bearer_auth(token).json(&body))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(json: serde_json::Value) -> Event {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn calendar_ids_are_percent_encoded() {
        let url = endpoint(&["calendars", "en.usa#holiday@group.v.calendar.google.com", "events"]);
        assert_eq!(
            url.as_str(),
            "https://www.googleapis.com/calendar/v3/calendars/en.usa%23holiday@group.v.calendar.google.com/events"
        );
    }

    #[test]
    fn join_url_prefers_conference_data() {
        let ev = event(serde_json::json!({
            "id": "1",
            "hangoutLink": "https://meet.google.com/abc-defg-hij",
            "location": "https://zoom.us/j/123"
        }));
        assert_eq!(ev.join_url().unwrap(), "https://meet.google.com/abc-defg-hij");
    }

    #[test]
    fn join_url_found_in_location() {
        let ev = event(serde_json::json!({
            "id": "1",
            "location": "Room 4 / https://acme.zoom.us/j/123?pwd=x."
        }));
        assert_eq!(ev.join_url().unwrap(), "https://acme.zoom.us/j/123?pwd=x");
        let none = event(serde_json::json!({ "id": "2", "location": "https://example.com/zoom.us" }));
        assert_eq!(none.join_url(), None);
    }

    #[test]
    fn reminders_resolve_defaults() {
        let defaults = vec![
            Reminder {
                method: "popup".into(),
                minutes: 10,
            },
            Reminder {
                method: "email".into(),
                minutes: 60,
            },
        ];
        let timed = event(serde_json::json!({
            "id": "1",
            "start": { "dateTime": "2026-10-27T15:00:00+01:00" },
            "reminders": { "useDefault": true }
        }));
        assert_eq!(timed.popup_reminders(&defaults), vec![10]);

        let custom = event(serde_json::json!({
            "id": "2",
            "start": { "dateTime": "2026-10-27T15:00:00Z" },
            "reminders": { "useDefault": false, "overrides": [{ "method": "popup", "minutes": 5 }] }
        }));
        assert_eq!(custom.popup_reminders(&defaults), vec![5]);

        let all_day = event(serde_json::json!({ "id": "3", "start": { "date": "2026-10-27" } }));
        assert!(all_day.is_all_day());
        assert!(all_day.popup_reminders(&defaults).is_empty());
    }

    #[test]
    fn declined_events_are_detected() {
        let ev = event(serde_json::json!({
            "id": "1",
            "attendees": [{ "self": true, "responseStatus": "declined" }, { "responseStatus": "accepted" }]
        }));
        assert!(ev.declined_by_me());
    }
}
