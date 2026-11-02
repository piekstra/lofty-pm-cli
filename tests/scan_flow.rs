//! End-to-end scan against the stub gro and a temp XDG home. Offline.

use assert_cmd::Command;
use std::path::PathBuf;

fn stub_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// Default `LOFTY_PM_NOW` used by `pm`/`pm_at`, so the bulk of tests (which
/// don't care about the clock) don't have to think about it.
const DEFAULT_NOW: &str = "2026-07-24T12:00:00Z";

/// Build a lofty-pm Command wired to temp config+data dirs and the gro stub.
fn pm(tmp: &std::path::Path, ids: &str) -> Command {
    pm_at_with(tmp, ids, DEFAULT_NOW, &serde_json::json!({}))
}

/// Like `pm`, but with an explicit `LOFTY_PM_NOW` (RFC 3339) instead of the
/// default — for reminder/deadline tests that need to move the clock.
fn pm_at(tmp: &std::path::Path, ids: &str, now: &str) -> Command {
    pm_at_with(tmp, ids, now, &serde_json::json!({}))
}

/// The common builder underlying `pm` and `pm_at`: explicit clock value
/// plus extra config.json keys, for a test that needs to merge in more
/// than `pm`/`pm_at` alone provide (call this directly with a non-empty
/// `extra` object).
///
/// Baseline `lofty_bin` points at a nonexistent path so every test is
/// deterministic regardless of the developer's real `lofty` install/auth
/// state (a real `lofty` on `$PATH` would otherwise make `lofty_present`/
/// `lofty_auth`-touching commands machine-dependent). Override it via
/// `extra` for a test that wants a working `lofty` stub.
fn pm_at_with(tmp: &std::path::Path, ids: &str, now: &str, extra: &serde_json::Value) -> Command {
    let cfg_dir = tmp.join("cfg/lofty-pm");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let mut config = serde_json::json!({
        "gro_bin": stub_dir().join("stubs/gro").to_str().unwrap(),
        "lofty_bin": "/nonexistent/lofty",
    });
    if let (Some(base), Some(extra)) = (config.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    std::fs::write(cfg_dir.join("config.json"), config.to_string()).unwrap();
    let mut c = Command::cargo_bin("lofty-pm").unwrap();
    c.env("XDG_CONFIG_HOME", tmp.join("cfg"))
        .env("LOFTY_PM_DATA_DIR", tmp.join("data"))
        .env("GRO_STUB_DIR", stub_dir().join("fixtures/emails"))
        .env("GRO_STUB_IDS", ids)
        .env("LOFTY_PM_NOW", now);
    c
}

/// Wraps Task 8's recommend invocation: position "Reject", confidence 0.8,
/// tier high-stakes, reasoning from a temp file.
fn recommend(tmp: &std::path::Path, vote_id: &str) {
    let reasoning = tmp.join(format!("{vote_id}-reasoning.md"));
    std::fs::write(
        &reasoning,
        "Reserves already depleted; stance is income-first.",
    )
    .unwrap();
    pm(tmp, "")
        .args([
            "recommend",
            vote_id,
            "--position",
            "Reject",
            "--confidence",
            "0.8",
            "--tier",
            "high-stakes",
            "--reasoning-file",
            reasoning.to_str().unwrap(),
        ])
        .assert()
        .success();
}

/// Runs `--json votes show <id>` and returns the vote's state string.
fn show_state(tmp: &std::path::Path, vote_id: &str) -> String {
    let out = pm(tmp, "")
        .args(["--json", "votes", "show", vote_id])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    v["state"].as_str().unwrap().to_string()
}

#[test]
fn scan_files_vote_update_and_dedups_on_rerun() {
    let tmp = tempfile::tempdir().unwrap();
    let out = pm(tmp.path(), "vote-roof update-pm unrelated")
        .args(["--json", "scan"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "scan-report/v1");
    assert_eq!(v["new_votes"].as_array().unwrap().len(), 1);
    assert_eq!(v["new_updates"], 1);

    // second run: everything already processed
    let out2 = pm(tmp.path(), "vote-roof update-pm unrelated")
        .args(["--json", "scan"])
        .assert()
        .success();
    let v2: serde_json::Value = serde_json::from_slice(&out2.get_output().stdout).unwrap();
    assert_eq!(v2["new_votes"].as_array().unwrap().len(), 0);
    assert_eq!(v2["skipped"], 3);
}

#[test]
fn scan_closes_vote_on_result_email() {
    let tmp = tempfile::tempdir().unwrap();
    pm(tmp.path(), "vote-roof")
        .args(["--json", "scan"])
        .assert()
        .success();
    let out = pm(tmp.path(), "vote-roof result-roof")
        .args(["--json", "scan"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["closed"].as_array().unwrap().len(), 1);
}

#[test]
fn scan_marks_missed_after_deadline() {
    let tmp = tempfile::tempdir().unwrap();
    pm(tmp.path(), "vote-roof")
        .args(["--json", "scan"])
        .assert()
        .success();
    // The real vote-roof template states its deadline in prose with no
    // year or exact time (see parse.rs's extract_deadline doc comment), so
    // it's intentionally left unparsed (`deadline: None`) rather than
    // guessed. Inject one by hand via `votes amend` — exactly the
    // human/agent fix-up path this design expects — so the deadline sweep
    // has something concrete to test.
    pm(tmp.path(), "")
        .args([
            "votes",
            "amend",
            "v-vote-roof",
            "--deadline",
            "2026-07-29T06:59:00Z",
        ])
        .assert()
        .success();
    // jump past the deadline just injected
    let mut c = pm(tmp.path(), "");
    c.env("LOFTY_PM_NOW", "2026-08-01T00:00:00Z");
    let out = c.args(["--json", "scan"]).assert().success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["missed"].as_array().unwrap().len(), 1);
}

/// A single unreadable/unparseable message must not abort the batch or
/// permanently wedge the scan: it's noted in `manual_parse` and left
/// unprocessed (so it's retried, not silently skipped) while every other
/// id in the batch still gets filed normally.
#[test]
fn scan_continues_past_unparseable_email() {
    let tmp = tempfile::tempdir().unwrap();
    let out = pm(tmp.path(), "broken vote-roof")
        .args(["--json", "scan"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    let new_votes: Vec<&str> = v["new_votes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(new_votes, vec!["v-vote-roof"]);
    let manual: Vec<&str> = v["manual_parse"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(manual, vec!["broken"]);

    // second run, same ids: `broken` was never marked processed, so it's
    // surfaced (and fails) again, while `vote-roof` is now skipped as
    // already-processed.
    let out2 = pm(tmp.path(), "broken vote-roof")
        .args(["--json", "scan"])
        .assert()
        .success();
    let v2: serde_json::Value = serde_json::from_slice(&out2.get_output().stdout).unwrap();
    let manual2: Vec<&str> = v2["manual_parse"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap())
        .collect();
    assert_eq!(manual2, vec!["broken"]);
    assert_eq!(v2["skipped"], 1);
}

#[test]
fn recommend_then_context_reflects_it_and_gates_double_recommend() {
    let tmp = tempfile::tempdir().unwrap();
    pm(tmp.path(), "vote-roof")
        .args(["--json", "scan"])
        .assert()
        .success();
    let reasoning = tmp.path().join("r.md");
    std::fs::write(
        &reasoning,
        "Reserves already depleted; stance is income-first.",
    )
    .unwrap();
    pm(tmp.path(), "")
        .args([
            "recommend",
            "v-vote-roof",
            "--position",
            "Reject",
            "--confidence",
            "0.8",
            "--tier",
            "high-stakes",
            "--reasoning-file",
            reasoning.to_str().unwrap(),
        ])
        .assert()
        .success();
    let out = pm(tmp.path(), "")
        .args(["--json", "votes", "show", "v-vote-roof"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["state"], "recommended");
    assert_eq!(v["recommendation"]["position"], "Reject");
    // second recommend on a recommended vote is an illegal transition → exit 2
    pm(tmp.path(), "")
        .args([
            "recommend",
            "v-vote-roof",
            "--position",
            "Approve",
            "--confidence",
            "0.9",
            "--tier",
            "routine",
            "--reasoning-file",
            reasoning.to_str().unwrap(),
        ])
        .assert()
        .code(2);
}

#[test]
fn context_is_degraded_without_lofty_cli() {
    let tmp = tempfile::tempdir().unwrap();
    // `pm`'s baseline config already points `lofty_bin` at a nonexistent
    // path (see `pm_at_with`'s doc comment), so this is deterministic with
    // no per-test override needed.
    pm(tmp.path(), "vote-roof")
        .args(["--json", "scan"])
        .assert()
        .success();
    let out = pm(tmp.path(), "")
        .args(["--json", "votes", "context", "v-vote-roof"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["degraded"], true);
}

#[test]
fn full_decide_accept_cast_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    pm(tmp.path(), "vote-roof")
        .args(["--json", "scan"])
        .assert()
        .success();
    recommend(tmp.path(), "v-vote-roof"); // helper wrapping Task 8's invocation
    let out = pm(tmp.path(), "")
        .args(["--json", "decide", "v-vote-roof", "--accept"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "decision/v1");
    // Real vote emails link out to a Typeform, not a `/governance/vote/`
    // path (see parse.rs's extract_vote_link doc comment).
    assert!(v["vote_link"]
        .as_str()
        .unwrap()
        .contains("loftyai.typeform.com"));
    pm(tmp.path(), "")
        .args(["votes", "mark-cast", "v-vote-roof"])
        .assert()
        .success();
    let s = show_state(tmp.path(), "v-vote-roof");
    assert_eq!(s, "cast-confirmed");
}

#[test]
fn decline_closes_without_casting() {
    let tmp = tempfile::tempdir().unwrap();
    pm(tmp.path(), "vote-roof")
        .args(["--json", "scan"])
        .assert()
        .success();
    recommend(tmp.path(), "v-vote-roof");
    pm(tmp.path(), "")
        .args(["decide", "v-vote-roof", "--decline"])
        .assert()
        .success();
    let out = pm(tmp.path(), "")
        .args(["--json", "votes", "show", "v-vote-roof"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["state"], "closed");
    assert_eq!(v["outcome"], "declined-by-owner");
}

#[test]
fn decide_requires_exactly_one_action_flag() {
    let tmp = tempfile::tempdir().unwrap();
    pm(tmp.path(), "")
        .args(["decide", "v-x", "--accept", "--decline"])
        .assert()
        .code(2);
}

#[test]
fn digest_groups_by_property_and_since_filters() {
    let tmp = tempfile::tempdir().unwrap();
    // `vote-roof` is scanned alongside `update-pm` to exercise multi-id
    // scan, but only produces a `VoteRecord` (not a corpus item), so it
    // never appears in `digest`; `update-pm`'s property (a genuinely
    // different, real-captured address from the vote/result fixtures — see
    // parse.rs's module doc) is the only one grouped here.
    pm(tmp.path(), "vote-roof update-pm")
        .args(["--json", "scan"])
        .assert()
        .success();
    let out = pm(tmp.path(), "")
        .args(["--json", "digest"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["properties"][0]["property"], "18-quarry-ridge-ct");
    let out2 = pm(tmp.path(), "")
        .args(["--json", "digest", "--since", "2099-01-01"])
        .assert()
        .success();
    let v2: serde_json::Value = serde_json::from_slice(&out2.get_output().stdout).unwrap();
    assert_eq!(v2["properties"].as_array().unwrap().len(), 0);
}

#[test]
fn ledger_filters_by_vote() {
    let tmp = tempfile::tempdir().unwrap();
    pm(tmp.path(), "vote-roof")
        .args(["--json", "scan"])
        .assert()
        .success();
    let out = pm(tmp.path(), "")
        .args(["--json", "ledger", "--vote", "v-vote-roof"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert!(v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["vote_id"] == "v-vote-roof"));
    assert!(!v["entries"].as_array().unwrap().is_empty());
}

#[test]
fn digest_since_rejects_garbage_date() {
    let tmp = tempfile::tempdir().unwrap();
    pm(tmp.path(), "")
        .args(["digest", "--since", "not-a-date"])
        .assert()
        .code(2);
}

#[test]
fn digest_items_are_newest_first_within_a_property() {
    let tmp = tempfile::tempdir().unwrap();
    // Two corpus items for the same property, filed by separate scans at
    // different clock times. Both are kind "result" (rather than one
    // "update"/one "result", as the pre-real-fixture version of this test
    // used) because the real captured update email turned out to be for a
    // genuinely different property than the vote/result emails (see
    // parse.rs's module doc) — so a same-property, mixed-kind pair isn't
    // available from the real-shaped fixtures. `result-owner-proposed`
    // (Listing Price) files first, then `result-roof` (Offer Accepted);
    // neither has an open vote-roof to match against yet, so both land as
    // corpus items rather than closing a vote (see scan.rs's
    // zero-candidates branch). Ordering is checked by subject, since kind
    // alone no longer distinguishes them.
    pm_at(tmp.path(), "result-owner-proposed", "2026-07-01T00:00:00Z")
        .args(["--json", "scan"])
        .assert()
        .success();
    pm_at(tmp.path(), "result-roof", "2026-07-10T00:00:00Z")
        .args(["--json", "scan"])
        .assert()
        .success();

    let out = pm(tmp.path(), "")
        .args(["--json", "digest"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["properties"].as_array().unwrap().len(), 1);
    let items = v["properties"][0]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["kind"], "result");
    assert_eq!(items[1]["kind"], "result");
    // 2026-07-10, newest first
    assert!(items[0]["subject"]
        .as_str()
        .unwrap()
        .contains("Offer Accepted"));
    // 2026-07-01
    assert!(items[1]["subject"]
        .as_str()
        .unwrap()
        .contains("Listing Price"));
}

#[test]
fn ledger_limit_keeps_last_n_in_chronological_order() {
    let tmp = tempfile::tempdir().unwrap();
    pm(tmp.path(), "vote-roof")
        .args(["--json", "scan"])
        .assert()
        .success();
    recommend(tmp.path(), "v-vote-roof");
    pm(tmp.path(), "")
        .args(["--json", "decide", "v-vote-roof", "--accept"])
        .assert()
        .success();
    pm(tmp.path(), "")
        .args(["votes", "mark-cast", "v-vote-roof"])
        .assert()
        .success();
    // Ledger now has 4 entries for v-vote-roof: detected, recommended,
    // accepted, cast-confirmed. --limit 2 keeps the last two, still oldest
    // first (chronological / "newest last" reading order).
    let out = pm(tmp.path(), "")
        .args(["--json", "ledger", "--vote", "v-vote-roof", "--limit", "2"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    let entries = v["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["detail"], "accepted");
    assert_eq!(entries[1]["detail"], "cast-confirmed");
}

#[test]
fn reminders_fire_by_level_and_ack_silences() {
    let tmp = tempfile::tempdir().unwrap();
    pm(tmp.path(), "vote-roof")
        .args(["--json", "scan"])
        .assert()
        .success();
    // The real vote-roof template's deadline isn't machine-parseable (see
    // parse.rs's extract_deadline doc comment / scan_marks_missed_after_deadline),
    // so inject one by hand to exercise reminder levels deterministically.
    pm(tmp.path(), "")
        .args([
            "votes",
            "amend",
            "v-vote-roof",
            "--deadline",
            "2026-07-29T06:59:00Z",
        ])
        .assert()
        .success();
    recommend(tmp.path(), "v-vote-roof");
    // deadline 2026-07-29T06:59Z; at NOW=2026-07-28T00:00Z hours_left≈31 → level 48 fires
    let out = pm_at(tmp.path(), "", "2026-07-28T00:00:00Z")
        .args(["--json", "pending"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["reminders"][0]["level_hours"], 48);
    pm_at(tmp.path(), "", "2026-07-28T00:00:00Z")
        .args(["pending", "ack", "v-vote-roof", "--level", "48"])
        .assert()
        .success();
    let out2 = pm_at(tmp.path(), "", "2026-07-28T00:00:00Z")
        .args(["--json", "pending"])
        .assert()
        .success();
    let v2: serde_json::Value = serde_json::from_slice(&out2.get_output().stdout).unwrap();
    assert_eq!(v2["reminders"].as_array().unwrap().len(), 0);
    // closer to deadline the 12h level fires fresh
    let out3 = pm_at(tmp.path(), "", "2026-07-29T00:00:00Z")
        .args(["--json", "pending"])
        .assert()
        .success();
    let v3: serde_json::Value = serde_json::from_slice(&out3.get_output().stdout).unwrap();
    assert_eq!(v3["reminders"][0]["level_hours"], 12);
}

#[test]
fn auto_mode_is_double_gated() {
    let tmp = tempfile::tempdir().unwrap();
    // no verifications: refused
    pm(tmp.path(), "")
        .args([
            "config",
            "set",
            "mode.742-birchwood-ave",
            "auto",
            "--acknowledge-risk",
        ])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("verifications"));
    for item in ["operating-agreement", "api-terms", "vote-attestation"] {
        pm(tmp.path(), "")
            .args(["verify", "record", item])
            .assert()
            .success();
    }
    // verified but no acknowledgment: refused
    pm(tmp.path(), "")
        .args(["config", "set", "mode.742-birchwood-ave", "auto"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("acknowledge-risk"));
    // both gates passed
    pm(tmp.path(), "")
        .args([
            "config",
            "set",
            "mode.742-birchwood-ave",
            "auto",
            "--acknowledge-risk",
        ])
        .assert()
        .success();
    let out = pm(tmp.path(), "")
        .args(["--json", "verify", "list"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["auto_mode_unlocked"], true);

    // The riskiest state change in the product must leave an audit trace:
    // enabling auto mode after both gates pass appends an `Actor::Human`
    // "auto-mode-enabled" ledger event for the property.
    let ledger_out = pm(tmp.path(), "")
        .args(["--json", "ledger"])
        .assert()
        .success();
    let ledger: serde_json::Value =
        serde_json::from_slice(&ledger_out.get_output().stdout).unwrap();
    let entries = ledger["entries"].as_array().unwrap();
    let enabled = entries
        .iter()
        .find(|e| e["event"] == "auto-mode-enabled")
        .expect("ledger missing auto-mode-enabled event");
    assert_eq!(enabled["actor"], "human");
    assert_eq!(enabled["detail"], "742-birchwood-ave");
}

/// The mirror image of `auto_mode_is_double_gated`'s enable-side ledger
/// assertion: turning a property back to `recommend` after it was `auto`
/// logs `auto-mode-disabled`, but a plain recommend-to-recommend set (never
/// having been `auto`) stays silent and never touches the ledger — or the
/// data store at all.
#[test]
fn disabling_auto_mode_logs_ledger_event_recommend_to_recommend_is_silent() {
    let tmp = tempfile::tempdir().unwrap();

    // `pm`/`pm_at` each rewrite config.json to a fresh baseline before every
    // invocation (see `pm_at_with`'s doc comment), so a prior `config set
    // mode.X auto` call in this same test wouldn't survive to the next
    // invocation. Seed the property as already-`auto` directly via `extra`
    // instead, so the disable transition can be exercised in one call —
    // the enable-side gates are already covered by `auto_mode_is_double_gated`.
    pm_at_with(
        tmp.path(),
        "",
        DEFAULT_NOW,
        &serde_json::json!({ "modes": { "742-birchwood-ave": "auto" } }),
    )
    .args(["config", "set", "mode.742-birchwood-ave", "recommend"])
    .assert()
    .success();

    // auto -> recommend: logs auto-mode-disabled.
    let out = pm(tmp.path(), "")
        .args(["--json", "ledger"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    let entries = v["entries"].as_array().unwrap();
    let disabled = entries
        .iter()
        .find(|e| e["event"] == "auto-mode-disabled")
        .expect("ledger missing auto-mode-disabled event");
    assert_eq!(disabled["actor"], "human");
    assert_eq!(disabled["detail"], "742-birchwood-ave");
    let ledger_len_after_disable = entries.len();

    // recommend -> recommend (never having been auto): silent, no new
    // ledger entry, no data-dir access for a property that's never touched
    // verify/auto at all.
    pm(tmp.path(), "")
        .args(["config", "set", "mode.elm-st-12", "recommend"])
        .assert()
        .success();
    let out2 = pm(tmp.path(), "")
        .args(["--json", "ledger"])
        .assert()
        .success();
    let v2: serde_json::Value = serde_json::from_slice(&out2.get_output().stdout).unwrap();
    assert_eq!(
        v2["entries"].as_array().unwrap().len(),
        ledger_len_after_disable,
        "recommend-to-recommend must not append a ledger entry"
    );
}

#[test]
fn init_reports_checks_and_warns_on_empty_mailbox() {
    let tmp = tempfile::tempdir().unwrap();
    let out = pm(tmp.path(), "")
        .args(["--json", "init"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "init-report/v1");
    let checks = v["checks"].as_array().unwrap();
    let mail = checks
        .iter()
        .find(|c| c["name"] == "mailbox_has_lofty_mail")
        .unwrap();
    assert_eq!(mail["ok"], false);
    assert!(mail["detail"].as_str().unwrap().contains("gro_ref"));
}

#[test]
fn votes_list_limit_truncates_the_already_newest_first_list() {
    let tmp = tempfile::tempdir().unwrap();
    pm(tmp.path(), "vote-roof")
        .args(["--json", "scan"])
        .assert()
        .success();

    let out = pm(tmp.path(), "")
        .args(["--json", "votes", "list", "--limit", "1"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["votes"].as_array().unwrap().len(), 1);

    let out0 = pm(tmp.path(), "")
        .args(["--json", "votes", "list", "--limit", "0"])
        .assert()
        .success();
    let v0: serde_json::Value = serde_json::from_slice(&out0.get_output().stdout).unwrap();
    assert_eq!(v0["votes"].as_array().unwrap().len(), 0);
}
