# Changelog

All notable changes to `lofty-pm` are documented here. Newest first. The project follows
[SemVer](https://semver.org/); until 1.0, minors carry the breaking-change signal.

## 0.2.1 — 2026-11-03

- `loftypm_pending`, `loftypm_votes`, `loftypm_digest` and `loftypm_ledger` page by position, so a row
  that a scan adds between pages repeats nothing and skips nothing. Votes and emails of the same day
  now run in a fixed order of their own. A cursor from an earlier version still works, as the count
  of rows it holds.
- cli-common v0.27.0, up from v0.22.0.

## 0.2.0 — 2026-11-03

- `lofty-pm mcp`: four read-only tools for Claude: `loftypm_pending`, `loftypm_votes`,
  `loftypm_digest` and `loftypm_ledger`, over the local store. They run over stdio for
  Claude Code and Claude Desktop, and with `--http` on 127.0.0.1 behind a bearer token for
  Claude on the web and mobile through your own Cloudflare Worker and tunnel. No tool can
  cast a vote, record a decision or change anything, and none returns a voting link.
- `scan` records when it finished, how many emails it could not read and whether `--query`,
  `--max` or the search limit of 100 emails kept it from all the matching mail, in
  `scan.json`, so results can say how fresh they are.
- The store's default location falls back to `%USERPROFILE%` where `$HOME` is not set.
- Releases attach `lofty-pm-<version>.mcpb`, a Claude Desktop extension, and a Windows
  x86_64 tarball. A release still cuts itself when a new version merges.
- `self-update` says to restart a running `lofty-pm mcp --http` and to install the new
  `.mcpb`.
- cli-common v0.22.0, up from v0.15.1. Rust 1.89 or later.
- `SECURITY.md` is new.
