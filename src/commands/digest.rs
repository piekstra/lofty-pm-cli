//! `digest` — corpus items (updates/results) grouped by property, most
//! recently updated property first, items newest first within a property.
//! Pure read: never touches the store's write path.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use pk_cli_core::CliError;
use serde_json::json;

use crate::commands::{emit, Ctx};
use crate::store::CorpusItem;

/// `lofty-pm digest [--since YYYY-MM-DD]`.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// ISO calendar date (YYYY-MM-DD). Items strictly before this date's
    /// 00:00:00 UTC are excluded.
    #[arg(long)]
    pub since: Option<String>,
}

/// Parse `--since` as an ISO calendar date, anchored to that date's
/// 00:00:00 UTC. Garbage input is a `Usage` error (exit 2), never a panic.
fn parse_since(s: &str) -> Result<DateTime<Utc>, CliError> {
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| CliError::Usage(format!("--since must be YYYY-MM-DD, got `{s}`: {e}")))?;
    let naive = date
        .and_hms_opt(0, 0, 0)
        .expect("00:00:00 is always a valid time for any calendar date");
    Ok(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
}

/// First 300 *characters* of `body` — never a byte slice, so a multibyte
/// codepoint straddling the cut point can't split/panic.
fn excerpt(body: &str) -> String {
    body.chars().take(300).collect()
}

pub fn run(ctx: &Ctx, args: &Args) -> Result<(), CliError> {
    let cutoff = args.since.as_deref().map(parse_since).transpose()?;

    let mut items = ctx.store.list_corpus(None)?;
    if let Some(cutoff) = cutoff {
        items.retain(|item| item.date >= cutoff);
    }

    let mut by_property: BTreeMap<String, Vec<&CorpusItem>> = BTreeMap::new();
    for item in &items {
        by_property
            .entry(item.property.clone())
            .or_default()
            .push(item);
    }

    // (most-recent-item date, rendered group) pairs, so the final sort by
    // "most recent item desc" doesn't need to re-inspect the JSON.
    let mut groups: Vec<(DateTime<Utc>, String, serde_json::Value)> = by_property
        .into_iter()
        .map(|(property, mut group_items)| {
            group_items.sort_by_key(|i| std::cmp::Reverse(i.date));
            let most_recent = group_items[0].date;
            let items_json: Vec<serde_json::Value> = group_items
                .iter()
                .map(|i| {
                    json!({
                        "date": i.date,
                        "kind": i.kind,
                        "subject": i.subject,
                        "excerpt": excerpt(&i.body),
                    })
                })
                .collect();
            (
                most_recent,
                property.clone(),
                json!({ "property": property, "items": items_json }),
            )
        })
        .collect();
    // Properties sorted by most recent item desc; ties broken by property
    // name so output ordering is deterministic.
    groups.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let properties: Vec<serde_json::Value> = groups.into_iter().map(|(_, _, v)| v).collect();

    let payload = json!({
        "since": args.since,
        "properties": properties,
    });
    emit(ctx.common, "digest", payload, |v| {
        let properties = v["properties"].as_array().expect("array");
        if properties.is_empty() {
            println!("no corpus items");
        }
        for group in properties {
            let items = group["items"].as_array().map_or(0, Vec::len);
            println!(
                "{}: {} item(s)",
                group["property"].as_str().unwrap_or_default(),
                items,
            );
            for item in group["items"].as_array().into_iter().flatten() {
                println!(
                    "  {} [{}] {}",
                    item["date"].as_str().unwrap_or_default(),
                    item["kind"].as_str().unwrap_or_default(),
                    item["subject"].as_str().unwrap_or_default(),
                );
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_since_rejects_garbage_and_accepts_iso_dates() {
        assert!(parse_since("not-a-date").is_err());
        assert!(matches!(parse_since("not-a-date"), Err(CliError::Usage(_))));
        assert!(parse_since("2026-07-24").is_ok());
    }

    #[test]
    fn excerpt_is_char_safe_at_300_with_multibyte_input() {
        // 400 three-byte codepoints: a byte-slice at [..300] would land
        // mid-codepoint and panic (or corrupt UTF-8). Char-based
        // truncation must not.
        let body: String = "€".repeat(400);
        let e = excerpt(&body);
        assert_eq!(e.chars().count(), 300);
        assert!(e.is_char_boundary(e.len()));
    }

    #[test]
    fn excerpt_passes_through_short_bodies_unchanged() {
        assert_eq!(excerpt("short body"), "short body");
    }
}
