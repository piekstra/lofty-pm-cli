//! Deadline sweep, shared by `scan` (pre-mail-processing) and `pending`
//! (pre-listing): pre-decision votes whose deadline has passed are marked
//! `Missed` by the clock, not a person.

use chrono::{DateTime, Utc};
use pk_cli_core::CliError;

use crate::model::{Actor, LedgerEntry, VoteRecord, VoteState};
use crate::store::Store;

/// Whether the clock has run out on a vote nobody decided: what the sweep marks
/// `Missed`, and what the read-only MCP tools report as missed without writing it.
pub fn lapsed(record: &VoteRecord, now: DateTime<Utc>) -> bool {
    let pre_decision = matches!(
        record.state,
        VoteState::NeedsManualParse | VoteState::NeedsAnalysis | VoteState::Recommended
    );
    pre_decision && record.deadline.is_some_and(|deadline| deadline < now)
}

/// Pre-decision votes (`NeedsManualParse`/`NeedsAnalysis`/`Recommended`)
/// whose deadline has passed are marked `Missed` by the clock, not a
/// person. Returns the ids of votes newly marked.
pub fn sweep_deadlines(store: &Store) -> Result<Vec<String>, CliError> {
    let now = crate::clock::now();
    let mut missed = Vec::new();
    for mut record in store.list_votes()? {
        if !lapsed(&record, now) {
            continue;
        }

        let (from, to) = record.advance(VoteState::Missed, Actor::Deadline)?;
        store.save_vote(&record)?;
        store.append_ledger(&LedgerEntry::transition(
            &record.id,
            Actor::Deadline,
            from,
            to,
            None,
        ))?;
        missed.push(record.id.clone());
    }
    Ok(missed)
}
