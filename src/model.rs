//! Domain model + guarded vote state machine.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use pk_cli_core::CliError;
use serde::{Deserialize, Serialize};

/// Lifecycle of a single vote, from first sighting through resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VoteState {
    NeedsManualParse,
    NeedsAnalysis,
    Recommended,
    Accepted,
    Overridden,
    CastConfirmed,
    Closed,
    Missed,
}

/// Who drove a given transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Actor {
    Scan,
    Agent,
    Human,
    Deadline,
}

/// Governance stakes tier, used to gate auto-mode behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    Routine,
    HighStakes,
}

/// The agent's recommendation for how to vote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub position: String,
    pub confidence: f64,
    pub tier: Tier,
    pub reasoning: String,
    pub at: DateTime<Utc>,
}

/// The human's (or agent's, in auto mode) decision on a vote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    /// accepted|overridden|declined
    pub action: String,
    pub position: Option<String>,
    pub at: DateTime<Utc>,
}

/// One immutable ledger record: either a state transition or a bare event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub schema: String,
    pub at: DateTime<Utc>,
    pub vote_id: Option<String>,
    pub actor: Actor,
    pub event: String,
    pub from: Option<VoteState>,
    pub to: Option<VoteState>,
    pub detail: Option<String>,
}

impl LedgerEntry {
    /// Record a legal state transition.
    pub fn transition(
        vote: &str,
        actor: Actor,
        from: VoteState,
        to: VoteState,
        detail: Option<String>,
    ) -> Self {
        Self {
            schema: "ledger-entry/v1".into(),
            at: crate::clock::now(),
            vote_id: Some(vote.into()),
            actor,
            event: "transition".into(),
            from: Some(from),
            to: Some(to),
            detail,
        }
    }

    /// Record a bare event with no state transition (e.g. a scan run).
    pub fn event(
        vote_id: Option<String>,
        actor: Actor,
        event: &str,
        detail: Option<String>,
    ) -> Self {
        Self {
            schema: "ledger-entry/v1".into(),
            at: crate::clock::now(),
            vote_id,
            actor,
            event: event.into(),
            from: None,
            to: None,
            detail,
        }
    }
}

fn default_vote_record_schema() -> String {
    "vote-record/v1".into()
}

/// A single tracked vote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoteRecord {
    #[serde(default = "default_vote_record_schema")]
    pub schema: String,
    pub id: String,
    pub property: String,
    pub subject: String,
    pub proposal: String,
    pub options: Vec<String>,
    pub deadline: Option<DateTime<Utc>>,
    pub vote_link: Option<String>,
    pub tier: Option<Tier>,
    pub state: VoteState,
    pub recommendation: Option<Recommendation>,
    pub decision: Option<Decision>,
    pub outcome: Option<String>,
    pub message_ids: Vec<String>,
    pub raw: Option<String>,
    pub reminded_levels: Vec<i64>,
    pub created_at: DateTime<Utc>,
}

impl VoteRecord {
    pub fn new(
        id: impl Into<String>,
        property: impl Into<String>,
        subject: impl Into<String>,
        proposal: impl Into<String>,
        options: Vec<String>,
    ) -> Self {
        Self {
            schema: default_vote_record_schema(),
            id: id.into(),
            property: property.into(),
            subject: subject.into(),
            proposal: proposal.into(),
            options,
            deadline: None,
            vote_link: None,
            tier: None,
            state: VoteState::NeedsAnalysis,
            recommendation: None,
            decision: None,
            outcome: None,
            message_ids: Vec::new(),
            raw: None,
            reminded_levels: Vec::new(),
            created_at: crate::clock::now(),
        }
    }

    /// Test/fixture builder: override the initial state. Also used in
    /// production for needs-manual-parse creation (Task 3 on).
    pub fn with_state(mut self, state: VoteState) -> Self {
        self.state = state;
        self
    }

    /// Attempt a guarded state transition. Returns `(from, to)` on success
    /// for the caller to hand to `LedgerEntry::transition`.
    pub fn advance(
        &mut self,
        to: VoteState,
        actor: Actor,
    ) -> Result<(VoteState, VoteState), CliError> {
        let from = self.state;
        if !allowed(from, to, actor) {
            return Err(CliError::Usage(format!(
                "illegal transition {from:?} -> {to:?} by {actor:?}"
            )));
        }
        self.state = to;
        Ok((from, to))
    }
}

fn allowed(from: VoteState, to: VoteState, actor: Actor) -> bool {
    use Actor::*;
    use VoteState::*;
    match (from, to, actor) {
        (NeedsManualParse, NeedsAnalysis, Agent) => true,
        (NeedsAnalysis, Recommended, Agent) => true,
        (Recommended, Accepted | Overridden, Human) => true,
        // decline is Decision::action="declined" + direct close by Human:
        (Recommended, Closed, Human) => true,
        (Accepted | Overridden, CastConfirmed, Human) => true,
        // scan closes anything not yet closed when a result email arrives:
        (
            NeedsManualParse | NeedsAnalysis | Recommended | Accepted | Overridden | CastConfirmed
            | Missed,
            Closed,
            Scan,
        ) => true,
        // `votes close --outcome` — the agent's (or human's, via the same
        // command) tool for ambiguous results not caught by scan; mirrors
        // the Scan row above but with correct attribution (Agent, not Scan):
        (
            NeedsManualParse | NeedsAnalysis | Recommended | Accepted | Overridden | CastConfirmed
            | Missed,
            Closed,
            Agent,
        ) => true,
        // deadline lapse, pre-decision states only:
        (NeedsManualParse | NeedsAnalysis | Recommended, Missed, Deadline) => true,
        _ => false,
    }
}

/// Per-property/global investment stance, used to steer recommendations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StanceProfile {
    pub risk_tolerance: Option<String>,
    pub horizon: Option<String>,
    pub orientation: Option<String>,
    pub add_capital: Option<String>,
    pub notes: Option<String>,
}

/// Provenance for a single stance field, keyed e.g. `"global.risk_tolerance"`
/// or `"<property>.horizon"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldMeta {
    pub interviewed_at: DateTime<Utc>,
    pub updated_by: String,
}

fn default_stance_schema() -> String {
    "stance/v1".into()
}

/// The full stance file: a global profile plus per-property overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StanceFile {
    #[serde(default = "default_stance_schema")]
    pub schema: String,
    pub global: StanceProfile,
    pub overrides: BTreeMap<String, StanceProfile>,
    pub meta: BTreeMap<String, FieldMeta>,
}

impl Default for StanceFile {
    fn default() -> Self {
        Self {
            schema: default_stance_schema(),
            global: StanceProfile::default(),
            overrides: BTreeMap::new(),
            meta: BTreeMap::new(),
        }
    }
}

impl StanceFile {
    /// Merge a property's override profile over the global profile,
    /// field by field (override wins where set).
    pub fn resolved(&self, property: &str) -> StanceProfile {
        let o = self.overrides.get(property);
        StanceProfile {
            risk_tolerance: o
                .and_then(|o| o.risk_tolerance.clone())
                .or_else(|| self.global.risk_tolerance.clone()),
            horizon: o
                .and_then(|o| o.horizon.clone())
                .or_else(|| self.global.horizon.clone()),
            orientation: o
                .and_then(|o| o.orientation.clone())
                .or_else(|| self.global.orientation.clone()),
            add_capital: o
                .and_then(|o| o.add_capital.clone())
                .or_else(|| self.global.add_capital.clone()),
            notes: o
                .and_then(|o| o.notes.clone())
                .or_else(|| self.global.notes.clone()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    fn vote(state: VoteState) -> VoteRecord {
        VoteRecord::new("v-1", "1500-lakeview-ave", "subj", "prop", vec![]).with_state(state)
    }

    #[test]
    fn happy_path_transitions() {
        let mut v = vote(VoteState::NeedsAnalysis);
        v.advance(VoteState::Recommended, Actor::Agent).unwrap();
        v.advance(VoteState::Accepted, Actor::Human).unwrap();
        v.advance(VoteState::CastConfirmed, Actor::Human).unwrap();
        v.advance(VoteState::Closed, Actor::Scan).unwrap();
        assert_eq!(v.state, VoteState::Closed);
    }

    #[test]
    fn missed_only_via_deadline_actor() {
        assert!(vote(VoteState::Recommended)
            .advance(VoteState::Missed, Actor::Agent)
            .is_err());
        assert!(vote(VoteState::Recommended)
            .advance(VoteState::Missed, Actor::Deadline)
            .is_ok());
    }

    #[test]
    fn decisions_only_by_human_from_recommended() {
        assert!(vote(VoteState::NeedsAnalysis)
            .advance(VoteState::Accepted, Actor::Human)
            .is_err());
        assert!(vote(VoteState::Recommended)
            .advance(VoteState::Accepted, Actor::Agent)
            .is_err());
    }

    #[test]
    fn closed_is_terminal() {
        assert!(vote(VoteState::Closed)
            .advance(VoteState::NeedsAnalysis, Actor::Agent)
            .is_err());
    }

    #[test]
    fn manual_parse_resolves_to_analysis() {
        assert!(vote(VoteState::NeedsManualParse)
            .advance(VoteState::NeedsAnalysis, Actor::Agent)
            .is_ok());
    }

    #[test]
    fn agent_can_close_ambiguous_results() {
        assert!(vote(VoteState::CastConfirmed)
            .advance(VoteState::Closed, Actor::Agent)
            .is_ok());
    }

    #[test]
    fn stance_resolution_merges_override_over_global() {
        let mut s = StanceFile::default();
        s.global.risk_tolerance = Some("moderate".into());
        s.global.horizon = Some("long-term".into());
        let mut o = StanceProfile::default();
        o.horizon = Some("short-term".into());
        s.overrides.insert("1500-lakeview-ave".into(), o);
        let r = s.resolved("1500-lakeview-ave");
        assert_eq!(r.risk_tolerance.as_deref(), Some("moderate"));
        assert_eq!(r.horizon.as_deref(), Some("short-term"));
    }

    #[test]
    fn serde_round_trip_uses_kebab_case_states() {
        let v = vote(VoteState::NeedsManualParse);
        let j = serde_json::to_value(&v).unwrap();
        assert_eq!(j["state"], "needs-manual-parse");
        let back: VoteRecord = serde_json::from_value(j).unwrap();
        assert_eq!(back.state, VoteState::NeedsManualParse);
    }
}
