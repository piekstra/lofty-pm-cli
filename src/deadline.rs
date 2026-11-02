//! Deadline sweep, shared by `scan` (pre-mail-processing) and `pending`
//! (pre-listing): pre-decision votes whose deadline has passed are marked
//! `Missed` by the clock, not a person.

use pk_cli_core::CliError;

use crate::model::{Actor, LedgerEntry, VoteState};
use crate::store::Store;

/// Pre-decision votes (`NeedsManualParse`/`NeedsAnalysis`/`Recommended`)
/// whose deadline has passed are marked `Missed` by the clock, not a
/// person. Returns the ids of votes newly marked.
pub fn sweep_deadlines(store: &Store) -> Result<Vec<String>, CliError> {
    let now = crate::clock::now();
    let mut missed = Vec::new();
    for mut record in store.list_votes()? {
        let pre_decision = matches!(
            record.state,
            VoteState::NeedsManualParse | VoteState::NeedsAnalysis | VoteState::Recommended
        );
        if !pre_decision {
            continue;
        }
        let Some(deadline) = record.deadline else {
            continue;
        };
        if deadline >= now {
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
