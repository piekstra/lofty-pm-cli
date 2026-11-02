//! Non-secret settings. Everything has a sane default; the file may not exist.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const KNOWN_KEYS: &[&str] = &[
    "gro_bin",
    "grw_bin",
    "lofty_bin",
    "gro_ref",
    "scan_query",
    "label_processed",
    "reminder_hours",
    "high_stakes_usd",
    "mode.<property>",
];

/// A property's governance mode: `Recommend` (the agent proposes, the human
/// decides — the default) or `Auto` (routine-tier votes get pre-approved
/// into the notification; still gated on `verify` + `--acknowledge-risk`
/// before it can be set — see `main.rs`'s `set_key`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Recommend,
    Auto,
}

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct Config {
    pub gro_bin: Option<String>,
    pub grw_bin: Option<String>,
    pub lofty_bin: Option<String>,
    pub gro_ref: Option<String>,
    pub scan_query: Option<String>,
    pub label_processed: Option<bool>,
    pub reminder_hours: Option<String>,
    pub high_stakes_usd: Option<String>,
    pub modes: BTreeMap<String, String>,
}

impl Config {
    pub fn gro_bin(&self) -> &str {
        self.gro_bin.as_deref().unwrap_or("gro")
    }
    pub fn grw_bin(&self) -> &str {
        self.grw_bin.as_deref().unwrap_or("grw")
    }
    pub fn lofty_bin(&self) -> &str {
        self.lofty_bin.as_deref().unwrap_or("lofty")
    }
    pub fn gro_ref(&self) -> Option<&str> {
        self.gro_ref.as_deref()
    }
    pub fn scan_query(&self) -> &str {
        self.scan_query
            .as_deref()
            .unwrap_or("from:lofty.ai newer_than:60d")
    }
    pub fn label_processed(&self) -> bool {
        self.label_processed.unwrap_or(false)
    }
    #[allow(dead_code)] // consumed from Task 10 on (reminder sweep)
    pub fn reminder_hours(&self) -> Vec<i64> {
        self.reminder_hours
            .as_deref()
            .unwrap_or("48,12")
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect()
    }
    pub fn high_stakes_usd(&self) -> f64 {
        self.high_stakes_usd
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5000.0)
    }
    /// Resolve a property's configured mode, defaulting to `Recommend` when
    /// unset. Used by `main.rs`'s `config set mode.<property> auto` gating
    /// to confirm a value actually landed as `Auto` before printing the
    /// at-enable acknowledgment.
    pub fn mode_for(&self, property: &str) -> Mode {
        match self.modes.get(property).map(String::as_str) {
            Some("auto") => Mode::Auto,
            _ => Mode::Recommend,
        }
    }
}
