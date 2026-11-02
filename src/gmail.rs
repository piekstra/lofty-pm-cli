//! Subprocess wrapper around `gro`/`grw` (Gmail CLI) plus a tolerant
//! parser for `mail read` output. Holds no credentials of its own —
//! `gro`/`grw` manage their own OAuth state per `--ref` profile.

use std::process::Command;

use chrono::{DateTime, Utc};
use pk_cli_core::CliError;

use crate::config::Config;

/// A parsed `gro mail read` result.
#[derive(Debug, Clone)]
pub struct RawEmail {
    #[allow(dead_code)] // read once recommend/pending surface sender/date
    pub id: String,
    #[allow(dead_code)]
    pub from: String,
    pub subject: String,
    #[allow(dead_code)]
    pub date: Option<DateTime<Utc>>,
    pub body: String,
}

/// Thin wrapper around a `gro`/`grw`-compatible binary for one configured
/// profile (`--ref`).
pub struct Gmail {
    bin: String,
    gro_ref: Option<String>,
}

impl Gmail {
    pub fn new(cfg: &Config) -> Gmail {
        Gmail {
            bin: cfg.gro_bin().to_string(),
            gro_ref: cfg.gro_ref().map(str::to_string),
        }
    }

    /// `<bin> [--ref <ref>] --no-color ...`, ready for subcommand args.
    fn base_command(&self) -> Command {
        let mut cmd = Command::new(&self.bin);
        if let Some(r) = &self.gro_ref {
            cmd.arg("--ref").arg(r);
        }
        cmd.arg("--no-color");
        cmd
    }

    fn ref_label(&self) -> &str {
        self.gro_ref.as_deref().unwrap_or("default")
    }

    pub fn search_ids(&self, query: &str) -> Result<Vec<String>, CliError> {
        let mut cmd = self.base_command();
        cmd.args(["mail", "search", query, "--ids", "--max", "100"]);
        let out = run(cmd, self.ref_label())?;
        Ok(out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect())
    }

    pub fn read(&self, id: &str) -> Result<RawEmail, CliError> {
        let mut cmd = self.base_command();
        cmd.args(["mail", "read", id]);
        let out = run(cmd, self.ref_label())?;
        parse_read_output(id, &out)
    }
}

/// Spawn `cmd`, mapping spawn failure and non-zero exit to `CliError`.
fn run(mut cmd: Command, gro_ref: &str) -> Result<String, CliError> {
    let output = cmd
        .output()
        .map_err(|e| CliError::Other(format!("running gro: {e}; is gro installed?")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(map_failure(&stderr, gro_ref));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Map a non-zero `gro`/`grw` exit's stderr to a `CliError`.
pub fn map_failure(stderr: &str, gro_ref: &str) -> CliError {
    if stderr.contains("invalid_grant") || stderr.contains("oauth2") {
        CliError::Auth(format!(
            "gro token expired/revoked — run `gro init` (profile {gro_ref})"
        ))
    } else {
        CliError::Upstream(stderr.to_string())
    }
}

/// Parse `gro mail read` output into a `RawEmail`. Headers are `Name:
/// value` lines up to the first blank line; unknown header lines (`ID:`,
/// `To:`, `Categories:`, ...) are tolerated — ignored. `Date:` is parsed
/// as RFC 2822, falling back to `None` on failure. Everything after the
/// first blank line is the body verbatim.
pub fn parse_read_output(id: &str, out: &str) -> Result<RawEmail, CliError> {
    let mut from = String::new();
    let mut subject = String::new();
    let mut date = None;
    let mut found_separator = false;
    let mut body_lines: Vec<&str> = Vec::new();
    let mut lines = out.lines();

    for line in &mut lines {
        if line.trim().is_empty() {
            found_separator = true;
            break;
        }
        if let Some(v) = line.strip_prefix("From:") {
            from = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Subject:") {
            subject = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("Date:") {
            date = DateTime::parse_from_rfc2822(v.trim())
                .ok()
                .map(|d| d.with_timezone(&Utc));
        }
        // Unknown header lines (ID:, To:, Categories:, ...) fall through
        // unhandled — tolerated by design.
    }

    if !found_separator {
        return Err(CliError::Upstream(format!(
            "malformed `gro mail read` output for {id}: no blank line separating headers from body"
        )));
    }

    body_lines.extend(lines);
    Ok(RawEmail {
        id: id.to_string(),
        from,
        subject,
        date,
        body: body_lines.join("\n"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture_read_output() {
        let raw = include_str!("../tests/fixtures/emails/vote-roof.txt");
        let m = parse_read_output("fixt-vote-roof", raw).unwrap();
        assert_eq!(m.from, "Lofty <info@lofty.ai>");
        assert!(m.subject.contains("Offer Received"));
        assert!(m.body.contains("Vote Now"));
    }

    #[test]
    fn auth_failure_maps_to_exit_3_error() {
        let e = map_failure(
            "oauth2: \"invalid_grant\" \"Token has been expired or revoked.\"",
            "default",
        );
        assert!(matches!(e, CliError::Auth(_)));
    }

    #[test]
    fn non_auth_failure_maps_to_upstream() {
        let e = map_failure("rate limit exceeded", "default");
        assert!(matches!(e, CliError::Upstream(_)));
    }

    #[test]
    fn parser_tolerates_unknown_headers_and_parses_date() {
        let raw = include_str!("../tests/fixtures/emails/vote-roof.txt");
        let m = parse_read_output("fixt-vote-roof", raw).unwrap();
        assert_eq!(m.id, "fixt-vote-roof");
        assert!(m.date.is_some(), "Date: header should parse via RFC 2822");
        assert!(m.body.contains("EXAMPLEVOTE01"));
    }

    #[test]
    fn parser_falls_back_to_none_on_unparsable_date() {
        let raw = "From: a@b.com\nSubject: x\nDate: not-a-date\n\nbody";
        let m = parse_read_output("id1", raw).unwrap();
        assert_eq!(m.date, None);
        assert_eq!(m.body, "body");
    }

    #[test]
    fn parser_errors_when_no_header_body_separator() {
        let raw = "From: a@b.com\nSubject: x\nDate: bogus";
        let e = parse_read_output("id1", raw).unwrap_err();
        assert!(matches!(e, CliError::Upstream(_)));
    }
}
