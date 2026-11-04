# AGENTS.md

Guidance for AI coding agents (and humans) working in this repo. Tool-agnostic.

## What this is

`lofty-pm` — a Rust CLI (executable `lofty-pm`) that watches a Lofty
investor's email for property governance votes and updates, and manages an
auditable recommend → decide → cast → outcome lifecycle. It's designed to be
driven by an AI agent via the skill in [`skills/lofty-pm/`](skills/lofty-pm/) —
the CLI supplies deterministic plumbing (scan/parse/state/deadlines/
reminders), the agent supplies judgment (reading proposals, weighing them
against the owner's stance, conversation). Built on the shared
[`cli-common`](https://github.com/piekstra/cli-common) `pk-cli-*` crates
(core, config, auth, self-update, MCP), pinned to **v0.27.0**.

This repo is public: no personal data anywhere — no account emails, no real
portfolio details, no real property addresses — in code, docs, fixtures, or git
history.

## Build, test, lint

```console
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all --locked
```

Run all three before considering a change done — it's exactly what CI's
`verify` job runs (`.github/workflows/ci.yml`), on both macOS and Linux.
`make verify` runs fmt, clippy and the tests plus an offline smoke of the
release binary. `make install` installs from `Cargo.lock` (`--locked`) and
re-signs the binary with the shared dev identity (see the `Makefile`).

## Layout

- `src/main.rs` — clap command tree, arg validation, exit-code mapping.
- `src/commands/*.rs` — one module per top-level command (scan, votes,
  recommend, decide, pending, stance, digest, ledger, verify, init). Each
  renders a human summary and a `--json` DTO from the same data.
- `src/model.rs` — `VoteRecord`/`VoteState`/`Actor` and the vote state
  machine (`advance`, legal-transition checks).
- `src/mcp.rs` — `lofty-pm mcp`: read-only tools over `ReadStore`, which offers reads
  alone and creates nothing. Its module doc says what is withheld and why.
  `tests/mcp_readonly.rs` holds every tool to leaving the data directory byte for byte.
- `src/store.rs` — plain-JSON file store with atomic writes; no database.
  Data dir resolves `$LOFTY_PM_DATA_DIR`, else
  `${XDG_DATA_HOME:-~/.local/share}/lofty-pm`, with `$USERPROFILE` for `~` where `$HOME`
  is unset (deliberately XDG-style, not
  `~/Library/Application Support`, for family consistency and easy test
  overriding).
- `src/gmail.rs` — thin `gro` subprocess wrapper (search/read/label), with a
  tolerant parser for its output.
- `src/parse.rs` — Lofty email classifier + field extractors. Template
  drift produces a `needs-manual-parse` vote rather than failing the scan.
- `src/deadline.rs` — the missed-deadline rule: `sweep_deadlines` writes it (used by `scan`
  and `pending`), and `lapsed` computes the same answer without writing, for `mcp`.
- `src/config.rs` — non-secret config (`gro_bin`/`gro_ref`/`lofty_bin`/
  `grw_bin`/per-property `modes`/reminder thresholds/high-stakes threshold).
- `skills/lofty-pm/SKILL.md` — the agent skill: the five operating duties,
  written to be portable across agent runtimes.
- `docs/compliance.md` — the terms-of-service analysis behind the design.
- `tests/` — offline integration tests (`cli_surface.rs`, `scan_flow.rs`)
  plus `tests/fixtures/emails/` (synthetic — see below) and
  `tests/stubs/gro` (a shell stub standing in for the real `gro` binary).

## Conventions (do not break these)

- **`--json` on every command** (except `completions`, which always prints
  the shell script — matches `lofty-cli`'s family precedent), emitting one
  DTO tagged with a `schema` field (e.g. `"schema":"vote-list/v1"`). Human
  output → stdout as short lines; diagnostics → stderr. Keep the human and
  JSON paths in sync. A breaking DTO change bumps the `/vN` suffix.
- **Exit codes:** 0 ok · 2 usage · 3 auth · 4 not found · 5 upstream · 6
  confirmation required, per `piekstra-cli/1`.
- **This CLI holds no credentials of its own** — `auth status` always
  reports `required: false`. The one keychain item is the bearer token `lofty-pm mcp
  --new-token` makes for `--http`. Mail access is `gro`, always read-only;
  Lofty account/property data is the `lofty` CLI. Never call the Lofty API
  or scrape lofty.ai pages directly from this codebase — see
  `docs/compliance.md`'s data-sourcing rule.
- **Vote casting is out of scope for v1.** Every accept/override path ends
  by printing a Vote Now link for a human to click; nothing in this repo
  submits a vote. Don't add code that does. `lofty-pm mcp` does not return that
  link at all.
- **Tests are offline, always.** No live mail, no live API, no network. The
  gro stub (`tests/stubs/gro`) and `tests/fixtures/emails/*.txt` stand in for
  real inbox data; `LOFTY_PM_NOW` overrides the clock for deadline tests.
- **No personal data in fixtures or docs — ever.** Fixture emails use
  `investor@example.com`, made-up addresses/dollar amounts, and synthetic
  vote/property IDs. Any new fixture must stay that way.

## Definition of done

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and
`cargo test` all green; new behavior has test coverage; `--json` and human
output both updated for any command you touch; no secrets or personal data
in the diff.
