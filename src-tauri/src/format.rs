//! Human-friendly time labels shared by the tray, toasts and quick-add.

use chrono::{DateTime, Datelike, Local, Utc};

pub fn clock(dt: DateTime<Utc>, use_24h: bool) -> String {
    let local = dt.with_timezone(&Local);
    if use_24h {
        local.format("%H:%M").to_string()
    } else {
        local.format("%-I:%M %p").to_string()
    }
}

/// "15:00", "Tomorrow 09:00", "Tue 09:00" or "3 Nov 09:00".
pub fn day_and_time(dt: DateTime<Utc>, all_day: bool, use_24h: bool, now: DateTime<Utc>) -> String {
    let local = dt.with_timezone(&Local);
    let days = (local.date_naive() - now.with_timezone(&Local).date_naive()).num_days();
    let day = match days {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        2..=6 => local.format("%a").to_string(),
        _ => format!("{} {}", local.day(), local.format("%b")),
    };
    match (all_day, days) {
        (true, _) => day,
        (false, 0) => clock(dt, use_24h),
        (false, _) => format!("{day} {}", clock(dt, use_24h)),
    }
}

/// "now", "in 5 min", "in 2 h 10 min", "in 3 days".
pub fn countdown(start: DateTime<Utc>, now: DateTime<Utc>) -> String {
    // Round up so "in 1 min" shows until the event actually starts.
    let minutes = ((start - now).num_seconds() + 59).div_euclid(60);
    match minutes {
        m if m <= 0 => "now".to_string(),
        1 => "in 1 min".to_string(),
        m if m < 60 => format!("in {m} min"),
        m if m < 24 * 60 && m % 60 == 0 => format!("in {} h", m / 60),
        m if m < 24 * 60 => format!("in {} h {} min", m / 60, m % 60),
        m => {
            let days = m / (24 * 60);
            if days == 1 {
                "tomorrow".to_string()
            } else {
                format!("in {days} days")
            }
        }
    }
}

pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn countdown_rounds_up_to_the_minute() {
        let now = Utc::now();
        assert_eq!(countdown(now - Duration::minutes(3), now), "now");
        assert_eq!(countdown(now + Duration::seconds(30), now), "in 1 min");
        assert_eq!(countdown(now + Duration::minutes(12), now), "in 12 min");
        assert_eq!(countdown(now + Duration::minutes(120), now), "in 2 h");
        assert_eq!(countdown(now + Duration::minutes(135), now), "in 2 h 15 min");
    }

    #[test]
    fn truncate_counts_chars() {
        assert_eq!(truncate("Standup", 10), "Standup");
        assert_eq!(truncate("Quarterly planning", 10), "Quarterly…");
    }
}
