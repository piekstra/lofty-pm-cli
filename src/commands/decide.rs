//! `decide` — the human's (or, for `--decline`, a direct-close) response to
//! a `recommended` vote. Legal only from `Recommended`; the state machine
//! enforces that, so an out-of-state call reports `CliError::Usage` (exit
//! 2), same as an illegal `recommend`.

use pk_cli_core::CliError;
use serde_json::json;

use crate::commands::{emit, Ctx};
use crate::model::{Actor, Decision, LedgerEntry, VoteState};

/// `lofty-pm decide <id> (--accept | --override POSITION | --decline)`.
/// The three action flags are mutually exclusive and one is required.
#[derive(clap::Args, Debug)]
#[command(group(
    clap::ArgGroup::new("action")
        .required(true)
        .multiple(false)
        .args(["accept", "override_position", "decline"])
))]
pub struct Args {
    pub id: String,
    /// Accept the agent's recommendation as-is.
    #[arg(long)]
    pub accept: bool,
    /// Accept the recommendation to vote, but with a different position.
    #[arg(long = "override", value_name = "POSITION")]
    pub override_position: Option<String>,
    /// Decline to vote at all; closes the vote immediately (no casting).
    #[arg(long)]
    pub decline: bool,
}

pub fn run(ctx: &Ctx, args: &Args) -> Result<(), CliError> {
    let mut vote = ctx.store.load_vote(&args.id)?;

    // (target state, decision.action, decision.position, outcome, ledger detail)
    let (target, action, position, outcome, detail) = if args.accept {
        let position = vote
            .recommendation
            .as_ref()
            .map(|r| r.position.clone())
            .ok_or_else(|| {
                CliError::Usage(format!(
                    "cannot --accept {}: vote has no recommendation",
                    args.id
                ))
            })?;
        (
            VoteState::Accepted,
            "accepted",
            Some(position),
            None,
            "accepted",
        )
    } else if let Some(p) = &args.override_position {
        (
            VoteState::Overridden,
            "overridden",
            Some(p.clone()),
            None,
            "overridden",
        )
    } else if args.decline {
        (
            VoteState::Closed,
            "declined",
            None,
            Some("declined-by-owner"),
            "declined-by-owner",
        )
    } else {
        // Unreachable in practice: clap's ArgGroup(required = true) already
        // rejects zero flags before `run` is ever called. Guarded anyway.
        return Err(CliError::Usage(
            "decide requires one of --accept, --override, --decline".into(),
        ));
    };

    let (from, to) = vote.advance(target, Actor::Human)?;
    vote.decision = Some(Decision {
        action: action.into(),
        position: position.clone(),
        at: crate::clock::now(),
    });
    if let Some(o) = outcome {
        vote.outcome = Some(o.into());
    }

    ctx.store.save_vote(&vote)?;
    ctx.store.append_ledger(&LedgerEntry::transition(
        &vote.id,
        Actor::Human,
        from,
        to,
        Some(detail.into()),
    ))?;

    let payload = json!({
        "vote_id": vote.id,
        "action": action,
        "position": position,
        "vote_link": vote.vote_link,
        "deadline": vote.deadline,
    });
    let show_cast_link = action != "declined";
    emit(ctx.common, "decision", payload, |v| {
        println!(
            "{}: {}",
            v["vote_id"].as_str().unwrap_or_default(),
            v["action"].as_str().unwrap_or_default(),
        );
        if show_cast_link {
            if let Some(link) = v["vote_link"].as_str() {
                println!("Cast it here: {link}");
            }
        }
    });
    Ok(())
}
