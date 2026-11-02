//! `pending` — the owner/agent work queue: votes waiting on the agent to
//! analyze, votes waiting on the owner to decide, and leveled deadline
//! reminders for the latter. `pending ack` silences a reminder level once
//! the agent has actually notified the owner.

use pk_cli_core::CliError;
use serde_json::json;

use crate::commands::votes::vote_payload;
use crate::commands::{emit, Ctx};
use crate::deadline::sweep_deadlines;
use crate::model::{Actor, LedgerEntry, VoteState};

/// `lofty-pm pending [ack <vote-id> --level N]`. Bare `pending` lists the
/// queue; `pending ack ...` is the one subcommand.
#[derive(clap::Args, Debug)]
pub struct Cmd {
    #[command(subcommand)]
    pub action: Option<Action>,
}

#[derive(clap::Subcommand, Debug)]
pub enum Action {
    /// Record that a reminder level has already been sent for a vote, so
    /// `pending` stops surfacing it (until/unless a smaller level applies).
    Ack {
        vote_id: String,
        /// Must be one of the configured `reminder_hours` levels.
        #[arg(long)]
        level: i64,
    },
}

pub fn run(ctx: &Ctx, cmd: &Cmd) -> Result<(), CliError> {
    match &cmd.action {
        None => list(ctx),
        Some(Action::Ack { vote_id, level }) => ack(ctx, vote_id, *level),
    }
}

/// The read path: sweep deadlines first (so `missed_now`/state are fresh),
/// then bucket every vote and compute at most one reminder per
/// awaiting-owner vote. Never writes `reminded_levels` — that's `ack`'s job
/// alone, so `pending` stays safe to poll repeatedly.
fn list(ctx: &Ctx) -> Result<(), CliError> {
    let missed_now = sweep_deadlines(ctx.store)?;
    let now = crate::clock::now();
    let levels = ctx.cfg.reminder_hours();

    let mut awaiting_agent = Vec::new();
    let mut awaiting_owner = Vec::new();
    let mut reminders = Vec::new();

    for vote in ctx.store.list_votes()? {
        match vote.state {
            VoteState::NeedsAnalysis | VoteState::NeedsManualParse => {
                awaiting_agent.push(vote_payload(&vote));
            }
            VoteState::Recommended => {
                if let Some(deadline) = vote.deadline {
                    let hours_left = (deadline - now).num_seconds() as f64 / 3600.0;
                    let fired = levels
                        .iter()
                        .copied()
                        .filter(|level| {
                            hours_left <= *level as f64 && !vote.reminded_levels.contains(level)
                        })
                        .min();
                    if let Some(level) = fired {
                        reminders.push(json!({
                            "vote_id": vote.id,
                            "level_hours": level,
                            "deadline": deadline,
                            "hours_left": round1(hours_left),
                        }));
                    }
                }
                awaiting_owner.push(vote_payload(&vote));
            }
            _ => {}
        }
    }

    let payload = json!({
        "awaiting_agent": awaiting_agent,
        "awaiting_owner": awaiting_owner,
        "reminders": reminders,
        "missed_now": missed_now,
    });
    emit(ctx.common, "pending", payload, |v| {
        println!(
            "{} awaiting agent, {} awaiting owner, {} reminder(s) due, {} missed just now",
            v["awaiting_agent"].as_array().map_or(0, Vec::len),
            v["awaiting_owner"].as_array().map_or(0, Vec::len),
            v["reminders"].as_array().map_or(0, Vec::len),
            v["missed_now"].as_array().map_or(0, Vec::len),
        );
    });
    Ok(())
}

/// One decimal place is plenty of precision for a human/agent-facing
/// "hours left" figure.
fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// The write path: silence a reminder level for a vote, once the agent has
/// actually notified the owner at that level. Idempotent — acking the same
/// level twice doesn't duplicate the entry in `reminded_levels`, though each
/// call still lands its own ledger event.
fn ack(ctx: &Ctx, vote_id: &str, level: i64) -> Result<(), CliError> {
    let levels = ctx.cfg.reminder_hours();
    if !levels.contains(&level) {
        return Err(CliError::Usage(format!(
            "--level {level} is not a configured reminder level (configured: {})",
            levels
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    let mut vote = ctx.store.load_vote(vote_id)?;
    if !vote.reminded_levels.contains(&level) {
        vote.reminded_levels.push(level);
        ctx.store.save_vote(&vote)?;
    }
    ctx.store.append_ledger(&LedgerEntry::event(
        Some(vote.id.clone()),
        Actor::Agent,
        "reminder-acked",
        Some(level.to_string()),
    ))?;

    let payload = vote_payload(&vote);
    emit(ctx.common, "vote", payload, |v| {
        println!(
            "{}: reminder level {level} acked",
            v["id"].as_str().unwrap_or_default(),
        );
    });
    Ok(())
}
