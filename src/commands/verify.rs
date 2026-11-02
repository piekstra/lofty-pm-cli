//! `verify` — ToS/compliance verification records that double-gate auto
//! mode (see `main.rs`'s `config set mode.<property> auto`). The CLI never
//! checks these against Lofty or a lawyer itself: `record` takes the
//! owner's word for it and just stamps when + an optional note.

use std::collections::BTreeMap;

use pk_cli_core::CliError;
use serde_json::json;

use crate::commands::{emit, Ctx};
use crate::model::{Actor, LedgerEntry};
use crate::store::Verification;

/// The three ToS/compliance items that must all be recorded before
/// `config set mode.<property> auto` will unlock. Order here is the order
/// `verify list` reports rows in.
pub const VERIFICATION_ITEMS: &[&str] = &["operating-agreement", "api-terms", "vote-attestation"];

/// `lofty-pm verify <list|record>`.
#[derive(clap::Subcommand, Debug)]
pub enum Cmd {
    /// Show each verification item's recorded state, and whether auto mode
    /// is unlocked (all three recorded).
    List,
    /// Record that `item` has been reviewed, stamping the current time.
    Record {
        /// One of VERIFICATION_ITEMS.
        item: String,
        /// Free-text context for the record (e.g. who reviewed it, or a
        /// link to the reviewed document).
        #[arg(long)]
        note: Option<String>,
    },
}

pub fn run(ctx: &Ctx, cmd: &Cmd) -> Result<(), CliError> {
    match cmd {
        Cmd::List => list(ctx),
        Cmd::Record { item, note } => record(ctx, item, note.as_deref()),
    }
}

/// All three items recorded: the precondition `config set mode.<property>
/// auto` checks before its own `--acknowledge-risk` gate.
pub fn all_recorded(verifications: &BTreeMap<String, Verification>) -> bool {
    VERIFICATION_ITEMS
        .iter()
        .all(|item| verifications.contains_key(*item))
}

fn list(ctx: &Ctx) -> Result<(), CliError> {
    let verifications = ctx.store.load_verifications()?;
    let items: Vec<_> = VERIFICATION_ITEMS
        .iter()
        .map(|item| match verifications.get(*item) {
            Some(v) => json!({
                "item": item,
                "recorded": true,
                "recorded_at": v.recorded_at,
                "note": v.note,
            }),
            None => json!({
                "item": item,
                "recorded": false,
                "recorded_at": null,
                "note": null,
            }),
        })
        .collect();
    let auto_mode_unlocked = all_recorded(&verifications);

    let payload = json!({ "items": items, "auto_mode_unlocked": auto_mode_unlocked });
    emit(ctx.common, "verifications", payload, |v| {
        for item in v["items"].as_array().expect("array") {
            let mark = if item["recorded"].as_bool().unwrap_or(false) {
                "x"
            } else {
                " "
            };
            let at = item["recorded_at"]
                .as_str()
                .map(|a| format!(" ({a})"))
                .unwrap_or_default();
            let note = item["note"]
                .as_str()
                .map(|n| format!(" — {n}"))
                .unwrap_or_default();
            println!(
                "[{mark}] {}{at}{note}",
                item["item"].as_str().unwrap_or_default(),
            );
        }
        println!(
            "auto mode unlocked: {}",
            v["auto_mode_unlocked"].as_bool().unwrap_or(false)
        );
    });
    Ok(())
}

fn record(ctx: &Ctx, item: &str, note: Option<&str>) -> Result<(), CliError> {
    if !VERIFICATION_ITEMS.contains(&item) {
        return Err(CliError::Usage(format!(
            "unknown verification item `{item}` (known: {})",
            VERIFICATION_ITEMS.join(", ")
        )));
    }

    let mut verifications = ctx.store.load_verifications()?;
    verifications.insert(
        item.to_string(),
        Verification {
            recorded_at: crate::clock::now(),
            note: note.map(str::to_string),
        },
    );
    ctx.store.save_verifications(&verifications)?;
    ctx.store.append_ledger(&LedgerEntry::event(
        None,
        Actor::Human,
        "verification-recorded",
        Some(item.to_string()),
    ))?;

    let payload = json!({ "item": item, "note": note });
    emit(ctx.common, "verification-recorded", payload, |v| {
        println!("recorded: {}", v["item"].as_str().unwrap_or_default());
    });
    Ok(())
}
