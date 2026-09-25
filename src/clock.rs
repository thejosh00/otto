//! Time helpers

use crate::error::OttoError;
use serde::{Deserialize, Serialize};
use std::fmt;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset};

/// UTC, second precision, Z-suffixed — sorts lexicographically.
pub fn now_iso() -> String {
    format_iso(now())
}

/// The current instant, truncated to second precision (matches what `now_iso` prints).
pub fn now() -> OffsetDateTime {
    truncate_to_seconds(OffsetDateTime::now_utc())
}

pub fn truncate_to_seconds(dt: OffsetDateTime) -> OffsetDateTime {
    dt.replace_nanosecond(0).expect("0 is always a valid nanosecond value")
}

pub fn format_iso(dt: OffsetDateTime) -> String {
    let dt = truncate_to_seconds(dt.to_offset(UtcOffset::UTC));
    // `time`'s Rfc3339 formatter already writes a bare `Z` for the UTC offset.
    dt.format(&Rfc3339).expect("a UTC OffsetDateTime always formats as RFC 3339")
}

/// Parses an ISO-8601 timestamp. A value with no timezone offset is treated as UTC —
/// mirroring the Python's `dt if dt.tzinfo else dt.replace(tzinfo=utc)`.
pub fn parse_iso(value: &str) -> Result<OffsetDateTime, OttoError> {
    let text = value.trim();
    if let Ok(dt) = OffsetDateTime::parse(text, &Rfc3339) {
        return Ok(dt.to_offset(UtcOffset::UTC));
    }
    let naive_fmt = format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");
    if let Ok(naive) = PrimitiveDateTime::parse(text, &naive_fmt) {
        return Ok(naive.assume_utc());
    }
    Err(OttoError::usage(format!("not a valid ISO-8601 timestamp: {value:?}")))
}

/// An ISO-8601 instant that is validated at the boundary — deserializing `run.json` — rather
/// than wherever it happens to be read.
///
/// Every timestamp on `RunState` used to be a bare `String`, parsed with `clock::parse_iso` at
/// each of a dozen call sites, each free to handle a parse failure however it liked. The
/// dangerous case was silent: `elapsed_hours` treated an unparseable `createdAt` as `0.0`,
/// which quietly disabled the hours budget rather than erroring. Wrapping the field means a
/// malformed timestamp fails to deserialize `RunState` at all — the same stance already taken
/// for `schemaVersion` — so the failure is loud and at read time, not a wrong answer three
/// calculations later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(OffsetDateTime);

impl Timestamp {
    pub fn now() -> Self {
        Timestamp(now())
    }

    /// From an already-computed instant — a wake time anchored on something other than now, or
    /// a test building a timestamp at an arbitrary offset from a fixed clock, where
    /// `in_minutes`/`in_seconds` (relative to the real `now()`) would not do.
    pub fn at(dt: OffsetDateTime) -> Self {
        Timestamp(truncate_to_seconds(dt))
    }

    pub fn parse(text: &str) -> Result<Self, OttoError> {
        parse_iso(text).map(Timestamp)
    }

    /// From a relative offset, e.g. an `--in <seconds>` or `--expires-in <seconds>` flag.
    pub fn in_seconds(seconds: i64) -> Self {
        Timestamp(now() + time::Duration::seconds(seconds))
    }

    /// From `--in <minutes>`, e.g. a backoff or a policy-driven deadline.
    pub fn in_minutes(minutes: i64) -> Self {
        Timestamp(now() + time::Duration::minutes(minutes))
    }

    pub fn dt(self) -> OffsetDateTime {
        self.0
    }

    pub fn is_past(self) -> bool {
        self.0 <= now()
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", format_iso(self.0))
    }
}

impl Serialize for Timestamp {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format_iso(self.0))
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse_iso(&text).map(Timestamp).map_err(serde::de::Error::custom)
    }
}

/// An age or period written the way a person would: `45m`, `2h`, `3d`. `None` for anything else,
/// so each caller can say in its own words what it expected.
pub fn parse_age(text: &str) -> Option<time::Duration> {
    let text = text.trim();
    let (i, unit) = text.char_indices().last()?;
    let n = text[..i].parse::<i64>().ok()?;
    match unit {
        'm' => Some(time::Duration::minutes(n)),
        'h' => Some(time::Duration::hours(n)),
        'd' => Some(time::Duration::days(n)),
        _ => None,
    }
}

/// The inverse of `parse_age` for whole minutes: the largest unit that divides evenly.
pub fn format_minutes(minutes: u64) -> String {
    if minutes > 0 && minutes % (24 * 60) == 0 {
        format!("{}d", minutes / (24 * 60))
    } else if minutes > 0 && minutes % 60 == 0 {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}

/// The machine's UTC offset, for showing a person times in their own clock. Falls back to UTC
/// when the platform will not say — `time` refuses on Unix once a process has more than one
/// thread, which is why this is cached on first use and `otto logs` asks before it does anything
/// else. Everything otto *stores* stays UTC; this is display only.
pub fn local_offset() -> UtcOffset {
    static OFFSET: std::sync::OnceLock<UtcOffset> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC))
}

/// `UTC`, or `UTC-07:00`: how an offset is named in a header.
pub fn offset_label(offset: UtcOffset) -> String {
    if offset.is_utc() {
        return "UTC".to_string();
    }
    let (h, m, _) = offset.as_hms();
    format!("UTC{h:+03}:{:02}", m.abs())
}

/// "in 24m" / "12m ago", relative to now. Absolute timestamps make you do arithmetic to answer
/// "is it late?" — used by `otto ls`/`otto show`'s timers and by `otto agent status`'s last-poke
/// time.
pub fn relative(at: Timestamp) -> String {
    let delta = at.dt() - now();
    let minutes = delta.whole_minutes();
    match minutes {
        m if m > 1440 => format!("in {}d", m / 1440),
        m if m > 60 => format!("in {}h{}m", m / 60, m % 60),
        m if m > 0 => format!("in {m}m"),
        0 => "now".to_string(),
        m if m > -60 => format!("{}m ago", -m),
        m if m > -1440 => format!("{}h ago", -m / 60),
        m => format!("{}d ago", -m / 1440),
    }
}

/// When something poke acts on is due: `relative` while it is still ahead, and "next poke" once it
/// has passed — an overdue timer is not late, it is waiting for poke's next pass, and "3m ago"
/// reads as though something was missed.
pub fn due(at: Timestamp) -> String {
    if at.is_past() {
        "next poke".to_string()
    } else {
        relative(at)
    }
}

/// A wake time as a person reads a clock: `14:05` today, `Sep 26 14:05` any other day, in
/// local time. Display only — everything stored stays UTC.
pub fn local_clock(at: Timestamp) -> String {
    let offset = local_offset();
    let local = at.dt().to_offset(offset);
    let same_day = local.date() == now().to_offset(offset).date();
    let fmt = if same_day {
        format_description!("[hour]:[minute]")
    } else {
        format_description!("[month repr:short] [day padding:none] [hour]:[minute]")
    };
    local.format(fmt).unwrap_or_else(|_| at.to_string())
}

/// Lowercase, collapse runs of non-`[a-z0-9]` to a single hyphen, trim leading/trailing
/// hyphens. Falls back to `fallback` if the result would be empty.
pub fn slugify(text: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut pending_sep = false;
    for ch in text.trim().chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('-');
            }
            pending_sep = false;
            out.push(lower);
        } else {
            pending_sep = true;
        }
    }
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn ages_parse_and_minutes_format_in_the_largest_even_unit() {
        assert_eq!(parse_age("45m"), Some(time::Duration::minutes(45)));
        assert_eq!(parse_age("2h"), Some(time::Duration::hours(2)));
        assert_eq!(parse_age("3d"), Some(time::Duration::days(3)));
        assert_eq!(parse_age("hourly"), None);
        assert_eq!(parse_age(""), None);
        assert_eq!(format_minutes(60), "1h");
        assert_eq!(format_minutes(90), "90m");
        assert_eq!(format_minutes(1440), "1d");
    }

    #[test]
    fn slugify_collapses_and_trims() {
        assert_eq!(slugify("  Hello, World!! ", "run"), "hello-world");
        assert_eq!(slugify("NR-1234", "run"), "nr-1234");
        assert_eq!(slugify("---", "run"), "run");
        assert_eq!(slugify("", "run"), "run");
    }

    #[test]
    fn iso_round_trip() {
        let dt = datetime!(2026-09-11 12:34:56 UTC);
        let text = format_iso(dt);
        assert_eq!(text, "2026-09-11T12:34:56Z");
        assert_eq!(parse_iso(&text).unwrap(), dt);
    }

    #[test]
    fn parse_iso_without_offset_assumes_utc() {
        let dt = parse_iso("2026-09-11T12:34:56").unwrap();
        assert_eq!(dt, datetime!(2026-09-11 12:34:56 UTC));
    }

    #[test]
    fn parse_iso_rejects_garbage() {
        assert!(parse_iso("not-a-date").is_err());
    }

    /// Deliberately tolerant of one minute either way. `whole_minutes()` truncates, so a second
    /// ticking between building the timestamp and formatting it turns 30 into 29 — which made an
    /// exact assertion here fail about one run in a hundred.
    #[test]
    fn local_clock_drops_the_date_only_for_today() {
        let soon = Timestamp::in_minutes(1);
        let shown = local_clock(soon);
        // Whether `soon` is still today depends on when the test runs; either form is right.
        assert!(shown.len() == 5 || shown.contains(' '), "got {shown}");
        let later = Timestamp::in_minutes(3 * 1440);
        assert!(local_clock(later).contains(' '), "another day names the day: {}", local_clock(later));
    }

    #[test]
    fn relative_times_read_forwards_and_backwards() {
        let future = Timestamp::in_minutes(30);
        assert!(["in 30m", "in 29m"].contains(&relative(future).as_str()), "got {}", relative(future));
        let past = Timestamp::in_minutes(-30);
        assert!(["30m ago", "29m ago"].contains(&relative(past).as_str()), "got {}", relative(past));
    }

    #[test]
    fn an_overdue_timer_waits_for_the_next_poke_rather_than_reading_as_missed() {
        assert_eq!(due(Timestamp::in_minutes(-3)), "next poke");
        assert_eq!(due(Timestamp::in_minutes(-3000)), "next poke");
        assert!(due(Timestamp::in_minutes(30)).starts_with("in "), "a future time still reads as one");
    }
}
