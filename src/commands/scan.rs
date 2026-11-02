//! `scan` — sweep Gmail for governance votes, updates, and results, filing
//! them into the store. Idempotent: message ids already recorded in
//! `processed.json` are skipped on rerun.

use pk_cli_core::CliError;
use serde_json::json;

use crate::commands::{emit, Ctx};
use crate::deadline::sweep_deadlines;
use crate::gmail::Gmail;
use crate::model::{Actor, LedgerEntry, VoteRecord, VoteState};
use crate::parse::{classify, default_tier, extract_amount, Classified};
use crate::store::CorpusItem;

/// `lofty-pm scan [--query Q] [--max N]`.
#[derive(clap::Args, Debug)]
pub struct Args {
    /// Override the configured Gmail search query.
    #[arg(long)]
    pub query: Option<String>,
    /// Cap the number of message ids processed this run (default: all
    /// returned by `gmail.search_ids`, itself capped at 100).
    #[arg(long)]
    pub max: Option<usize>,
}

pub fn run(ctx: &Ctx, args: &Args) -> Result<(), CliError> {
    // Deadline sweep first, so state is fresh before mail processing too.
    let missed = sweep_deadlines(ctx.store)?;

    let gmail = Gmail::new(ctx.cfg);
    let query = args
        .query
        .as_deref()
        .unwrap_or_else(|| ctx.cfg.scan_query());
    let mut ids = gmail.search_ids(query)?;
    if let Some(max) = args.max {
        ids.truncate(max);
    }

    let mut scanned: u64 = 0;
    let mut skipped: u64 = 0;
    let mut new_votes: Vec<String> = Vec::new();
    let mut new_updates: u64 = 0;
    let mut closed: Vec<String> = Vec::new();
    let mut manual_parse: Vec<String> = Vec::new();

    for id in &ids {
        scanned += 1;
        if ctx.store.is_processed(id)? {
            skipped += 1;
            continue;
        }

        let raw = match gmail.read(id) {
            Ok(raw) => raw,
            // Auth is global — no point continuing the batch if the gro
            // token is dead; propagate so the caller sees exit 3.
            Err(e @ CliError::Auth(_)) => return Err(e),
            // Any other read/parse failure (e.g. a message shape the
            // parser doesn't recognize) is local to this one id: note it,
            // leave it for manual review, and — critically — do NOT
            // mark_processed, so a transient failure gets retried next
            // scan instead of permanently blocking every id after it.
            Err(e) => {
                if !ctx.common.quiet {
                    eprintln!("warning: could not read {id}, leaving for manual review: {e}");
                }
                manual_parse.push(id.clone());
                continue;
            }
        };
        match classify(&raw) {
            Classified::Vote(v) => {
                let vote_id = format!("v-{id}");
                let amount = extract_amount(&v.proposal);
                let tier = default_tier(&v.proposal, amount, ctx.cfg.high_stakes_usd());
                let needs_manual = v.property_slug.is_empty() || v.proposal.is_empty();

                let mut record = VoteRecord::new(
                    vote_id.clone(),
                    v.property_slug.clone(),
                    raw.subject.clone(),
                    v.proposal.clone(),
                    v.options.clone(),
                );
                record.deadline = v.deadline;
                record.vote_link = v.vote_link.clone();
                record.tier = Some(tier);
                record.raw = Some(raw.body.clone());
                record.message_ids.push(id.clone());
                if needs_manual {
                    record = record.with_state(VoteState::NeedsManualParse);
                }

                ctx.store.save_vote(&record)?;
                ctx.store.append_ledger(&LedgerEntry::event(
                    Some(vote_id.clone()),
                    Actor::Scan,
                    "detected",
                    None,
                ))?;
                ctx.store.mark_processed(id, "vote")?;

                new_votes.push(vote_id.clone());
                if needs_manual {
                    manual_parse.push(vote_id);
                }
            }
            Classified::Update(u) => {
                let item = CorpusItem::new(
                    u.property_slug.clone(),
                    id.clone(),
                    raw.subject.clone(),
                    "update",
                    raw.body.clone(),
                );
                ctx.store.save_corpus_item(&item)?;
                ctx.store.mark_processed(id, "update")?;
                new_updates += 1;
            }
            Classified::VoteResult(r) => {
                let mut candidates: Vec<VoteRecord> = ctx
                    .store
                    .list_votes()?
                    .into_iter()
                    .filter(|v| v.property == r.property_slug && v.state != VoteState::Closed)
                    .collect();

                if candidates.len() == 1 {
                    let mut record = candidates.pop().expect("checked len == 1 above");
                    let (from, to) = record.advance(VoteState::Closed, Actor::Scan)?;
                    record.outcome = Some(r.outcome.clone());
                    record.message_ids.push(id.clone());
                    ctx.store.save_vote(&record)?;
                    ctx.store.append_ledger(&LedgerEntry::transition(
                        &record.id,
                        Actor::Scan,
                        from,
                        to,
                        Some(r.outcome.clone()),
                    ))?;
                    ctx.store.mark_processed(id, "result")?;
                    closed.push(record.id.clone());
                } else {
                    // Zero or many open candidates for the property: can't
                    // safely auto-close, so leave it for the agent/human.
                    let item = CorpusItem::new(
                        r.property_slug.clone(),
                        id.clone(),
                        raw.subject.clone(),
                        "result",
                        raw.body.clone(),
                    );
                    ctx.store.save_corpus_item(&item)?;
                    ctx.store.mark_processed(id, "manual")?;
                    manual_parse.push(id.clone());
                }
            }
            Classified::Unrelated => {
                ctx.store.mark_processed(id, "unrelated")?;
            }
        }

        if ctx.cfg.label_processed() {
            label_processed(ctx, id);
        }
    }

    let report = json!({
        "scanned": scanned,
        "new_votes": new_votes,
        "new_updates": new_updates,
        "closed": closed,
        "manual_parse": manual_parse,
        "missed": missed,
        "skipped": skipped,
    });

    emit(ctx.common, "scan-report", report, |v| {
        println!(
            "scanned {}: {} new vote(s), {} update(s), {} closed, {} missed, {} skipped, {} need manual parse",
            v["scanned"],
            v["new_votes"].as_array().map_or(0, Vec::len),
            v["new_updates"],
            v["closed"].as_array().map_or(0, Vec::len),
            v["missed"].as_array().map_or(0, Vec::len),
            v["skipped"],
            v["manual_parse"].as_array().map_or(0, Vec::len),
        );
    });

    Ok(())
}

/// Best-effort `grw ... mail label <id> --label Lofty/Processed`. Failures
/// (binary missing, non-zero exit) never fail the scan — just a stderr note
/// unless `--quiet`.
fn label_processed(ctx: &Ctx, id: &str) {
    let result = std::process::Command::new(ctx.cfg.grw_bin())
        .args([
            "--no-color",
            "mail",
            "label",
            id,
            "--label",
            "Lofty/Processed",
        ])
        .output();
    match result {
        Ok(out) if out.status.success() => {}
        Ok(out) if !ctx.common.quiet => {
            eprintln!(
                "warning: labeling {id} as processed failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Err(e) if !ctx.common.quiet => {
            eprintln!("warning: labeling {id} as processed failed: {e}");
        }
        _ => {}
    }
}
