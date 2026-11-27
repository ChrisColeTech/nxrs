//! Dates as the library file has them: local time, "2026-11-20T15:10:03", "" when unset.

use chrono::{DateTime, Local, NaiveDateTime, Timelike};

pub fn parse(text: &str) -> Option<NaiveDateTime> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
    ] {
        if let Ok(d) = NaiveDateTime::parse_from_str(t, format) {
            return Some(d);
        }
    }
    DateTime::parse_from_rfc3339(t)
        .ok()
        .map(|d| d.with_timezone(&Local).naive_local())
}

pub fn format(date: &Option<NaiveDateTime>) -> String {
    date.map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string())
        .unwrap_or_default()
}

/// The list's Last played column: "2026-11-20 15:10", "—" when never.
pub fn list(date: &Option<NaiveDateTime>) -> String {
    date.map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "—".into())
}

/// A file time (ms since 1970) as the Play and Build dialogs say it: "26 Nov 2026, 9:14 PM".
pub fn spoken(ms: u128) -> String {
    i64::try_from(ms)
        .ok()
        .and_then(DateTime::from_timestamp_millis)
        .map(|d| {
            d.with_timezone(&Local)
                .format("%-d %b %Y, %-I:%M %p")
                .to_string()
        })
        .unwrap_or_default()
}

/// Now, to the second.
pub fn now() -> NaiveDateTime {
    let n = Local::now().naive_local();
    n.with_nanosecond(0).unwrap_or(n)
}

/// "20261120-151003" for a backup folder's name.
pub fn stamp() -> String {
    Local::now().format("%Y%m%d-%H%M%S").to_string()
}
