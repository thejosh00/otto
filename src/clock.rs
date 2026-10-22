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

/// A time on the local clock a run's period wakes are due at (`otto period 7 1d --at 3:00`), so a
/// heavy daily run can be put where nobody is using the machine. Stored as the wall-clock time
/// rather than an instant, so it means "3 AM here" whatever the offset is that day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeOfDay(time::Time);

impl TimeOfDay {
    /// `3:00`, `03:00`, `22:30`, `3am`, `10:30pm`. `None` for anything else, so each caller can
    /// say in its own words what it expected.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        let (clock, half) = match text.strip_suffix("am").or_else(|| text.strip_suffix("a.m.")) {
            Some(rest) => (rest.trim_end(), Some(false)),
            None => match text.strip_suffix("pm").or_else(|| text.strip_suffix("p.m.")) {
                Some(rest) => (rest.trim_end(), Some(true)),
                None => (text.as_str(), None),
            },
        };
        let (h, m) = match clock.split_once(':') {
            Some((h, m)) if m.len() == 2 => (h, m),
            Some(_) => return None,
            None if half.is_some() => (clock, "00"),
            None => return None,
        };
        if h.is_empty() || h.len() > 2 || !h.bytes().all(|b| b.is_ascii_digit()) || !m.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let (mut hour, minute): (u8, u8) = (h.parse().ok()?, m.parse().ok()?);
        if let Some(pm) = half {
            if !(1..=12).contains(&hour) {
                return None;
            }
            hour = match (hour, pm) {
                (12, false) => 0,
                (12, true) => 12,
                (h, true) => h + 12,
                (h, false) => h,
            };
        }
        time::Time::from_hms(hour, minute, 0).ok().map(TimeOfDay)
    }

    /// Where a period wake due at `due` lands: on the first occurrence of this time on the local
    /// clock no more than eighteen hours before `due`. For a daily run that's the next one at least
    /// six hours after the wake it is anchored on — so a wake poke started a few minutes late, or a
    /// catch-up in the afternoon, is followed by that night's, while one run by hand late in the
    /// evening isn't followed by another a few hours later. A period of days keeps its days.
    pub fn snap(self, due: OffsetDateTime, offset: UtcOffset) -> OffsetDateTime {
        let earliest = due - time::Duration::hours(18);
        let mut at = earliest.to_offset(offset).date().with_time(self.0).assume_offset(offset);
        while at < earliest {
            at += time::Duration::days(1);
        }
        at.to_offset(UtcOffset::UTC)
    }

    /// `3:00 AM`, as the plain-words lines and the web page write a time.
    pub fn plain(self) -> String {
        let (hour, minute) = (self.0.hour(), self.0.minute());
        let (h12, half) = match hour {
            0 => (12, "AM"),
            1..=11 => (hour, "AM"),
            12 => (12, "PM"),
            _ => (hour - 12, "PM"),
        };
        format!("{h12}:{minute:02} {half}")
    }
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}", self.0.hour(), self.0.minute())
    }
}

impl Serialize for TimeOfDay {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        TimeOfDay::parse(&text).ok_or_else(|| serde::de::Error::custom(format!("not a time of day: {text:?}")))
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

/// A time as the plain-words lines say it — `7:25 AM`, `tomorrow 9:00 AM`, `Sat 9:00 AM` within the
/// week, `Oct 19 9:00 AM` past it — the way the web page writes every other time, so a line from
/// here reads the same as the page around it. `local_clock` stays for the terminal's tables.
pub fn plain_time(at: Timestamp) -> String {
    plain_time_from(at.dt().to_offset(local_offset()), now().to_offset(local_offset()))
}

fn plain_time_from(local: OffsetDateTime, today: OffsetDateTime) -> String {
    let (hour, minute) = (local.hour(), local.minute());
    let (h12, half) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    let time = format!("{h12}:{minute:02} {half}");
    let days = (local.date() - today.date()).whole_days();
    match days {
        0 => time,
        1 => format!("tomorrow {time}"),
        -5..=5 => format!("{} {time}", &local.weekday().to_string()[..3]),
        _ => format!("{} {} {time}", &local.month().to_string()[..3], local.day()),
    }
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
    fn a_time_of_day_parses_the_ways_a_person_writes_one() {
        let t = |s: &str| TimeOfDay::parse(s).map(|t| t.to_string());
        assert_eq!(t("3:00").as_deref(), Some("03:00"));
        assert_eq!(t("03:00").as_deref(), Some("03:00"));
        assert_eq!(t("22:30").as_deref(), Some("22:30"));
        assert_eq!(t("3am").as_deref(), Some("03:00"));
        assert_eq!(t("10:30 PM").as_deref(), Some("22:30"));
        assert_eq!(t("12am").as_deref(), Some("00:00"));
        assert_eq!(t("12pm").as_deref(), Some("12:00"));
        for bad in ["", "3", "25:00", "3:7", "13pm", "0am", "noon", "3:00:00", "-1:00"] {
            assert_eq!(t(bad), None, "{bad:?}");
        }
        assert_eq!(TimeOfDay::parse("15:05").unwrap().plain(), "3:05 PM");
    }

    #[test]
    fn a_time_of_day_lands_on_the_next_occurrence_at_least_six_hours_after_the_last_wake() {
        let three = TimeOfDay::parse("3:00").unwrap();
        let utc = UtcOffset::UTC;
        let day = time::Duration::days(1);
        // A daily wake poke started a few minutes late lands back on 3:00, not 3:05.
        assert_eq!(three.snap(datetime!(2026-10-22 03:05 UTC) + day, utc), datetime!(2026-10-23 03:00 UTC));
        // A catch-up wake in the afternoon is followed by that night's.
        assert_eq!(three.snap(datetime!(2026-10-22 15:00 UTC) + day, utc), datetime!(2026-10-23 03:00 UTC));
        assert_eq!(three.snap(datetime!(2026-10-22 21:00 UTC) + day, utc), datetime!(2026-10-23 03:00 UTC));
        // One run by hand late in the evening isn't followed by another five hours on.
        assert_eq!(three.snap(datetime!(2026-10-22 22:00 UTC) + day, utc), datetime!(2026-10-24 03:00 UTC));
        // Every two days stays every two days.
        assert_eq!(three.snap(datetime!(2026-10-22 03:05 UTC) + day * 2, utc), datetime!(2026-10-24 03:00 UTC));
        // Local, not UTC: 3 AM at UTC-05:00 is 08:00Z.
        let cdt = UtcOffset::from_hms(-5, 0, 0).unwrap();
        assert_eq!(three.snap(datetime!(2026-10-22 20:57 UTC) + day, cdt), datetime!(2026-10-23 08:00 UTC));
    }

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
    fn plain_time_reads_like_the_page() {
        let now = datetime!(2026-10-16 06:30:00 UTC);
        assert_eq!(plain_time_from(datetime!(2026-10-16 07:25:00 UTC), now), "7:25 AM");
        assert_eq!(plain_time_from(datetime!(2026-10-16 00:05:00 UTC), now), "12:05 AM");
        assert_eq!(plain_time_from(datetime!(2026-10-17 23:51:00 UTC), now), "tomorrow 11:51 PM");
        assert_eq!(plain_time_from(datetime!(2026-10-19 09:00:00 UTC), now), "Mon 9:00 AM");
        assert_eq!(plain_time_from(datetime!(2026-11-02 12:00:00 UTC), now), "Nov 2 12:00 PM");
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
