//! Deterministic time: tests set LOFTY_PM_NOW (RFC 3339).

use chrono::{DateTime, Utc};

pub fn now() -> DateTime<Utc> {
    match std::env::var("LOFTY_PM_NOW") {
        Ok(s) => s
            .parse()
            .expect("LOFTY_PM_NOW must be RFC 3339, e.g. 2026-07-24T12:00:00Z"),
        Err(_) => Utc::now(),
    }
}
