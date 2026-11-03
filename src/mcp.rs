//! `lofty-pm mcp`: read-only tools for Claude, over the local store `scan` fills.
//!
//! The tools read files and nothing else: no mail, no Lofty, no subprocess, no
//! keychain, no config. [`ReadStore`] offers reads alone and opening it creates
//! nothing, so no tool can change the data directory, and the deadline sweep that
//! `lofty-pm pending` writes is computed here without writing.
//!
//! A tool never builds a path from an argument: `Store::load_vote` and
//! `Store::list_corpus(Some(..))` would, so a vote or property is found by
//! comparing what the store lists.
//!
//! A Vote Now link lets whoever holds it reach the ballot, so it is withheld: the
//! record's `vote_link`, and any URL in the text of a proposal or an email. The raw
//! email, the owner's stance and the message ids a record lists are not served
//! either. A vote's own id is: `scan` builds it from the id of the email that
//! announced the vote, it is the name `lofty-pm` knows the vote by, and it opens
//! nothing without the mailbox.
//!
//! An empty answer from a store nothing has ever filled would read as "no votes",
//! so that is an error naming `lofty-pm scan`, and results carry `last_scan`.

use std::path::PathBuf;
use std::sync::OnceLock;

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use pk_cli_core::CliError;
use pk_cli_mcp::{
    parse_args, ListArgs, Page, PageRequest, Server, ToolDef, ToolError, ToolOutput, Toolset,
};
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::deadline::lapsed;
use crate::model::{VoteRecord, VoteState};
use crate::store::{CorpusItem, LastScan, ReadStore};

/// How much of an email a digest row quotes, in characters.
const EXCERPT: usize = 300;
/// How much of an email or proposal a detailed row quotes, in characters.
const DETAILED_EXCERPT: usize = 2000;

/// The tools `lofty-pm mcp` serves. Nothing here touches the disk: a client lists
/// tools before anything has been scanned.
pub fn tools() -> PmTools {
    PmTools {
        server: server(),
        root: None,
    }
}

fn server() -> Server {
    Server::new(
        "lofty-pm",
        env!("CARGO_PKG_VERSION"),
        "Lofty governance",
        "loftypm",
    )
    // There is no sign-in. What a tool can ask for is a scan.
    .sign_in_command("lofty-pm scan")
    .instructions(
        "Access here is read-only: no tool can cast a vote, record a recommendation or a \
         decision, or change anything. A vote is cast by the user on Lofty, from the link in \
         their email; no tool returns that link. Results come from the local store `lofty-pm \
         scan` fills from the user's email and do not reach Lofty, so an old last_scan means \
         they may be behind. unread_emails or scan_narrowed mean that scan did not cover all \
         the mail, so votes may be missing. Ask the user to run `lofty-pm scan` again; if \
         unread_emails stays, `lofty-pm scan --json` lists those emails in manual_parse, to \
         read by hand. A vote past its deadline that nobody decided reads as missed.",
    )
}

/// The toolset `lofty-pm mcp` serves.
pub struct PmTools {
    server: Server,
    /// The data directory, for tests. Otherwise resolved on each call.
    root: Option<PathBuf>,
}

/// The error's message names a file under the user's home directory, which has no
/// business crossing to a model; the command names it in their terminal.
fn store_error(command: &str, _: CliError) -> ToolError {
    ToolError::other("the local lofty-pm data could not be read").with_fix(format!(
        "Ask the user to run `lofty-pm {command}` in a terminal, which names the file"
    ))
}

fn never_scanned() -> ToolError {
    ToolError::other("lofty-pm has not scanned yet, so it knows of no votes or updates")
        .with_fix("Ask the user to run `lofty-pm scan` in a terminal, then call the tool again")
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn day(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d").to_string()
}

fn wire<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

/// `text` with every URL taken out.
fn without_links(text: &str) -> String {
    static LINK: OnceLock<Regex> = OnceLock::new();
    LINK.get_or_init(|| Regex::new(r"(?i)\b(?:https?://|www\.)[^\s]+").expect("a valid pattern"))
        .replace_all(text, "[link withheld]")
        .into_owned()
}

/// The first `max` characters of `text` once its URLs are out, so a cut cannot
/// leave part of one behind.
fn quoted(text: &str, max: usize) -> String {
    let clean = without_links(text);
    if clean.chars().count() <= max {
        return clean;
    }
    let mut cut: String = clean.chars().take(max).collect();
    cut.push('…');
    cut
}

fn trimmed(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn parse_since(value: &str) -> Result<DateTime<Utc>, ToolError> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| ToolError::usage(format!("since must be YYYY-MM-DD, got `{value}`")))?;
    let midnight = date.and_hms_opt(0, 0, 0).expect("midnight exists");
    Ok(DateTime::<Utc>::from_naive_utc_and_offset(midnight, Utc))
}

fn matches_property(property: &str, query: &str) -> bool {
    property.to_lowercase().contains(&query.to_lowercase())
}

/// A vote's state as of `now`: what the next sweep would make it.
fn state_now(vote: &VoteRecord, now: DateTime<Utc>) -> VoteState {
    if lapsed(vote, now) {
        VoteState::Missed
    } else {
        vote.state
    }
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A result, and what the store last heard from the mailbox.
#[derive(Debug, Serialize, JsonSchema)]
struct Scanned<T: Serialize + JsonSchema> {
    /// When `lofty-pm scan` last finished. Null if no scan has recorded one.
    last_scan: Option<String>,
    /// Emails that scan found and could not read, so it may have missed a vote. If this
    /// stays after another scan, `lofty-pm scan --json` lists them in `manual_parse`.
    #[serde(skip_serializing_if = "is_zero")]
    unread_emails: u64,
    /// A `--query`, `--max` or the search cap kept that scan from all the matching
    /// mail, so it may have missed a vote.
    #[serde(skip_serializing_if = "is_false")]
    scan_narrowed: bool,
    #[serde(flatten)]
    page: Page<T>,
}

impl<T: Serialize + JsonSchema> Scanned<T> {
    fn new(last: Option<LastScan>, page: Page<T>) -> Self {
        Scanned {
            last_scan: last.map(|scan| rfc3339(scan.at)),
            unread_emails: last.map_or(0, |scan| scan.unread),
            scan_narrowed: last.is_some_and(|scan| scan.narrowed),
            page,
        }
    }
}

/// `loftypm_pending`'s arguments.
#[derive(Debug, Deserialize, JsonSchema)]
struct PendingArgs {
    #[serde(flatten)]
    list: ListArgs,
}

/// A vote someone has yet to act on.
#[derive(Debug, Serialize, JsonSchema)]
struct PendingItem {
    id: String,
    property: String,
    subject: String,
    state: String,
    /// `analysis`, `decision` (the user's) or `casting` (the user's, on Lofty).
    waiting_on: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    deadline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hours_left: Option<f64>,
    /// What the analysis recommended, if it has run.
    #[serde(skip_serializing_if = "Option::is_none")]
    recommended: Option<String>,
    /// What the user decided, if they have.
    #[serde(skip_serializing_if = "Option::is_none")]
    decided: Option<String>,
}

/// `loftypm_pending`'s result.
#[derive(Debug, Serialize, JsonSchema)]
struct PendingOut {
    /// Votes whose deadline passed with no decision.
    missed: u64,
    #[serde(flatten)]
    scanned: Scanned<PendingItem>,
}

/// `loftypm_votes`' arguments.
#[derive(Debug, Deserialize, JsonSchema)]
struct VoteArgs {
    /// Only votes in this state.
    #[serde(default)]
    state: Option<VoteState>,
    /// Only properties whose name holds this text, in any case.
    #[serde(default)]
    property: Option<String>,
    /// One vote, by its id.
    #[serde(default)]
    vote: Option<String>,
    #[serde(flatten)]
    list: ListArgs,
}

/// One tracked vote, without its link, raw email or message ids.
#[derive(Debug, Serialize, JsonSchema)]
struct VoteItem {
    id: String,
    property: String,
    subject: String,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    deadline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recommended: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decided: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proposal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detected: Option<String>,
}

/// `loftypm_digest`'s arguments.
#[derive(Debug, Deserialize, JsonSchema)]
struct DigestArgs {
    /// Only emails from this day on, YYYY-MM-DD.
    #[serde(default)]
    since: Option<String>,
    /// Only properties whose name holds this text, in any case.
    #[serde(default)]
    property: Option<String>,
    #[serde(flatten)]
    list: ListArgs,
}

/// One filed email, quoted without its links.
#[derive(Debug, Serialize, JsonSchema)]
struct DigestItem {
    date: String,
    property: String,
    /// `update` or `result`.
    kind: String,
    subject: String,
    excerpt: String,
}

/// `loftypm_ledger`'s arguments.
#[derive(Debug, Deserialize, JsonSchema)]
struct LedgerArgs {
    /// Only this vote's entries, by its id.
    #[serde(default)]
    vote: Option<String>,
    #[serde(flatten)]
    list: ListArgs,
}

/// One audit entry.
#[derive(Debug, Serialize, JsonSchema)]
struct LedgerItem {
    at: String,
    event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    vote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

impl PmTools {
    fn store(&self) -> Result<ReadStore, ToolError> {
        match &self.root {
            Some(root) => Ok(ReadStore::at(root)),
            None => ReadStore::open().map_err(|e| store_error("init", e)),
        }
    }

    /// What the last scan covered, or an error if nothing has ever filled the store.
    /// The other files are read only when no scan and no vote say otherwise.
    fn last_scan(store: &ReadStore, votes: &[VoteRecord]) -> Result<Option<LastScan>, ToolError> {
        let last = store.last_scan().map_err(|e| store_error("scan", e))?;
        if last.is_none()
            && votes.is_empty()
            && store
                .corpus()
                .map_err(|e| store_error("digest", e))?
                .is_empty()
            && store
                .ledger()
                .map_err(|e| store_error("ledger", e))?
                .is_empty()
        {
            return Err(never_scanned());
        }
        Ok(last)
    }

    /// A vote id argument, as long as the store holds that vote.
    fn known_vote<'a>(
        votes: &[VoteRecord],
        id: Option<&'a str>,
    ) -> Result<Option<&'a str>, ToolError> {
        match id {
            Some(id) if !votes.iter().any(|v| v.id == id) => {
                Err(ToolError::not_found(format!("no vote with id `{id}`"))
                    .with_fix("Call loftypm_votes to see the ids, then call again"))
            }
            other => Ok(other),
        }
    }

    /// A property argument, as long as something in the store is about it: a
    /// mistyped name would otherwise read as nothing having happened there.
    fn known_property(
        votes: &[VoteRecord],
        corpus: &[CorpusItem],
        query: &str,
    ) -> Result<(), ToolError> {
        if votes.iter().any(|v| matches_property(&v.property, query))
            || corpus.iter().any(|c| matches_property(&c.property, query))
        {
            return Ok(());
        }
        Err(
            ToolError::not_found(format!("no property matching `{query}`"))
                .with_fix("Call loftypm_votes to see the property names, then call again"),
        )
    }

    fn pending(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args: PendingArgs = parse_args(args)?;
        let page = PageRequest::new(&args.list, &())?;
        let store = self.store()?;
        let votes = store.votes().map_err(|e| store_error("votes list", e))?;
        let last_scan = Self::last_scan(&store, &votes)?;
        let now = crate::clock::now();

        let mut missed = 0;
        let mut waiting: Vec<(Option<DateTime<Utc>>, PendingItem)> = Vec::new();
        for vote in votes {
            let waiting_on = match state_now(&vote, now) {
                VoteState::NeedsManualParse | VoteState::NeedsAnalysis => "analysis",
                VoteState::Recommended => "decision",
                VoteState::Accepted | VoteState::Overridden
                    if vote.deadline.is_none_or(|d| d >= now) =>
                {
                    "casting"
                }
                VoteState::Missed => {
                    missed += 1;
                    continue;
                }
                _ => continue,
            };
            waiting.push((
                vote.deadline,
                PendingItem {
                    id: vote.id.clone(),
                    property: vote.property.clone(),
                    subject: quoted(&vote.subject, EXCERPT),
                    state: wire(&vote.state),
                    waiting_on: waiting_on.into(),
                    deadline: vote.deadline.map(rfc3339),
                    hours_left: vote
                        .deadline
                        .map(|d| ((d - now).num_seconds() as f64 / 360.0).round() / 10.0),
                    recommended: vote
                        .recommendation
                        .as_ref()
                        .map(|r| quoted(&r.position, EXCERPT)),
                    decided: decided(&vote),
                },
            ));
        }
        // Soonest deadline first, and those with none after them.
        waiting.sort_by_key(|(deadline, _)| (deadline.is_none(), *deadline));
        let items = waiting.into_iter().map(|(_, item)| item).collect();
        ToolOutput::new(&PendingOut {
            missed,
            scanned: Scanned::new(last_scan, page.page(items, "limit")?),
        })
    }

    fn votes(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args: VoteArgs = parse_args(args)?;
        let property = trimmed(args.property.as_deref());
        let wanted_vote = trimmed(args.vote.as_deref());
        let page = PageRequest::new(&args.list, &(args.state, property, wanted_vote))?;
        let detailed = args.list.format().is_detailed();
        let store = self.store()?;
        let votes = store.votes().map_err(|e| store_error("votes list", e))?;
        let last_scan = Self::last_scan(&store, &votes)?;
        if let Some(query) = property {
            let corpus = store.corpus().map_err(|e| store_error("digest", e))?;
            Self::known_property(&votes, &corpus, query)?;
        }
        let now = crate::clock::now();
        Self::known_vote(&votes, wanted_vote)?;
        let mut items = Vec::new();
        for vote in &votes {
            let state = state_now(vote, now);
            if args.state.is_some_and(|wanted| wanted != state)
                || property.is_some_and(|p| !matches_property(&vote.property, p))
                || wanted_vote.is_some_and(|id| id != vote.id)
            {
                continue;
            }
            let recommendation = vote.recommendation.as_ref();
            items.push(VoteItem {
                id: vote.id.clone(),
                property: vote.property.clone(),
                subject: quoted(&vote.subject, EXCERPT),
                state: wire(&state),
                deadline: vote.deadline.map(rfc3339),
                recommended: recommendation.map(|r| quoted(&r.position, EXCERPT)),
                decided: decided(vote),
                outcome: vote.outcome.as_deref().map(|o| quoted(o, EXCERPT)),
                proposal: detailed.then(|| quoted(&vote.proposal, DETAILED_EXCERPT)),
                options: detailed.then(|| vote.options.iter().map(|o| without_links(o)).collect()),
                tier: vote.tier.filter(|_| detailed).map(|t| wire(&t)),
                confidence: recommendation.filter(|_| detailed).map(|r| r.confidence),
                reasoning: recommendation
                    .filter(|_| detailed)
                    .map(|r| quoted(&r.reasoning, DETAILED_EXCERPT)),
                detected: detailed.then(|| day(vote.created_at)),
            });
        }
        ToolOutput::new(&Scanned::new(
            last_scan,
            page.page(items, "state or property")?,
        ))
    }

    fn digest(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args: DigestArgs = parse_args(args)?;
        let since = trimmed(args.since.as_deref());
        let property = trimmed(args.property.as_deref());
        let cutoff = since.map(parse_since).transpose()?;
        let page = PageRequest::new(&args.list, &(since, property))?;
        let length = if args.list.format().is_detailed() {
            DETAILED_EXCERPT
        } else {
            EXCERPT
        };
        let store = self.store()?;
        let votes = store.votes().map_err(|e| store_error("votes list", e))?;
        let last_scan = Self::last_scan(&store, &votes)?;
        let mut filed = store.corpus().map_err(|e| store_error("digest", e))?;
        if let Some(query) = property {
            Self::known_property(&votes, &filed, query)?;
        }

        filed.retain(|item| {
            cutoff.is_none_or(|c| item.date >= c)
                && property.is_none_or(|p| matches_property(&item.property, p))
        });
        filed.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.msg_id.cmp(&b.msg_id)));
        let items = filed
            .into_iter()
            .map(|item| DigestItem {
                date: day(item.date),
                property: item.property,
                kind: item.kind,
                subject: quoted(&item.subject, EXCERPT),
                excerpt: quoted(&item.body, length),
            })
            .collect();
        ToolOutput::new(&Scanned::new(
            last_scan,
            page.page(items, "since or property")?,
        ))
    }

    fn ledger(&self, args: Value) -> Result<ToolOutput, ToolError> {
        let args: LedgerArgs = parse_args(args)?;
        let wanted_vote = trimmed(args.vote.as_deref());
        let page = PageRequest::new(&args.list, &wanted_vote)?;
        let detailed = args.list.format().is_detailed();
        let store = self.store()?;
        let votes = store.votes().map_err(|e| store_error("votes list", e))?;
        let last_scan = Self::last_scan(&store, &votes)?;
        Self::known_vote(&votes, wanted_vote)?;

        let mut entries = store.ledger().map_err(|e| store_error("ledger", e))?;
        entries.retain(|e| wanted_vote.is_none_or(|id| e.vote_id.as_deref() == Some(id)));
        // The file is oldest first; the newest is what a caller wants first.
        entries.reverse();
        let items = entries
            .into_iter()
            .map(|e| LedgerItem {
                at: rfc3339(e.at),
                event: e.event,
                vote: e.vote_id,
                to: e.to.map(|s| wire(&s)),
                actor: detailed.then(|| wire(&e.actor)),
                from: e.from.filter(|_| detailed).map(|s| wire(&s)),
                detail: e
                    .detail
                    .filter(|_| detailed)
                    .map(|d| quoted(&d, DETAILED_EXCERPT)),
            })
            .collect();
        ToolOutput::new(&Scanned::new(last_scan, page.page(items, "vote")?))
    }
}

/// What the user decided: the position, or the action when none was taken.
fn decided(vote: &VoteRecord) -> Option<String> {
    let decision = vote.decision.as_ref()?;
    let said = decision.position.as_deref().unwrap_or(&decision.action);
    Some(quoted(said, EXCERPT))
}

impl Toolset for PmTools {
    fn server(&self) -> Server {
        self.server.clone()
    }

    fn tools(&self) -> Vec<ToolDef> {
        let mut tools = vec![
            ToolDef::read(
                "loftypm_pending",
                "Lofty votes waiting on someone",
                "Lists the Lofty governance votes someone has yet to act on, soonest deadline \
                 first. Use it for what needs the user's attention. Returns last_scan, missed \
                 (deadlines that passed with no decision) and items with id, property, subject, \
                 state, waiting_on, deadline, hours_left, recommended and decided.",
            )
            .input::<PendingArgs>()
            .output::<PendingOut>(),
            ToolDef::read(
                "loftypm_votes",
                "Tracked Lofty votes",
                "Lists every tracked Lofty governance vote, newest first, or one by id. Use it to \
                 see what a proposal says or how a vote went. Items carry id, property, subject, \
                 state, deadline, recommended, decided and outcome (detailed adds proposal, \
                 options, tier, confidence, reasoning and detected). It gives no voting link.",
            )
            .input::<VoteArgs>()
            .output::<Scanned<VoteItem>>(),
            ToolDef::read(
                "loftypm_digest",
                "Lofty property updates",
                "Lists the update and result emails filed for the user's Lofty properties, newest \
                 first. Use it for what has happened at a property. Items carry date, property, \
                 kind, subject and an excerpt, longer when detailed. Narrow with since or property.",
            )
            .input::<DigestArgs>()
            .output::<Scanned<DigestItem>>(),
            ToolDef::read(
                "loftypm_ledger",
                "Lofty vote audit trail",
                "Lists the audit trail of what happened to tracked votes and when, newest first. \
                 Use it for who decided what. Items carry at, event, vote and to (detailed adds \
                 actor, from and detail). Narrow with vote.",
            )
            .input::<LedgerArgs>()
            .output::<Scanned<LedgerItem>>(),
        ];
        tools.iter_mut().for_each(tighten);
        tools
    }

    async fn call(&self, name: &str, args: Value) -> Result<ToolOutput, ToolError> {
        match name {
            "loftypm_pending" => self.pending(args),
            "loftypm_votes" => self.votes(args),
            "loftypm_digest" => self.digest(args),
            "loftypm_ledger" => self.ledger(args),
            _ => Err(ToolError::other(format!("unknown tool `{name}`"))),
        }
    }
}

/// The shared argument descriptions take a quarter of the 8,000-character budget
/// the definitions ride along in every prompt under.
fn tighten(tool: &mut ToolDef) {
    for (arg, text) in [
        (
            "cursor",
            "A result's next_cursor, to continue the same query.",
        ),
        ("response_format", "`detailed` adds the fuller fields."),
    ] {
        if let Some(description) = tool
            .input_schema
            .pointer_mut(&format!("/properties/{arg}/description"))
        {
            *description = Value::String(text.into());
        }
    }
}

#[cfg(test)]
mod tests;
