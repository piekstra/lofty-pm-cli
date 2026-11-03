//! The `lofty-pm mcp` tools, offline, over a copy of `tests/fixtures/store/`.
//! `tests/mcp_stdio.rs`, `tests/mcp_http.rs` and `tests/mcp_readonly.rs` drive the
//! real binary.

use std::path::Path;

use pk_cli_mcp::conformance::{assert_conforms, assert_fits, call, definitions_size};
use serde_json::json;

use super::*;

const TOOLS: [&str; 4] = [
    "loftypm_pending",
    "loftypm_votes",
    "loftypm_digest",
    "loftypm_ledger",
];

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The toolset over its own copy of the fixture store, and the directory to keep.
fn over_fixture() -> (PmTools, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/store");
    copy_dir(&from, dir.path());
    (over(dir.path()), dir)
}

fn over(root: &Path) -> PmTools {
    PmTools {
        server: server(),
        root: Some(root.to_path_buf()),
    }
}

fn ok(tools: &PmTools, name: &str, args: Value) -> Value {
    let result = call(tools, name, args);
    assert_fits(tools, name, &result).clone()
}

fn error(tools: &PmTools, name: &str, args: Value) -> String {
    let result = call(tools, name, args);
    assert!(result.is_error(), "{name}: {}", result.text());
    result.text().to_string()
}

fn ids(got: &Value) -> Vec<&str> {
    got["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect()
}

#[test]
fn the_tools_conform_and_fit_the_definition_budget() {
    let (tools, _dir) = over_fixture();
    assert_conforms(&tools);
    let size = definitions_size(&tools);
    assert!(size < 8_000, "{size} characters");
    let names: Vec<String> = Toolset::tools(&tools).into_iter().map(|t| t.name).collect();
    assert_eq!(names, TOOLS);
}

/// Every tool is a read: annotated so, and with no approval prompt, which only a
/// write carries. Each command that writes is refused by name, and
/// `tests/mcp_readonly.rs` shows the data directory is unchanged by every call.
#[test]
fn no_tool_definition_reaches_a_mutating_command() {
    const MUTATING: [&str; 13] = [
        "scan",
        "recommend",
        "decide",
        "amend",
        "close",
        "mark_cast",
        "cast",
        "ack",
        "stance",
        "record",
        "init",
        "config",
        "vote",
    ];
    for tool in Toolset::tools(&tools()) {
        assert!(tool.annotations.read_only, "{}", tool.name);
        assert!(!tool.annotations.destructive, "{}", tool.name);
        assert!(!tool.requires_user_interaction(), "{}", tool.name);
        let verb = tool.name.strip_prefix("loftypm_").unwrap();
        assert!(
            !MUTATING.contains(&verb),
            "{} is named for a mutating command",
            tool.name
        );
    }
}

#[test]
fn bad_arguments_are_usage_errors_before_the_store_is_read() {
    // A directory that does not exist reads as "never scanned", so reaching the
    // store would answer with that instead.
    let dir = tempfile::tempdir().unwrap();
    let tools = over(&dir.path().join("none"));
    for (name, args) in [
        ("loftypm_digest", json!({"since": "2026-13-01"})),
        ("loftypm_digest", json!({"since": "yesterday"})),
        ("loftypm_votes", json!({"state": "maybe"})),
        ("loftypm_pending", json!({"limit": 0})),
        ("loftypm_ledger", json!({"cursor": "not-a-cursor"})),
    ] {
        let text = error(&tools, name, args.clone());
        assert!(text.starts_with("usage:"), "{name} {args}: {text}");
    }
}

/// With nothing scanned, "no votes" would be a false comfort.
#[test]
fn a_store_nothing_has_filled_says_to_scan_and_not_that_nothing_is_pending() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("none");
    let tools = over(&root);
    for name in TOOLS {
        let text = error(&tools, name, json!({}));
        assert!(text.contains("has not scanned"), "{name}: {text}");
        assert!(text.contains("lofty-pm scan"), "{name}: {text}");
    }
    assert!(!root.exists(), "a tool created the data directory");
}

/// A scan that found nothing still counts: an empty queue is then the truth.
#[test]
fn a_scan_that_found_nothing_is_an_empty_list_with_its_time() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("scan.json"),
        r#"{"schema": "scan/v1", "at": "2030-03-02T06:00:00Z"}"#,
    )
    .unwrap();
    let tools = over(dir.path());
    for name in TOOLS {
        let got = ok(&tools, name, json!({}));
        assert_eq!(got["returned"], 0, "{name}");
        assert_eq!(got["last_scan"], "2030-03-02T06:00:00Z", "{name}");
    }
}

#[test]
fn a_store_from_before_the_scan_time_was_kept_says_so_with_a_null() {
    let (tools, dir) = over_fixture();
    std::fs::remove_file(dir.path().join("scan.json")).unwrap();
    let got = ok(&tools, "loftypm_pending", json!({}));
    assert!(got["last_scan"].is_null(), "{got}");
    assert!(got["returned"].as_u64().unwrap() > 0);
}

/// A clean scan says nothing extra, and one that missed mail says so on every tool,
/// since `last_scan` alone would read as fresh.
#[test]
fn a_scan_that_did_not_cover_the_mail_says_so() {
    let (tools, dir) = over_fixture();
    for name in TOOLS {
        let got = ok(&tools, name, json!({}));
        assert!(got.get("unread_emails").is_none(), "{name}: {got}");
        assert!(got.get("scan_narrowed").is_none(), "{name}: {got}");
    }
    std::fs::write(
        dir.path().join("scan.json"),
        r#"{"schema":"scan/v1","at":"2030-03-02T06:00:00Z","unread":2,"narrowed":true}"#,
    )
    .unwrap();
    for name in TOOLS {
        let got = ok(&tools, name, json!({}));
        assert_eq!(got["unread_emails"], 2, "{name}");
        assert_eq!(got["scan_narrowed"], true, "{name}");
    }
}

#[test]
fn pending_lists_what_waits_on_someone_soonest_deadline_first() {
    let (tools, _dir) = over_fixture();
    let got = ok(&tools, "loftypm_pending", json!({}));
    assert_eq!(got["last_scan"], "2030-03-02T06:00:00Z");
    assert_eq!(ids(&got), ["v-analyze", "v-decide", "v-msg-cast"]);
    assert_eq!(got["missed"], 1);
    let items = got["items"].as_array().unwrap();
    assert_eq!(items[0]["waiting_on"], "analysis");
    assert_eq!(items[1]["waiting_on"], "decision");
    assert_eq!(items[1]["recommended"], "Approve");
    assert_eq!(items[2]["waiting_on"], "casting");
    assert_eq!(items[2]["decided"], "Approve");
    assert!(items[0]["hours_left"].as_f64().unwrap() > 0.0);
}

/// `lofty-pm pending` writes this sweep; a tool reports it without writing.
#[test]
fn a_lapsed_vote_reads_as_missed_though_the_file_still_says_recommended() {
    let (tools, dir) = over_fixture();
    let path = dir.path().join("votes/v-lapsed.json");
    let before = std::fs::read(&path).unwrap();
    let got = ok(&tools, "loftypm_votes", json!({"vote": "v-lapsed"}));
    assert_eq!(got["items"][0]["state"], "missed");
    assert_eq!(std::fs::read(&path).unwrap(), before);

    // And it filters as what it now is.
    let got = ok(&tools, "loftypm_votes", json!({"state": "missed"}));
    assert_eq!(ids(&got), ["v-lapsed"]);
    let got = ok(&tools, "loftypm_votes", json!({"state": "recommended"}));
    assert_eq!(ids(&got), ["v-decide"]);
}

#[test]
fn votes_list_newest_first_and_narrow() {
    let (tools, _dir) = over_fixture();
    let got = ok(&tools, "loftypm_votes", json!({}));
    assert_eq!(
        ids(&got),
        [
            "v-analyze",
            "v-msg-cast",
            "v-decide",
            "v-closed",
            "v-lapsed"
        ]
    );
    let got = ok(&tools, "loftypm_votes", json!({"property": "OAK"}));
    assert_eq!(ids(&got), ["v-analyze", "v-closed"]);
    let got = ok(&tools, "loftypm_votes", json!({"vote": "v-closed"}));
    assert_eq!(got["items"][0]["outcome"], "Sample outcome: approved");
}

#[test]
fn only_detailed_carries_the_proposal_and_the_reasoning() {
    let (tools, _dir) = over_fixture();
    let got = ok(&tools, "loftypm_votes", json!({"vote": "v-decide"}));
    assert!(got["items"][0].get("proposal").is_none(), "{got}");
    assert!(got["items"][0].get("reasoning").is_none(), "{got}");

    let got = ok(
        &tools,
        "loftypm_votes",
        json!({"vote": "v-decide", "response_format": "detailed"}),
    );
    let item = &got["items"][0];
    assert!(item["proposal"]
        .as_str()
        .unwrap()
        .contains("replace the roof"));
    assert!(item["reasoning"]
        .as_str()
        .unwrap()
        .contains("Sample reasoning"));
    assert_eq!(item["options"], json!(["Approve", "Reject"]));
    assert_eq!(item["tier"], "routine");
    assert_eq!(item["confidence"], 0.5);
    assert_eq!(item["detected"], "2030-01-01");
}

/// A Vote Now link reaches the ballot, so it is out of every result: the record's
/// own field, and any URL quoted in the text. The raw email too, and every message
/// id except the one a vote's own id is built from (`v-msg-cast`, as `scan` builds it).
#[test]
fn no_result_carries_a_link_a_message_id_or_the_raw_email() {
    let (tools, _dir) = over_fixture();
    for name in TOOLS {
        for args in [
            json!({}),
            json!({"response_format": "detailed", "limit": 100}),
        ] {
            let result = call(&tools, name, args.clone());
            let text = assert_fits(&tools, name, &result).to_string();
            for held_back in [
                "example.invalid",
                "token=",
                "http",
                "msg-later",
                "msg-dummy",
                "msg-1",
                "msg-2",
                "msg-3",
                "message_ids",
                "Sample raw",
            ] {
                assert!(
                    !text.contains(held_back),
                    "{name} {args}: {held_back} in {text}"
                );
            }
        }
    }
}

#[test]
fn a_url_is_cut_out_of_the_text_around_it() {
    assert_eq!(
        without_links("see https://example.invalid/a?b=1 and WWW.example.invalid/c, then."),
        "see [link withheld] and [link withheld] then."
    );
    // Cut after the links go, so a long one cannot leave a stub.
    let long = format!("{} tail", "x".repeat(10));
    assert_eq!(quoted(&long, 4), "xxxx…");
    assert_eq!(quoted("héllo", 10), "héllo");
}

#[test]
fn digest_lists_filed_email_newest_first_and_narrows() {
    let (tools, _dir) = over_fixture();
    let got = ok(&tools, "loftypm_digest", json!({}));
    let items = got["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["date"], "2030-03-01");
    assert_eq!(items[0]["property"], "elm-st-12");
    assert_eq!(items[0]["kind"], "update");
    assert_eq!(items[0]["subject"], "Sample update: roof finished");

    let got = ok(&tools, "loftypm_digest", json!({"since": "2030-02-01"}));
    assert_eq!(got["returned"], 2);
    let got = ok(&tools, "loftypm_digest", json!({"property": "oak"}));
    assert_eq!(got["returned"], 1);
    assert_eq!(got["items"][0]["property"], "oak-ave-34");
}

#[test]
fn detailed_quotes_more_of_an_email() {
    let (tools, _dir) = over_fixture();
    let short = ok(&tools, "loftypm_digest", json!({"since": "2030-01-01"}));
    let long = ok(
        &tools,
        "loftypm_digest",
        json!({"since": "2030-01-01", "response_format": "detailed"}),
    );
    let excerpt = |got: &Value| got["items"][2]["excerpt"].as_str().unwrap().chars().count();
    assert_eq!(excerpt(&short), EXCERPT + 1);
    assert!(excerpt(&long) > EXCERPT + 1);
}

#[test]
fn the_ledger_lists_newest_first_and_narrows_to_a_vote() {
    let (tools, _dir) = over_fixture();
    let got = ok(&tools, "loftypm_ledger", json!({}));
    assert_eq!(got["returned"], 4);
    assert_eq!(got["items"][0]["at"], "2030-01-04T00:00:00Z");
    assert_eq!(got["items"][1]["to"], "accepted");
    assert!(got["items"][1].get("actor").is_none(), "{got}");

    let got = ok(
        &tools,
        "loftypm_ledger",
        json!({"vote": "v-decide", "response_format": "detailed"}),
    );
    assert_eq!(got["returned"], 2);
    assert_eq!(got["items"][0]["actor"], "agent");
    assert_eq!(got["items"][0]["from"], "needs-analysis");
    assert_eq!(
        got["items"][0]["detail"],
        "Sample detail, see [link withheld]"
    );
}

#[test]
fn a_cursor_continues_its_own_query() {
    let (tools, _dir) = over_fixture();
    let first = ok(&tools, "loftypm_votes", json!({"limit": 2}));
    assert_eq!(first["returned"], 2);
    let cursor = first["next_cursor"].as_str().expect("a next cursor");
    let next = ok(
        &tools,
        "loftypm_votes",
        json!({"limit": 2, "cursor": cursor}),
    );
    assert_eq!(ids(&next), ["v-decide", "v-closed"]);

    let text = error(
        &tools,
        "loftypm_votes",
        json!({"limit": 2, "cursor": cursor, "property": "elm"}),
    );
    assert!(text.starts_with("usage:"), "{text}");
}

/// A mistyped name must not read as nothing having happened at that property.
#[test]
fn an_unknown_vote_or_property_is_not_found_and_says_how_to_look_one_up() {
    let (tools, _dir) = over_fixture();
    for (name, args) in [
        ("loftypm_votes", json!({"vote": "v-nope"})),
        ("loftypm_votes", json!({"property": "nowhere"})),
        ("loftypm_digest", json!({"property": "nowhere"})),
        ("loftypm_ledger", json!({"vote": "v-nope"})),
    ] {
        let text = error(&tools, name, args.clone());
        assert!(text.starts_with("not_found:"), "{name} {args}: {text}");
        assert!(text.contains("loftypm_votes"), "{name} {args}: {text}");
    }
}

/// `Store::load_vote` and `list_corpus(Some(..))` build a path from what they are
/// given. A tool compares against what the store lists, so an argument cannot reach
/// a file outside it.
#[test]
fn an_argument_cannot_walk_out_of_the_store() {
    let (tools, dir) = over_fixture();
    std::fs::write(
        dir.path().join("outside.json"),
        std::fs::read(dir.path().join("votes/v-closed.json")).unwrap(),
    )
    .unwrap();
    for (name, args) in [
        ("loftypm_votes", json!({"vote": "../outside"})),
        ("loftypm_ledger", json!({"vote": "../outside"})),
        ("loftypm_digest", json!({"property": "../.."})),
        ("loftypm_digest", json!({"property": "/etc"})),
    ] {
        let text = error(&tools, name, args.clone());
        assert!(text.starts_with("not_found:"), "{name} {args}: {text}");
    }
}

/// The message names a file under the home directory.
#[test]
fn a_corrupt_file_is_reported_without_its_path() {
    let (tools, dir) = over_fixture();
    std::fs::write(dir.path().join("votes/v-closed.json"), "{not json").unwrap();
    let text = error(&tools, "loftypm_votes", json!({}));
    assert!(text.contains("could not be read"), "{text}");
    assert!(text.contains("lofty-pm votes list"), "{text}");
    let root = dir.path().to_string_lossy().to_string();
    assert!(!text.contains(&root), "{text}");
    assert!(!text.contains("v-closed"), "{text}");
}
