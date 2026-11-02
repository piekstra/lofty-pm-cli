//! `init` — a readiness wizard. Probes `gro`/`lofty` presence and auth, the
//! configured mailbox's Lofty coverage, and data-dir writability, and
//! reports every check's outcome. It's a report, not a gate: it always
//! exits 0 (even with failing/warning checks) so an agent can inspect
//! `ready` and individual check details programmatically.
//!
//! Deliberately does not go through `commands::{Ctx, emit}`: those assume
//! an already-open `Store`, but `data_dir_writable` must itself be one of
//! the reported checks rather than a precondition for running the command
//! at all (see `main.rs`, which loads config but never opens the store for
//! `init`).

use std::process::Command;

use pk_cli_core::{output, CliError, CommonArgs};
use serde_json::{json, Value};

use crate::commands::votes::lofty_ok;
use crate::config::Config;
use crate::gmail::Gmail;
use crate::store::Store;

/// `lofty-pm init` takes no flags today; kept as a struct for wiring
/// consistency with the other subcommands and headroom for e.g. a future
/// `--fix`.
#[derive(clap::Args, Debug)]
pub struct Args {}

/// One readiness probe's outcome.
struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
}

pub fn run(common: &CommonArgs, cfg: &Config, _args: &Args) -> Result<(), CliError> {
    // Every check always runs and produces a row, even when an earlier
    // check fails — this is a report, not a short-circuiting pipeline.
    let checks = [
        check_gro_present(cfg),
        check_gro_auth(cfg),
        check_lofty_present(cfg),
        check_lofty_auth(cfg),
        check_mailbox_has_lofty_mail(cfg),
        check_data_dir_writable(),
    ];

    let ok = |name: &str| checks.iter().find(|c| c.name == name).is_some_and(|c| c.ok);
    // lofty_auth is a warning, not a blocker: `lofty` being unauthenticated
    // just means recommendations run degraded, it doesn't stop mail
    // scanning/tracking, so it's excluded here on purpose.
    let ready = ok("gro_present") && ok("gro_auth") && ok("data_dir_writable");

    let checks_json: Vec<Value> = checks
        .iter()
        .map(|c| json!({ "name": c.name, "ok": c.ok, "detail": c.detail }))
        .collect();

    if common.json {
        output::json(&json!({
            "schema": "init-report/v1",
            "checks": checks_json,
            "ready": ready,
        }));
    } else {
        for c in &checks {
            let mark = if c.ok { "\u{2713}" } else { "\u{2717}" };
            println!("{mark} {}: {}", c.name, c.detail);
        }
        println!("ready: {ready}");
    }
    Ok(())
}

/// First non-empty trimmed line of `bytes`, or `None` if there isn't one.
fn first_line(bytes: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(bytes);
    let line = s.lines().next()?.trim();
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

fn check_gro_present(cfg: &Config) -> Check {
    let bin = cfg.gro_bin();
    match Command::new(bin).arg("--version").output() {
        Ok(out) if out.status.success() => Check {
            name: "gro_present",
            ok: true,
            detail: first_line(&out.stdout).unwrap_or_else(|| format!("{bin} is installed")),
        },
        Ok(out) => Check {
            name: "gro_present",
            ok: false,
            detail: format!("`{bin} --version` exited {}; is gro installed?", out.status),
        },
        Err(e) => Check {
            name: "gro_present",
            ok: false,
            detail: format!("running `{bin} --version`: {e}; is gro installed?"),
        },
    }
}

fn check_gro_auth(cfg: &Config) -> Check {
    let bin = cfg.gro_bin();
    let ref_label = cfg.gro_ref().unwrap_or("default");
    let mut cmd = Command::new(bin);
    if let Some(r) = cfg.gro_ref() {
        cmd.arg("--ref").arg(r);
    }
    cmd.arg("--no-color").args(["mail", "labels"]);

    match cmd.output() {
        Ok(out) if out.status.success() => Check {
            name: "gro_auth",
            ok: true,
            detail: format!("authenticated (profile {ref_label})"),
        },
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Check {
                name: "gro_auth",
                ok: false,
                detail: format!(
                    "`{bin} mail labels` exited {}: {stderr} — re-auth via `gro init` \
                     (profile {ref_label})",
                    out.status
                ),
            }
        }
        Err(e) => Check {
            name: "gro_auth",
            ok: false,
            detail: format!(
                "running `{bin} mail labels`: {e} — re-auth via `gro init` (profile {ref_label})"
            ),
        },
    }
}

fn check_lofty_present(cfg: &Config) -> Check {
    let bin = cfg.lofty_bin();
    match Command::new(bin).arg("--version").output() {
        Ok(out) if out.status.success() => Check {
            name: "lofty_present",
            ok: true,
            detail: first_line(&out.stdout).unwrap_or_else(|| format!("{bin} is installed")),
        },
        Ok(out) => Check {
            name: "lofty_present",
            ok: false,
            detail: format!(
                "`{bin} --version` exited {}; is lofty installed?",
                out.status
            ),
        },
        Err(e) => Check {
            name: "lofty_present",
            ok: false,
            detail: format!("running `{bin} --version`: {e}; is lofty installed?"),
        },
    }
}

/// Warning, not a blocker (see `ready`'s computation in `run`): a missing
/// `lofty` session degrades recommendations but doesn't stop mail
/// scanning/tracking.
fn check_lofty_auth(cfg: &Config) -> Check {
    let (ok, reason) = lofty_ok(cfg);
    Check {
        name: "lofty_auth",
        ok,
        detail: reason.unwrap_or_else(|| "authenticated".into()),
    }
}

fn check_mailbox_has_lofty_mail(cfg: &Config) -> Check {
    let gmail = Gmail::new(cfg);
    let hint = "connect the Lofty-registered mailbox as a gro profile and set gro_ref";
    match gmail.search_ids(cfg.scan_query()) {
        Ok(ids) if !ids.is_empty() => Check {
            name: "mailbox_has_lofty_mail",
            ok: true,
            detail: format!("found {} matching message(s)", ids.len()),
        },
        Ok(_) => Check {
            name: "mailbox_has_lofty_mail",
            ok: false,
            detail: format!("no messages matched the scan query — {hint}"),
        },
        Err(e) => Check {
            name: "mailbox_has_lofty_mail",
            ok: false,
            detail: format!("{e} — {hint}"),
        },
    }
}

fn check_data_dir_writable() -> Check {
    match Store::open().and_then(|s| s.probe_writable()) {
        Ok(()) => Check {
            name: "data_dir_writable",
            ok: true,
            detail: "data directory is writable".into(),
        },
        Err(e) => Check {
            name: "data_dir_writable",
            ok: false,
            detail: e.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_trims_and_handles_empty() {
        assert_eq!(first_line(b"1.2.3\nextra\n"), Some("1.2.3".to_string()));
        assert_eq!(first_line(b"  1.2.3  \n"), Some("1.2.3".to_string()));
        assert_eq!(first_line(b""), None);
        assert_eq!(first_line(b"\n\n"), None);
    }

    #[test]
    fn gro_auth_detail_names_configured_ref_or_default() {
        let cfg = Config {
            gro_bin: Some("/nonexistent/gro-bin-that-does-not-exist".into()),
            ..Default::default()
        };
        let c = check_gro_auth(&cfg);
        assert!(!c.ok);
        assert!(c.detail.contains("default"));
        assert!(c.detail.contains("gro init"));

        let cfg_ref = Config {
            gro_bin: Some("/nonexistent/gro-bin-that-does-not-exist".into()),
            gro_ref: Some("work".into()),
            ..Default::default()
        };
        let c_ref = check_gro_auth(&cfg_ref);
        assert!(!c_ref.ok);
        assert!(c_ref.detail.contains("work"));
    }
}
