//! `ledger` — read the append-only transition/event ledger, optionally
//! filtered to one vote and/or capped to the most recent N entries. Pure
//! read: never appends.

use pk_cli_core::CliError;
use serde_json::json;

use crate::commands::{emit, Ctx};

/// `lofty-pm ledger [--vote ID] [--limit N]`.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// Only entries for this vote id.
    #[arg(long)]
    pub vote: Option<String>,
    /// Keep only the last N entries after filtering (chronological order is
    /// preserved — oldest first, newest last).
    #[arg(long)]
    pub limit: Option<usize>,
}

pub fn run(ctx: &Ctx, args: &Args) -> Result<(), CliError> {
    let mut entries = ctx.store.read_ledger()?;

    if let Some(vote_id) = &args.vote {
        entries.retain(|e| e.vote_id.as_deref() == Some(vote_id.as_str()));
    }

    // `read_ledger` already returns entries in append (chronological) order;
    // trimming to the tail after filtering keeps that order while
    // satisfying "last N".
    if let Some(limit) = args.limit {
        if entries.len() > limit {
            entries = entries.split_off(entries.len() - limit);
        }
    }

    let payload = json!({ "entries": entries });
    emit(ctx.common, "ledger", payload, |v| {
        let entries = v["entries"].as_array().expect("array");
        if entries.is_empty() {
            println!("no ledger entries");
        }
        for e in entries {
            let detail = e["detail"]
                .as_str()
                .map(|d| format!(": {d}"))
                .unwrap_or_default();
            println!(
                "{} {} {} {}{}",
                e["at"].as_str().unwrap_or_default(),
                e["actor"].as_str().unwrap_or_default(),
                e["event"].as_str().unwrap_or_default(),
                e["vote_id"].as_str().unwrap_or("-"),
                detail,
            );
        }
    });
    Ok(())
}
