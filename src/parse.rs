//! Classifier and field extractors for Lofty governance emails.
//!
//! Grounded in real Lofty vote/result/update emails captured from a live
//! investor mailbox and sanitized (2026-07-27) — see
//! `tests/fixtures/emails/{vote,result}-*.txt` and `update-pm.txt`. The
//! real subject/body shapes turned out not to match the originally
//! invented fixtures at all (no colon in vote/result subjects; property
//! name is a dash-segmented middle field; no machine-parseable deadline
//! or `Options:` list; the vote link is a Typeform URL, not a
//! `/governance/vote/` path; results state the outcome as "The winning
//! vote is:" rather than "passed"/"failed"). The classification/
//! extraction regexes below target that real shape first, falling back
//! to the original synthetic patterns in case Lofty ever uses (or
//! reverts to) that template for some sender/campaign.
//!
//! Parser failure is safe by design: `classify` never panics on
//! arbitrary input. A `Classified::Unrelated`, or a `Vote`/`Update`/
//! `VoteResult` with missing/empty fields (e.g. no parseable
//! deadline), is expected — callers from Task 6 on route those to a
//! `needs-manual-parse` state rather than treating them as errors.

use std::sync::LazyLock;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use regex::Regex;

use crate::gmail::RawEmail;
use crate::model::Tier;

/// Classification result for a `RawEmail`.
#[derive(Debug, Clone)]
pub enum Classified {
    Vote(VoteEmail),
    Update(UpdateEmail),
    VoteResult(ResultEmail),
    Unrelated,
}

/// A new (or still-open) governance vote.
#[derive(Debug, Clone)]
pub struct VoteEmail {
    pub property_slug: String,
    #[allow(dead_code)] // surfaced once recommend renders friendly names
    pub property_name: String,
    pub proposal: String,
    pub options: Vec<String>,
    pub deadline: Option<DateTime<Utc>>,
    pub vote_link: Option<String>,
}

/// A non-actionable property update (financials, maintenance notice, ...).
#[derive(Debug, Clone)]
pub struct UpdateEmail {
    pub property_slug: String,
    #[allow(dead_code)] // surfaced once recommend renders friendly names
    pub property_name: String,
    #[allow(dead_code)] // corpus-driven recommendations use the subtype later
    pub kind: String,
}

/// The closed-vote outcome announcement.
#[derive(Debug, Clone)]
pub struct ResultEmail {
    pub property_slug: String,
    #[allow(dead_code)] // surfaced once recommend renders friendly names
    pub property_name: String,
    pub outcome: String,
}

// ---- subject-line classification (checked first) ----
//
// Real subjects (no colon at all, property is a dash-segmented middle
// field):
//   [Action Required] Governance Vote - <property> - <topic>
//   [Action Required] Owner-Proposed Governance Vote - <property> - <topic>
//   Governance Results - <property> - <outcome>
//   Owner-Proposed Governance Results - <property> - <outcome>
//   <property> - Update from Property Manager
// The original synthetic `New Governance Vote: ...`/`Vote Results: ...`/
// `Property Update: ...` patterns never matched anything Lofty actually
// sends; kept below as fallbacks, cheap insurance in case Lofty uses that
// shape for some other sender/template over time.

static VOTE_SUBJECT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)governance vote|^new governance vote:").unwrap());
static RESULT_SUBJECT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)governance results?|^vote results?:").unwrap());
static UPDATE_SUBJECT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)update from property manager|^property update:").unwrap());

// ---- body-keyword fallback (only when the subject matches none of the
// above — e.g. a forwarded email with a mangled subject line) ----

static VOTE_BODY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)cast your vote|voting ends|options:|vote now|governance vote for"#).unwrap()
});
static RESULT_BODY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)vote results|passed with|failed with|approval\b|the winning vote is|governance results for",
    )
    .unwrap()
});
static UPDATE_BODY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)financial summary|rent collected|reserve balance|update from property manager|property manager has shared the following update",
    )
    .unwrap()
});

// ---- field extraction ----

/// Property name out of a real vote/result subject: the dash-segmented
/// field right after the literal "Governance Vote"/"Governance Results"
/// (itself possibly prefixed by `[Action Required] `/`Owner-Proposed `).
static VOTE_SUBJECT_DASH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)governance vote\s*-\s*(?P<property>.+?)\s*-\s*.+$").unwrap());
static RESULT_SUBJECT_DASH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)governance results?\s*-\s*(?P<property>.+?)\s*-\s*.+$").unwrap()
});
/// Property name out of a real update subject: everything before the
/// literal `- Update from Property Manager` suffix.
static UPDATE_SUBJECT_DASH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?P<property>.+?)\s*-\s*update from property manager\s*$").unwrap()
});

static DEADLINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)Voting ends:?\s*(.+)").unwrap());
static PACIFIC_TZ_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(PT|PST|PDT)\b").unwrap());
// Only strip a trailing token that's a *known* timezone abbreviation —
// never a generic "last word", or "11:59 PM" (no explicit tz) would lose
// its AM/PM and fail to parse.
static TRAILING_TZ_ABBREV_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s+(PST|PDT|PT|EST|EDT|ET|CST|CDT|CT|MST|MDT|MT|UTC|GMT)$").unwrap()
});
static URL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"https?://\S+").unwrap());
/// The real "Vote Now ( <url> )" call-to-action link — the actual
/// Typeform URL owners cast a vote through.
static VOTE_NOW_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)Vote Now\s*\(\s*(?P<url>https?://\S+?)\s*\)"#).unwrap());
/// Real result bodies state the outcome as a "The winning vote is:"
/// marker line followed (after a blank line) by the outcome text itself,
/// e.g. "Yes. Accept the offer for $180,000." — never "passed"/"failed".
static WINNING_VOTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^the winning vote is:?$").unwrap());
static OUTCOME_KEYWORDS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)passed|failed|approved|rejected").unwrap());
static AMOUNT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$[\d,]+(\.\d+)?").unwrap());
static HIGH_STAKES_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)sell|sale of|for sale|refinanc|special assessment|capital call|dissolv|listing price|asking price|received an offer|accept(?:ing)? the offer|under contract",
    )
    .unwrap()
});

/// Classify a raw email by subject regex first, falling back to body
/// keywords. Never panics; unmatched/unparseable input safely returns
/// `Classified::Unrelated`.
pub fn classify(email: &RawEmail) -> Classified {
    if VOTE_SUBJECT_RE.is_match(&email.subject) {
        return Classified::Vote(parse_vote(email));
    }
    if RESULT_SUBJECT_RE.is_match(&email.subject) {
        return Classified::VoteResult(parse_result(email));
    }
    if UPDATE_SUBJECT_RE.is_match(&email.subject) {
        return Classified::Update(parse_update(email));
    }

    // Subject didn't match a known pattern — fall back to body keywords.
    if VOTE_BODY_RE.is_match(&email.body) {
        return Classified::Vote(parse_vote(email));
    }
    if RESULT_BODY_RE.is_match(&email.body) {
        return Classified::VoteResult(parse_result(email));
    }
    if UPDATE_BODY_RE.is_match(&email.body) {
        return Classified::Update(parse_update(email));
    }

    Classified::Unrelated
}

fn parse_vote(email: &RawEmail) -> VoteEmail {
    let property_name = extract_property_name(&email.subject);
    VoteEmail {
        property_slug: slugify(&property_name),
        property_name,
        proposal: extract_proposal(&email.body),
        options: extract_options(&email.body),
        deadline: extract_deadline(&email.body),
        vote_link: extract_vote_link(&email.body),
    }
}

fn parse_update(email: &RawEmail) -> UpdateEmail {
    let property_name = extract_property_name(&email.subject);
    UpdateEmail {
        property_slug: slugify(&property_name),
        property_name,
        kind: extract_update_kind(&email.subject),
    }
}

fn parse_result(email: &RawEmail) -> ResultEmail {
    let property_name = extract_property_name(&email.subject);
    ResultEmail {
        property_slug: slugify(&property_name),
        property_name,
        outcome: extract_outcome(&email.body),
    }
}

/// Property name from the subject line. Tries the real dash-segmented
/// vote/result shape first (`Governance Vote - <property> - <topic>` /
/// `Governance Results - <property> - <outcome>`, either possibly
/// `Owner-Proposed`-prefixed), then the real update shape (`<property> -
/// Update from Property Manager`), then falls back to the original
/// synthetic `<Prefix>: <property> - <topic>` colon shape. No pattern at
/// all (shouldn't happen for a real Lofty subject) → the whole trimmed
/// subject, defensively.
fn extract_property_name(subject: &str) -> String {
    if let Some(c) = VOTE_SUBJECT_DASH_RE.captures(subject) {
        return c["property"].trim().to_string();
    }
    if let Some(c) = RESULT_SUBJECT_DASH_RE.captures(subject) {
        return c["property"].trim().to_string();
    }
    if let Some(c) = UPDATE_SUBJECT_DASH_RE.captures(subject) {
        return c["property"].trim().to_string();
    }

    let tail = match subject.split_once(':') {
        Some((_, rest)) => rest.trim(),
        None => subject.trim(),
    };
    match tail.split_once(" - ") {
        Some((name, _)) => name.trim().to_string(),
        None => tail.to_string(),
    }
}

/// The subject text after the first ` - ` (the update's descriptive
/// tail — for a real update subject this is the literal "Update from
/// Property Manager"; for the old synthetic shape it was e.g. "June
/// Financials"); `"update"` if there is none.
fn extract_update_kind(subject: &str) -> String {
    let tail = match subject.split_once(':') {
        Some((_, rest)) => rest.trim(),
        None => subject.trim(),
    };
    match tail.split_once(" - ") {
        Some((_, kind)) => kind.trim().to_string(),
        None => "update".to_string(),
    }
}

/// Body paragraph text: everything between the tolerated `--- Body ---`
/// marker (see `parse_read_output`) and the first stop line, with
/// boilerplate masthead/header lines and blank lines dropped.
///
/// Real vote emails open with a brand masthead line (`Lofty (
/// https://lofty.ai?... )`) and restate the property in a `Governance
/// Vote for <property> ( <link> ):` / `Owner-Proposed Governance Vote
/// for <property> ( <link> ):` header before the actual proposal text —
/// both are boilerplate, not proposal content, so they're skipped rather
/// than included. Extraction stops at the real `Vote Now` call-to-action
/// or signature (`Cheers, The Lofty Team`), or — for the older synthetic
/// template, kept as a fallback — `Options:`/`Voting ends`/`Cast your
/// vote`/a bare `--` signature separator.
fn extract_proposal(body: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "--- Body ---" {
            continue;
        }
        if trimmed.starts_with("Lofty (")
            || trimmed.starts_with("Governance Vote for")
            || trimmed.starts_with("Owner-Proposed Governance Vote for")
        {
            continue;
        }
        if trimmed.starts_with("Options:")
            || trimmed.starts_with("Voting ends")
            || trimmed.starts_with("Cast your vote")
            || trimmed.starts_with("Click the \"Vote Now\"")
            || trimmed.starts_with("Vote Now (")
            || trimmed.starts_with("Cheers, The Lofty Team")
            || trimmed == "--"
        {
            break;
        }
        out.push(trimmed);
    }
    out.join(" ")
}

/// The `Options:` line (old synthetic template), split on `/`, trimmed.
///
/// Real Lofty vote emails (2026-07-27 capture) carry no machine-readable
/// options list at all — owners vote informally (accept/counter/deny an
/// offer, yes/no on an owner's proposal) through the linked Typeform, not
/// a menu Lofty states in the email body. When no `Options:` line is
/// found, default to a generic Approve/Reject pair so `recommend`/
/// `decide` always have *some* options to work with; a human/agent can
/// always override the specifics via `votes amend --options`.
fn extract_options(body: &str) -> Vec<String> {
    for line in body.lines() {
        if let Some(rest) = line.trim().strip_prefix("Options:") {
            let opts: Vec<String> = rest
                .split('/')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !opts.is_empty() {
                return opts;
            }
        }
    }
    vec!["Approve".to_string(), "Reject".to_string()]
}

/// `Voting ends:?\s*(.+)`, parsed as `%B %d, %Y %I:%M %p` in
/// `America/Los_Angeles` when the line mentions PT/PST/PDT, else UTC.
/// Unparseable/ambiguous input safely returns `None`.
///
/// Real Lofty vote emails (2026-07-27 capture) don't state a deadline
/// this precisely at all — the only mention is prose like "sent to all
/// owners on Friday, July 31st once the voting period ends", with no
/// year and no time of day. That can't be turned into a trustworthy
/// `DateTime<Utc>` without guessing (wrong reminders are worse than no
/// reminders), so for that shape this intentionally returns `None`
/// rather than fabricate a time — consistent with this module's
/// documented "missing deadline is an expected, safely-handled outcome"
/// design. `votes amend --deadline` lets a human/agent set one by hand
/// once they've read the actual voting-period-end date off Lofty's site.
fn extract_deadline(body: &str) -> Option<DateTime<Utc>> {
    let line = body
        .lines()
        .find_map(|l| DEADLINE_RE.captures(l).map(|c| c[1].trim().to_string()))?;

    let is_pacific = PACIFIC_TZ_RE.is_match(&line);
    let cleaned = TRAILING_TZ_ABBREV_RE.replace(&line, "");
    let naive = NaiveDateTime::parse_from_str(cleaned.trim(), "%B %d, %Y %I:%M %p").ok()?;

    if is_pacific {
        chrono_tz::America::Los_Angeles
            .from_local_datetime(&naive)
            .single()
            .map(|dt| dt.with_timezone(&Utc))
    } else {
        Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
    }
}

/// The real "Vote Now ( <url> )" call-to-action link if present (the
/// actual Typeform URL owners vote through); else the first
/// `https?://` URL that looks like a governance vote link (the older
/// synthetic template's shape); else the first URL in the body,
/// generically.
fn extract_vote_link(body: &str) -> Option<String> {
    if let Some(c) = VOTE_NOW_URL_RE.captures(body) {
        let url = c["url"].trim_end_matches(['.', ',', ')', ']', '!']);
        return Some(url.to_string());
    }

    let mut fallback = None;
    for m in URL_RE.find_iter(body) {
        let url = m.as_str().trim_end_matches(['.', ',', ')', ']', '!']);
        if url.contains("/governance/vote/") {
            return Some(url.to_string());
        }
        if fallback.is_none() {
            fallback = Some(url.to_string());
        }
    }
    fallback
}

/// The outcome text. Real result emails state it as a "The winning vote
/// is:" marker line followed by the actual outcome text (e.g. "Yes.
/// Accept the offer for $180,000.") — checked first. Falls back to the
/// first non-blank body line mentioning an old-template pass/fail-style
/// outcome keyword, then to the proposal-style paragraph text if neither
/// is found anywhere.
fn extract_outcome(body: &str) -> String {
    let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty());
    while let Some(line) = lines.next() {
        if WINNING_VOTE_RE.is_match(line) {
            if let Some(next) = lines.next() {
                return next.to_string();
            }
        }
    }

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "--- Body ---" {
            continue;
        }
        if OUTCOME_KEYWORDS_RE.is_match(trimmed) {
            return trimmed.to_string();
        }
    }
    extract_proposal(body)
}

/// First `$1,234.56`-style amount in `proposal`, for feeding
/// [`default_tier`].
pub fn extract_amount(proposal: &str) -> Option<f64> {
    let m = AMOUNT_RE.find(proposal)?;
    m.as_str()
        .trim_start_matches('$')
        .replace(',', "")
        .parse()
        .ok()
}

/// Routine vs. high-stakes heuristic: a high-stakes keyword match on
/// the proposal text, or a dollar amount over `high_stakes_usd`. The
/// keyword list covers both maintenance/capital-event language (roof,
/// refinance, special assessment, ...) and the real sale-adjacent
/// vocabulary Lofty's own governance emails use (an offer *received* on
/// the property, changing the *listing*/*asking* price, a property
/// going *under contract*) — a property sale is inherently high-stakes
/// even when the specific dollar figure in the proposal text doesn't
/// clear `high_stakes_usd`.
pub fn default_tier(proposal: &str, amount: Option<f64>, high_stakes_usd: f64) -> Tier {
    if HIGH_STAKES_RE.is_match(proposal) {
        return Tier::HighStakes;
    }
    if amount.is_some_and(|a| a > high_stakes_usd) {
        Tier::HighStakes
    } else {
        Tier::Routine
    }
}

/// Lowercase, ASCII-alphanumeric runs joined by `-`. Non-ASCII
/// characters and punctuation are separators (dropped, not
/// transliterated) — e.g. `"  Élm St. #12 "` -> `"lm-st-12"`.
pub fn slugify(input: &str) -> String {
    let lower = input.to_lowercase();
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            current.push(c);
        } else if !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts.join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(name: &str) -> RawEmail {
        let raw = std::fs::read_to_string(format!("tests/fixtures/emails/{name}.txt")).unwrap();
        crate::gmail::parse_read_output(&format!("fixt-{name}"), &raw).unwrap()
    }

    #[test]
    fn classifies_standard_vote_offer_received() {
        match classify(&load("vote-roof")) {
            Classified::Vote(v) => {
                assert_eq!(v.property_slug, "742-birchwood-ave");
                assert_eq!(v.property_name, "742 Birchwood Ave");
                assert!(v.proposal.contains("received an offer for $150,000"));
                // Real vote bodies carry no `Options:` list — default pair.
                assert_eq!(v.options, vec!["Approve", "Reject"]);
                // No year/time in the real deadline phrasing => None, by design.
                assert_eq!(v.deadline, None);
                assert_eq!(
                    v.vote_link.as_deref(),
                    Some("https://loftyai.typeform.com/to/EXAMPLEVOTE01#userid=xxxxx")
                );
            }
            other => panic!("expected Vote, got {other:?}"),
        }
    }

    #[test]
    fn classifies_owner_proposed_vote() {
        match classify(&load("vote-owner-proposed")) {
            Classified::Vote(v) => {
                assert_eq!(v.property_slug, "742-birchwood-ave");
                assert!(v
                    .proposal
                    .contains("proposing a governance vote to change the listing price"));
                assert_eq!(v.options, vec!["Approve", "Reject"]);
                assert_eq!(
                    v.vote_link.as_deref(),
                    Some("https://loftyai.typeform.com/to/EXAMPLEVOTE02#userid=xxxxx")
                );
            }
            other => panic!("expected Vote, got {other:?}"),
        }
    }

    /// The property-extraction dash regex is lazy specifically so a topic
    /// segment that itself contains " - " (e.g. an urgency suffix a sender
    /// tacks on) doesn't get swallowed into the property name — it should
    /// still stop at the *first* dash after the property.
    #[test]
    fn topic_containing_dash_does_not_break_property_extraction() {
        let email = RawEmail {
            id: "vote-dash-topic".into(),
            from: "Lofty <info@lofty.ai>".into(),
            subject:
                "[Action Required] Governance Vote - 742 Birchwood Ave - Change Listing Price - Urgent"
                    .into(),
            date: None,
            body: "Vote Now ( https://loftyai.typeform.com/to/EXAMPLE#userid=xxxxx )\n".into(),
        };
        match classify(&email) {
            Classified::Vote(v) => assert_eq!(v.property_slug, "742-birchwood-ave"),
            other => panic!("expected Vote, got {other:?}"),
        }
    }

    #[test]
    fn classifies_standard_result_offer_accepted() {
        match classify(&load("result-roof")) {
            Classified::VoteResult(r) => {
                assert_eq!(r.property_slug, "742-birchwood-ave");
                assert_eq!(r.outcome, "Yes. Accept the offer for $180,000.");
            }
            other => panic!("expected VoteResult, got {other:?}"),
        }
    }

    #[test]
    fn classifies_owner_proposed_result() {
        match classify(&load("result-owner-proposed")) {
            Classified::VoteResult(r) => {
                assert_eq!(r.property_slug, "742-birchwood-ave");
                assert_eq!(r.outcome, "Yes, lower the listing price to $190,000.");
            }
            other => panic!("expected VoteResult, got {other:?}"),
        }
    }

    #[test]
    fn classifies_property_manager_update() {
        match classify(&load("update-pm")) {
            Classified::Update(u) => {
                assert_eq!(u.property_slug, "18-quarry-ridge-ct");
                assert_eq!(u.property_name, "18 Quarry Ridge Ct");
                assert_eq!(u.kind, "Update from Property Manager");
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn unrelated_falls_through() {
        assert!(matches!(
            classify(&load("unrelated")),
            Classified::Unrelated
        ));
    }

    #[test]
    fn tier_heuristic() {
        assert_eq!(
            default_tier("Replace hallway lighting", Some(800.0), 5000.0),
            Tier::Routine
        );
        assert_eq!(
            default_tier("Special assessment for roof", Some(12500.0), 5000.0),
            Tier::HighStakes
        );
        assert_eq!(
            default_tier("Proposal to sell the property", None, 5000.0),
            Tier::HighStakes
        );
        // Real-world sale-adjacent vocabulary (item 3): high-stakes even
        // when the dollar figure alone wouldn't clear the threshold.
        assert_eq!(
            default_tier(
                "The property received an offer for $1,000.",
                Some(1_000.0),
                5000.0
            ),
            Tier::HighStakes
        );
        assert_eq!(
            default_tier(
                "Owners are asked to vote on changing the listing price.",
                None,
                5000.0
            ),
            Tier::HighStakes
        );
    }

    #[test]
    fn slugify_is_stable() {
        assert_eq!(slugify("1500 Lakeview Ave"), "1500-lakeview-ave");
        assert_eq!(slugify("  Élm St. #12 "), "lm-st-12");
    }

    // ---- old-template fallback coverage (cheap: literal strings, no
    // fixture files) — Lofty may still use (or revert to) this shape for
    // some sender/template, so the fallback regexes stay exercised. ----

    #[test]
    fn old_synthetic_deadline_format_still_parses() {
        let body = "Voting ends: July 28, 2026 11:59 PM PT\n";
        let d = extract_deadline(body).expect("old-format deadline should still parse");
        assert_eq!(d.format("%Y-%m-%d").to_string(), "2026-07-29"); // 11:59 PM PT = 06:59 UTC next day
    }

    #[test]
    fn old_synthetic_options_line_still_parses() {
        let body = "Options: Approve / Reject / Defer to Property Manager\n";
        assert_eq!(
            extract_options(body),
            vec!["Approve", "Reject", "Defer to Property Manager"]
        );
    }

    #[test]
    fn old_synthetic_governance_vote_link_still_preferred() {
        let body = "Cast your vote here: https://www.lofty.ai/governance/vote/fixt123\n";
        assert_eq!(
            extract_vote_link(body).as_deref(),
            Some("https://www.lofty.ai/governance/vote/fixt123")
        );
    }

    #[test]
    fn old_synthetic_outcome_keyword_still_parses() {
        let body = "The proposal \"Roof Replacement\" passed with 74% approval.\n";
        assert_eq!(
            extract_outcome(body),
            "The proposal \"Roof Replacement\" passed with 74% approval."
        );
    }

    #[test]
    fn body_fallback_classifies_vote_with_mangled_subject() {
        let email = RawEmail {
            id: "mangled-vote".into(),
            from: "Lofty <info@lofty.ai>".into(),
            subject: "Fwd: FW: (no subject)".into(),
            date: None,
            body: "Governance Vote for 742 Birchwood Ave ( https://example.com ):\n\n\
                   Click the \"Vote Now\" button below to vote:\n\n\
                   Vote Now ( https://loftyai.typeform.com/to/EXAMPLE#userid=xxxxx )\n"
                .into(),
        };
        assert!(matches!(classify(&email), Classified::Vote(_)));
    }

    #[test]
    fn body_fallback_classifies_result_with_mangled_subject() {
        let email = RawEmail {
            id: "mangled-result".into(),
            from: "Lofty <info@lofty.ai>".into(),
            subject: "Fwd: FW: (no subject)".into(),
            date: None,
            body: "Governance Results for 742 Birchwood Ave ( https://example.com ):\n\n\
                   The winning vote is:\n\nYes.\n"
                .into(),
        };
        assert!(matches!(classify(&email), Classified::VoteResult(_)));
    }

    #[test]
    fn body_fallback_classifies_update_with_mangled_subject() {
        let email = RawEmail {
            id: "mangled-update".into(),
            from: "Lofty <info@lofty.ai>".into(),
            subject: "Fwd: FW: (no subject)".into(),
            date: None,
            body: "The property manager has shared the following update:\n\nAll is well.\n".into(),
        };
        assert!(matches!(classify(&email), Classified::Update(_)));
    }
}
