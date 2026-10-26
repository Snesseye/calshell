//! `events.quickAdd` always creates 1-hour events when no length is given.
//! Calshell shortens (or lengthens) them to the configured default, but only
//! when the text clearly gave no length or end time of its own.

const UNITS: [&str; 10] = [
    "m", "min", "mins", "minute", "minutes", "h", "hr", "hrs", "hour", "hours",
];
const RANGE_WORDS: [&str; 5] = ["until", "till", "til", "through", "thru"];
const PHRASES: [&str; 4] = ["all day", "all-day", "half an hour", "an hour"];

/// True when the text names a duration ("30min", "2 hours"), a range
/// ("3-4pm", "3 to 4") or an end ("until 5").
pub fn has_explicit_length(text: &str) -> bool {
    let text = text.to_lowercase();
    if PHRASES.iter().any(|p| text.contains(p)) {
        return true;
    }
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty())
        .collect();

    words.iter().enumerate().any(|(i, word)| {
        let next = words.get(i + 1).copied();
        RANGE_WORDS.contains(word)
            || (*word == "to" && next.is_some_and(starts_with_digit))
            || is_duration(word, next)
            || is_time_range(word)
    })
}

fn starts_with_digit(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_digit())
}

fn is_duration(word: &str, next: Option<&str>) -> bool {
    let split = word
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(word.len());
    let (number, unit) = word.split_at(split);
    if number.is_empty() || !starts_with_digit(number) {
        return false;
    }
    if unit.is_empty() {
        return next.is_some_and(|n| UNITS.contains(&n));
    }
    // "1h30" / "1h30m"
    if let Some(rest) = unit.strip_prefix('h') {
        if starts_with_digit(rest) {
            return true;
        }
    }
    UNITS.contains(&unit)
}

fn is_time_range(word: &str) -> bool {
    let parts: Vec<&str> = word.split(['-', '–']).collect();
    parts.len() == 2 && parts.iter().all(|p| starts_with_digit(p))
}

#[cfg(test)]
mod tests {
    use super::has_explicit_length;

    #[test]
    fn detects_lengths_and_ranges() {
        for text in [
            "call Sam tomorrow 3pm 30min",
            "standup 9:30 15 min",
            "review 2 hours friday",
            "focus 1h30 tomorrow",
            "lunch 12-1pm",
            "workshop 10:00-13:00",
            "offsite until friday",
            "call 3 to 4",
            "conference all day thursday",
            "sync for half an hour",
        ] {
            assert!(has_explicit_length(text), "{text}");
        }
    }

    #[test]
    fn plain_times_have_no_length() {
        for text in [
            "call Sam tomorrow 3pm",
            "dentist 2026-11-03 10am",
            "talk to Sam at 4",
            "meet Anna monday 14:00",
            "dinner at 7pm",
        ] {
            assert!(!has_explicit_length(text), "{text}");
        }
    }
}
