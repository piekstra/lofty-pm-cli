//! Time. Tests set LOFTY_PM_NOW (RFC 3339) to make it deterministic.

use std::str::FromStr;
use std::sync::LazyLock;

use jiff::{civil, fmt::rfc2822, tz::TimeZone, Timestamp};
use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An instant. Written as RFC 3339 in UTC with no, three, six or nine decimals, the fewest that
/// lose nothing: the form the store and `--json` have always had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct UtcTime(Timestamp);

/// RFC 3339, and the near misses the store and `--deadline` have always taken: unpadded numbers,
/// stray spaces, `UTC` for `Z`, an offset without its colon. jiff's own parser refuses those, and
/// takes a time without seconds or an offset without minutes.
static RFC3339_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)^\s* \+?([0-9]{1,4}) \s*-\s* ([0-9]{1,2}) \s*-\s* ([0-9]{1,2}) [T\x20]
        \s* ([0-9]{1,2}) \s*:\s* ([0-9]{1,2}) \s*:\s* ([0-9]{1,2}) (?:\.([0-9]{1,9})[0-9]*)? \s*
        (?: UTC | Z | ([-+\x{2212}]) ([01][0-9]|2[0-3]) [:\s]* ([0-5][0-9]) ) \s*$",
    )
    .unwrap()
});

impl UtcTime {
    /// 00:00:00 UTC on a `YYYY-MM-DD` date.
    pub fn midnight(day: &str) -> Result<Self, jiff::Error> {
        // jiff's formats match only ASCII spaces.
        let spaced = day.replace(char::is_whitespace, " ");
        let date = civil::Date::strptime("%Y-%m-%d", &spaced)?;
        TimeZone::UTC.to_timestamp(date.at(0, 0, 0, 0)).map(Self)
    }

    pub fn from_rfc2822(text: &str) -> Option<Self> {
        let parsed = rfc2822::DateTimeParser::new().parse_timestamp(text);
        parsed.ok().map(Self)
    }

    /// What a clock in `zone` shows as `at`. `None` where that clock skips `at` or shows it twice.
    pub fn from_clock(at: civil::DateTime, zone: &TimeZone) -> Option<Self> {
        let instant = zone.to_ambiguous_timestamp(at).unambiguous();
        instant.ok().map(Self)
    }

    /// Whole seconds from `earlier` to `self`, rounded toward zero.
    pub fn seconds_since(self, earlier: Self) -> i64 {
        self.0.duration_since(earlier.0).as_secs()
    }

    pub fn rfc3339_seconds(self) -> String {
        self.0.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
    }

    /// The UTC date, `YYYY-MM-DD`.
    pub fn day(self) -> String {
        self.0.strftime("%Y-%m-%d").to_string()
    }
}

impl FromStr for UtcTime {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let c = RFC3339_RE
            .captures(text)
            .ok_or("expected the form 2026-07-24T12:00:00Z")?;
        let part = |i: usize| c.get(i).map_or("", |m| m.as_str());
        let offset = match part(8) {
            "" => "Z".to_string(),
            "+" => format!("+{}:{}", part(9), part(10)),
            _ => format!("-{}:{}", part(9), part(10)),
        };
        let decimals = format!("{:0<9}", part(7));
        format!(
            "{:0>4}-{:0>2}-{:0>2}T{:0>2}:{:0>2}:{:0>2}.{decimals}{offset}",
            part(1),
            part(2),
            part(3),
            part(4),
            part(5),
            part(6)
        )
        .parse()
        .map(Self)
        .map_err(|e: jiff::Error| e.to_string())
    }
}

impl Serialize for UtcTime {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let decimals = match self.0.subsec_nanosecond() {
            0 => 0,
            n if n % 1_000_000 == 0 => 3,
            n if n % 1_000 == 0 => 6,
            _ => 9,
        };
        serializer.collect_str(&format_args!("{:.decimals$}", self.0))
    }
}

impl<'de> Deserialize<'de> for UtcTime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

pub fn now() -> UtcTime {
    match std::env::var("LOFTY_PM_NOW") {
        Ok(s) => s
            .parse()
            .expect("LOFTY_PM_NOW must be RFC 3339, e.g. 2026-07-24T12:00:00Z"),
        Err(_) => UtcTime(Timestamp::now()),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Each row of `tests/fixtures/time.json` pairs an input with how the instant it reads as is
    /// written, or `null` where it is refused. chrono 0.4.45, used through 0.2.1, wrote the second
    /// column, so a row that stops matching is a change in behavior.
    pub fn check(kind: &str, read: impl Fn(&str) -> Option<UtcTime>) {
        let fixture = include_str!("../tests/fixtures/time.json");
        let fixture: serde_json::Value = serde_json::from_str(fixture).unwrap();
        for row in fixture[kind].as_array().unwrap() {
            let read = read(row[0].as_str().unwrap());
            assert_eq!(serde_json::to_value(read).unwrap(), row[1], "{row}");
        }
    }

    #[test]
    fn rfc3339_reads_and_writes_as_it_did_under_chrono() {
        check("rfc3339", |s| s.parse().ok());
        check("rfc3339", |s| serde_json::from_value(s.into()).ok());
    }

    #[test]
    fn rfc2822_reads_as_it_did_under_chrono() {
        check("rfc2822", UtcTime::from_rfc2822);
    }

    #[test]
    fn a_date_reads_as_its_utc_midnight() {
        check("day", |s| UtcTime::midnight(s).ok());
    }

    #[test]
    fn seconds_and_days_drop_the_rest_without_rounding() {
        let at = |s: &str| s.parse::<UtcTime>().unwrap();
        let (early, late) = (at("2026-07-24T12:00:00.9Z"), at("2026-07-25T13:00:00.1Z"));
        assert_eq!(late.seconds_since(early), 89_999);
        assert_eq!(early.seconds_since(late), -89_999);
        assert_eq!(early.rfc3339_seconds(), "2026-07-24T12:00:00Z");
        assert_eq!(at("2026-07-24T17:59:59.9-07:00").day(), "2026-07-25");
    }
}
