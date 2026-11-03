//! Offline surface tests: flags, exit codes, JSON contract. No network.

use assert_cmd::Command;
use predicates::prelude::*;

fn pm() -> Command {
    Command::cargo_bin("lofty-pm").unwrap()
}

#[test]
fn help_lists_the_standard_surface() {
    pm().arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("auth"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("self-update"))
        .stdout(predicate::str::contains("completions"))
        .stdout(predicate::str::contains("info"))
        .stdout(predicate::str::contains("mcp"));
}

#[test]
fn info_emits_cli_info_v1_credential_free() {
    let out = pm().arg("info").assert().success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "cli-info/v1");
    assert_eq!(v["name"], "lofty-pm");
    assert_eq!(v["spec"], "piekstra-cli/1");
    assert_eq!(v["auth"]["required"], false);
}

#[test]
fn auth_status_reports_credential_free() {
    let out = pm().args(["--json", "auth", "status"]).assert().success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "auth-status/v1");
    assert_eq!(v["required"], false);
}

#[test]
fn no_color_in_the_environment_never_fails_a_command() {
    // Any non-empty NO_COLOR disables color (no-color.org); parsed as a
    // bool, the conventional NO_COLOR=1 was a usage error.
    let tmp = tempfile::tempdir().unwrap();
    for v in ["1", "true", "0", ""] {
        pm().env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
            .env("LOFTY_PM_DATA_DIR", tmp.path().join("data"))
            .env("NO_COLOR", v)
            .args(["completions", "bash"])
            .assert()
            .success();
    }
}

#[test]
fn unknown_config_key_is_usage_error_exit_2() {
    let tmp = tempfile::tempdir().unwrap();
    let out = pm()
        .env("XDG_CONFIG_HOME", tmp.path())
        .args(["--json", "config", "set", "bogus_key", "x"])
        .assert()
        .code(2);
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["error"]["code"], "usage");
}

#[test]
fn config_set_non_mode_key_never_touches_the_data_store() {
    // Regression guard: `config set` of a non-`mode.*` key must never open
    // (and so never create) the data store. Point LOFTY_PM_DATA_DIR at a
    // path that's un-creatable (a regular file sits where a directory
    // component would need to go) — if `Store::open` were ever called
    // here, `fs::create_dir_all` fails loudly ("Not a directory") and the
    // command would exit non-zero instead of the success asserted below.
    let tmp = tempfile::tempdir().unwrap();
    let blocker = tmp.path().join("blocker");
    std::fs::write(&blocker, "not a directory").unwrap();
    let data_dir = blocker.join("data");

    pm().env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
        .env("LOFTY_PM_DATA_DIR", &data_dir)
        .args(["config", "set", "gro_bin", "/usr/bin/true"])
        .assert()
        .success();
}

#[test]
fn config_show_json_carries_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let out = pm()
        .env("XDG_CONFIG_HOME", tmp.path())
        .args(["--json", "config", "show"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "config/v1");
}

#[test]
fn config_path_json_carries_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let out = pm()
        .env("XDG_CONFIG_HOME", tmp.path())
        .args(["--json", "config", "path"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "config-path/v1");
    assert!(v["path"]
        .as_str()
        .unwrap()
        .ends_with("lofty-pm/config.json"));
}

#[test]
fn stance_set_validates_and_stamps_provenance() {
    let tmp = tempfile::tempdir().unwrap();
    let env = |c: &mut Command| {
        c.env("XDG_CONFIG_HOME", tmp.path().join("cfg"))
            .env("LOFTY_PM_DATA_DIR", tmp.path().join("data"))
            .env("LOFTY_PM_NOW", "2026-07-24T12:00:00Z");
    };
    let mut c = pm();
    env(&mut c);
    c.args(["stance", "set", "risk_tolerance", "moderate"])
        .assert()
        .success();
    let mut c2 = pm();
    env(&mut c2);
    c2.args(["stance", "set", "horizon", "sideways"])
        .assert()
        .code(2);
    let mut c3 = pm();
    env(&mut c3);
    let out = c3.args(["--json", "stance", "show"]).assert().success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["global"]["risk_tolerance"], "moderate");
    assert_eq!(v["meta"]["global.risk_tolerance"]["updated_by"], "owner");
}

#[test]
fn stance_questions_are_versioned_and_complete() {
    // Belt and braces: point LOFTY_PM_DATA_DIR at a dir that doesn't exist
    // yet, on top of `stance questions` being dispatched store-free by
    // `main.rs`. Either protection alone would keep this test off the
    // developer's real `~/.local/share/lofty-pm` — see the regression
    // assertion below, which proves the store-free dispatch specifically.
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().join("should-not-be-created");
    let out = pm()
        .env("LOFTY_PM_DATA_DIR", &data_dir)
        .args(["--json", "stance", "questions"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["schema"], "stance-questions/v1");
    let fields: Vec<&str> = v["questions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["field"].as_str().unwrap())
        .collect();
    for f in [
        "risk_tolerance",
        "horizon",
        "orientation",
        "add_capital",
        "notes",
    ] {
        assert!(fields.contains(&f), "missing question for {f}");
    }
    // Regression guard: `stance questions` is documented as "Works without
    // a store" (see `stance::Cmd::Questions`). If dispatch ever regresses
    // to opening the data store again, `Store::open` would `create_dir_all`
    // this path, and this assertion would catch it.
    assert!(
        !data_dir.exists(),
        "stance questions must not open (and so must not create) the data store"
    );
}
