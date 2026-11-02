//! `recommend` — the agent's recommendation for how to vote. Advances a
//! vote from `needs-analysis` to `recommended`; illegal on any other state
//! (e.g. a vote that's already been recommended or decided), which
//! `VoteRecord::advance` reports as a `CliError::Usage` (exit 2).

use pk_cli_core::CliError;
use serde_json::json;

use crate::commands::{emit, read_file_or_stdin, Ctx};
use crate::model::{Actor, LedgerEntry, Recommendation, Tier, VoteState};

/// `lofty-pm recommend <id> --position P --confidence 0.0-1.0 --tier
/// routine|high-stakes --reasoning-file PATH`.
#[derive(clap::Args, Debug)]
pub struct Args {
    pub id: String,
    #[arg(long)]
    pub position: String,
    /// 0.0-1.0.
    #[arg(long)]
    pub confidence: f64,
    /// routine|high-stakes — overrides the scan-time heuristic.
    #[arg(long)]
    pub tier: String,
    /// File path, or `-` for stdin. Reasoning is never passed inline on
    /// argv (long text, shell quoting).
    #[arg(long, value_name = "PATH")]
    pub reasoning_file: String,
}

fn parse_tier(s: &str) -> Result<Tier, CliError> {
    match s {
        "routine" => Ok(Tier::Routine),
        "high-stakes" => Ok(Tier::HighStakes),
        other => Err(CliError::Usage(format!(
            "--tier must be routine|high-stakes (got `{other}`)"
        ))),
    }
}

pub fn run(ctx: &Ctx, args: &Args) -> Result<(), CliError> {
    if !(0.0..=1.0).contains(&args.confidence) {
        return Err(CliError::Usage(format!(
            "--confidence must be within 0.0..=1.0 (got {})",
            args.confidence
        )));
    }
    let tier = parse_tier(&args.tier)?;

    let mut vote = ctx.store.load_vote(&args.id)?;
    // Gate on the state machine before touching the filesystem for the
    // reasoning text — an illegal transition (e.g. double-recommend)
    // should fail fast without requiring a readable --reasoning-file.
    let (from, to) = vote.advance(VoteState::Recommended, Actor::Agent)?;

    let reasoning = read_file_or_stdin(&args.reasoning_file)?;
    let recommendation = Recommendation {
        position: args.position.clone(),
        confidence: args.confidence,
        tier,
        reasoning,
        at: crate::clock::now(),
    };
    vote.recommendation = Some(recommendation.clone());
    vote.tier = Some(tier);

    ctx.store.save_vote(&vote)?;
    ctx.store.append_ledger(&LedgerEntry::transition(
        &vote.id,
        Actor::Agent,
        from,
        to,
        Some("recommended".into()),
    ))?;

    let payload = json!({
        "vote_id": vote.id,
        "recommendation": recommendation,
    });
    emit(ctx.common, "recommendation", payload, |v| {
        println!(
            "{}: recommend {} (confidence {}, tier {})",
            v["vote_id"].as_str().unwrap_or_default(),
            v["recommendation"]["position"].as_str().unwrap_or_default(),
            v["recommendation"]["confidence"],
            v["recommendation"]["tier"].as_str().unwrap_or_default(),
        );
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_parses_known_values_and_rejects_others() {
        assert!(matches!(parse_tier("routine"), Ok(Tier::Routine)));
        assert!(matches!(parse_tier("high-stakes"), Ok(Tier::HighStakes)));
        assert!(parse_tier("urgent").is_err());
    }
}
