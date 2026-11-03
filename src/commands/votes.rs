//! `votes` — list/show/context tracked votes, and amend a
//! `needs-manual-parse` record by hand.

use chrono::{DateTime, Utc};
use pk_cli_core::CliError;
use serde_json::{json, Value};

use crate::commands::{emit, read_file_or_stdin, Ctx};
use crate::config::Config;
use crate::model::{Actor, LedgerEntry, VoteState};

/// `lofty-pm votes <list|show|context|amend>`.
#[derive(clap::Subcommand, Debug)]
pub enum Cmd {
    /// List tracked votes, newest first, optionally filtered by state.
    List {
        /// One of: needs-manual-parse, needs-analysis, recommended,
        /// accepted, overridden, cast-confirmed, closed, missed.
        #[arg(long)]
        state: Option<String>,
        /// Keep only the first N after filtering (list is already newest
        /// first, so this is the N most recent).
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Show one vote record in full.
    Show { id: String },
    /// Everything an agent needs to recommend: the vote, resolved stance,
    /// recent corpus for the property, suggested `lofty` lookups, and
    /// whether `lofty` is currently usable.
    Context { id: String },
    /// Fix up a `needs-manual-parse` vote's fields by hand. Once both
    /// `property` and `proposal` are non-empty, advances it to
    /// `needs-analysis`.
    Amend {
        id: String,
        #[arg(long)]
        property: Option<String>,
        /// File path, or `-` for stdin. Never pass proposal text inline.
        #[arg(long, value_name = "PATH")]
        proposal_file: Option<String>,
        /// RFC 3339, e.g. 2026-07-29T06:59:00Z.
        #[arg(long)]
        deadline: Option<String>,
        #[arg(long)]
        link: Option<String>,
        /// Slash-separated, e.g. "Approve/Reject/Defer to Property Manager".
        #[arg(long)]
        options: Option<String>,
    },
    /// Confirm the vote has actually been cast on the Lofty site (via the
    /// email link surfaced by `decide`). From `accepted`/`overridden` only.
    MarkCast { id: String },
    /// Close a vote whose result was ambiguous or otherwise missed by
    /// `scan`'s automatic close-on-result-email path. Works from any
    /// non-closed state; the agent (or a human, via the same command) is
    /// asserting the outcome by hand.
    Close {
        id: String,
        #[arg(long)]
        outcome: String,
    },
}

pub fn run(ctx: &Ctx, cmd: &Cmd) -> Result<(), CliError> {
    match cmd {
        Cmd::List { state, limit } => list(ctx, state.as_deref(), *limit),
        Cmd::Show { id } => show(ctx, id),
        Cmd::Context { id } => context(ctx, id),
        Cmd::Amend {
            id,
            property,
            proposal_file,
            deadline,
            link,
            options,
        } => amend(
            ctx,
            id,
            property.as_deref(),
            proposal_file.as_deref(),
            deadline.as_deref(),
            link.as_deref(),
            options.as_deref(),
        ),
        Cmd::MarkCast { id } => mark_cast(ctx, id),
        Cmd::Close { id, outcome } => close(ctx, id, outcome),
    }
}

fn parse_state(s: &str) -> Result<VoteState, CliError> {
    serde_json::from_value(json!(s)).map_err(|_| {
        CliError::Usage(format!(
            "unknown vote state `{s}` (known: needs-manual-parse, needs-analysis, \
             recommended, accepted, overridden, cast-confirmed, closed, missed)"
        ))
    })
}

fn list(ctx: &Ctx, state: Option<&str>, limit: Option<usize>) -> Result<(), CliError> {
    let filter = state.map(parse_state).transpose()?;
    let mut votes = ctx.store.list_votes()?;
    if let Some(f) = filter {
        votes.retain(|v| v.state == f);
    }
    if let Some(limit) = limit {
        votes.truncate(limit);
    }

    let payload = json!({ "votes": votes });
    emit(ctx.common, "vote-list", payload, |v| {
        let votes = v["votes"].as_array().expect("array");
        if votes.is_empty() {
            println!("no votes tracked");
        }
        for vote in votes {
            println!(
                "{} [{}] {} — {}",
                vote["id"].as_str().unwrap_or_default(),
                vote["state"].as_str().unwrap_or_default(),
                vote["property"].as_str().unwrap_or_default(),
                vote["subject"].as_str().unwrap_or_default(),
            );
        }
    });
    Ok(())
}

/// Serialize a `VoteRecord`, stripping its own on-disk `schema` field
/// (`vote-record/v1`) so it doesn't clash with `emit`'s wire-level schema
/// tag when the record's fields are merged at the payload's top level.
/// `pub(crate)`: also reused by `pending` for `awaiting_agent`/`awaiting_owner`.
pub(crate) fn vote_payload(vote: &crate::model::VoteRecord) -> Value {
    let mut v = serde_json::to_value(vote).expect("VoteRecord always serializes");
    if let Value::Object(ref mut m) = v {
        m.remove("schema");
    }
    v
}

fn show(ctx: &Ctx, id: &str) -> Result<(), CliError> {
    let vote = ctx.store.load_vote(id)?;
    let payload = vote_payload(&vote);
    emit(ctx.common, "vote", payload, |v| {
        println!(
            "{} [{}] {}",
            v["id"].as_str().unwrap_or_default(),
            v["state"].as_str().unwrap_or_default(),
            v["property"].as_str().unwrap_or_default(),
        );
        println!("proposal: {}", v["proposal"].as_str().unwrap_or_default());
        if let Some(r) = v.get("recommendation").filter(|r| !r.is_null()) {
            println!(
                "recommendation: {} (confidence {})",
                r["position"].as_str().unwrap_or_default(),
                r["confidence"],
            );
        }
    });
    Ok(())
}

fn context(ctx: &Ctx, id: &str) -> Result<(), CliError> {
    let vote = ctx.store.load_vote(id)?;
    let stance = ctx.store.load_stance()?.resolved(&vote.property);

    let mut corpus = ctx.store.list_corpus(Some(&vote.property))?;
    corpus.sort_by_key(|c| std::cmp::Reverse(c.date));
    corpus.truncate(10);

    let (ok, degraded_reason) = lofty_ok(ctx.cfg);

    let payload = json!({
        "vote": vote,
        "stance": stance,
        "corpus": corpus,
        "lofty_queries": [
            "lofty properties list --json  # find <property_name>",
            "lofty properties get <slug-or-id> --json",
        ],
        "degraded": !ok,
        "degraded_reason": degraded_reason,
    });
    emit(ctx.common, "vote-context", payload, |v| {
        println!(
            "{} [{}]: {} corpus item(s), degraded={}",
            v["vote"]["id"].as_str().unwrap_or_default(),
            v["vote"]["state"].as_str().unwrap_or_default(),
            v["corpus"].as_array().map_or(0, Vec::len),
            v["degraded"],
        );
        if let Some(r) = v["degraded_reason"].as_str() {
            println!("degraded_reason: {r}");
        }
    });
    Ok(())
}

/// Whether `<lofty_bin> auth status --json` reports a usable session.
/// Degraded (`(false, Some(reason))`) covers four failure shapes: the
/// binary fails to spawn at all, it exits non-zero, its stdout doesn't
/// parse as JSON, or the parsed `authenticated` field isn't `true`.
/// `votes context` must never fail outright just because `lofty` is
/// unavailable — this always returns, never propagates a `CliError`.
/// `pub(crate)`: also reused by `init`'s `lofty_auth` check.
pub(crate) fn lofty_ok(cfg: &Config) -> (bool, Option<String>) {
    let bin = cfg.lofty_bin();
    let output = match std::process::Command::new(bin)
        .args(["auth", "status", "--json"])
        .output()
    {
        Ok(o) => o,
        Err(e) => return (false, Some(format!("running `{bin} auth status`: {e}"))),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return (
            false,
            Some(format!(
                "`{bin} auth status` exited {}: {stderr}",
                output.status
            )),
        );
    }
    let parsed: Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => {
            return (
                false,
                Some(format!("parsing `{bin} auth status` output: {e}")),
            )
        }
    };
    if parsed.get("authenticated") == Some(&Value::Bool(true)) {
        (true, None)
    } else {
        (
            false,
            Some(format!("`{bin} auth status` reports not authenticated")),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn amend(
    ctx: &Ctx,
    id: &str,
    property: Option<&str>,
    proposal_file: Option<&str>,
    deadline: Option<&str>,
    link: Option<&str>,
    options: Option<&str>,
) -> Result<(), CliError> {
    let mut vote = ctx.store.load_vote(id)?;

    if let Some(p) = property {
        vote.property = p.to_string();
    }
    if let Some(path) = proposal_file {
        vote.proposal = read_file_or_stdin(path)?;
    }
    if let Some(d) = deadline {
        let parsed: DateTime<Utc> = d
            .parse()
            .map_err(|e| CliError::Usage(format!("--deadline must be RFC 3339, got `{d}`: {e}")))?;
        vote.deadline = Some(parsed);
    }
    if let Some(l) = link {
        vote.vote_link = Some(l.to_string());
    }
    if let Some(opts) = options {
        vote.options = opts
            .split('/')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }

    // Only the agent's fix-up path (needs-manual-parse -> needs-analysis)
    // is legal here; amending a vote in any other state just updates
    // fields in place with no transition.
    let transition = if vote.state == VoteState::NeedsManualParse
        && !vote.property.is_empty()
        && !vote.proposal.is_empty()
    {
        Some(vote.advance(VoteState::NeedsAnalysis, Actor::Agent)?)
    } else {
        None
    };

    ctx.store.save_vote(&vote)?;
    if let Some((from, to)) = transition {
        ctx.store.append_ledger(&LedgerEntry::transition(
            &vote.id,
            Actor::Agent,
            from,
            to,
            Some("amended".into()),
        ))?;
    }

    let payload = vote_payload(&vote);
    emit(ctx.common, "vote", payload, |v| {
        println!(
            "{} [{}] {}",
            v["id"].as_str().unwrap_or_default(),
            v["state"].as_str().unwrap_or_default(),
            v["property"].as_str().unwrap_or_default(),
        );
    });
    Ok(())
}

/// Confirm the vote has actually been cast (the human clicked the email
/// link and voted on lofty.ai). Only legal from `accepted`/`overridden` —
/// the state machine enforces that.
fn mark_cast(ctx: &Ctx, id: &str) -> Result<(), CliError> {
    let mut vote = ctx.store.load_vote(id)?;
    let (from, to) = vote.advance(VoteState::CastConfirmed, Actor::Human)?;

    ctx.store.save_vote(&vote)?;
    ctx.store.append_ledger(&LedgerEntry::transition(
        &vote.id,
        Actor::Human,
        from,
        to,
        Some("cast-confirmed".into()),
    ))?;

    let payload = vote_payload(&vote);
    emit(ctx.common, "vote", payload, |v| {
        println!(
            "{} [{}] cast confirmed",
            v["id"].as_str().unwrap_or_default(),
            v["state"].as_str().unwrap_or_default(),
        );
    });
    Ok(())
}

/// Close a vote by hand with a stated outcome — the agent's (or a human's)
/// tool for ambiguous results that `scan`'s close-on-result-email path
/// didn't catch. Legal from any non-closed state.
fn close(ctx: &Ctx, id: &str, outcome: &str) -> Result<(), CliError> {
    let mut vote = ctx.store.load_vote(id)?;
    let (from, to) = vote.advance(VoteState::Closed, Actor::Agent)?;
    vote.outcome = Some(outcome.to_string());

    ctx.store.save_vote(&vote)?;
    ctx.store.append_ledger(&LedgerEntry::transition(
        &vote.id,
        Actor::Agent,
        from,
        to,
        Some(outcome.to_string()),
    ))?;

    let payload = vote_payload(&vote);
    emit(ctx.common, "vote", payload, |v| {
        println!(
            "{} [{}] closed: {}",
            v["id"].as_str().unwrap_or_default(),
            v["state"].as_str().unwrap_or_default(),
            v["outcome"].as_str().unwrap_or_default(),
        );
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Write `script` from a child process, so this process never holds it open for
    /// writing. A fork on another test's thread keeps a copy of every descriptor open
    /// here until it execs, and on Linux running a file with a write descriptor open
    /// anywhere fails with ETXTBSY (os error 26).
    fn write_from_a_child(path: &std::path::Path, script: &str) {
        let status = std::process::Command::new("sh")
            .args(["-c", "printf %s \"$1\" > \"$2\"", "sh", script])
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success(), "writing {}", path.display());
    }

    /// Write an executable shell script stub at `dir/lofty` and return its
    /// path as a `Config` pointing `lofty_bin` at it.
    fn stub_cfg(dir: &std::path::Path, script: &str) -> Config {
        let path = dir.join("lofty");
        write_from_a_child(&path, &format!("#!/bin/sh\n{script}\n"));
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        Config {
            lofty_bin: Some(path.to_str().unwrap().to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn lofty_ok_true_when_authenticated() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = stub_cfg(tmp.path(), r#"echo '{"authenticated": true}'"#);
        assert_eq!(lofty_ok(&cfg), (true, None));
    }

    #[test]
    fn lofty_ok_degraded_on_spawn_failure() {
        let cfg = Config {
            lofty_bin: Some("/nonexistent/lofty-bin-that-does-not-exist".into()),
            ..Default::default()
        };
        let (ok, reason) = lofty_ok(&cfg);
        assert!(!ok);
        assert!(reason.is_some());
    }

    #[test]
    fn lofty_ok_degraded_on_nonzero_exit() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = stub_cfg(tmp.path(), "exit 1");
        let (ok, reason) = lofty_ok(&cfg);
        assert!(!ok);
        assert!(reason.is_some());
    }

    #[test]
    fn lofty_ok_degraded_on_unparseable_stdout() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = stub_cfg(tmp.path(), "echo 'not json'");
        let (ok, reason) = lofty_ok(&cfg);
        assert!(!ok);
        assert!(reason.is_some());
    }

    #[test]
    fn lofty_ok_degraded_when_not_authenticated() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = stub_cfg(tmp.path(), r#"echo '{"authenticated": false}'"#);
        let (ok, reason) = lofty_ok(&cfg);
        assert!(!ok);
        assert!(reason.is_some());
    }

    #[test]
    fn state_filter_rejects_unknown_state() {
        assert!(parse_state("bogus").is_err());
        assert_eq!(
            parse_state("needs-analysis").unwrap(),
            VoteState::NeedsAnalysis
        );
    }
}
