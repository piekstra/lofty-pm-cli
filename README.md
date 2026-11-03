# lofty-pm — Lofty governance assistant

[![CI](https://github.com/piekstra/lofty-pm-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/piekstra/lofty-pm-cli/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/piekstra/lofty-pm-cli?sort=semver)](https://github.com/piekstra/lofty-pm-cli/releases)

An agent-first command line for [Lofty](https://www.lofty.ai) investors. It
watches your email for property governance votes and property updates, has
an AI agent analyze each proposal against your recorded investment stance,
and tracks the whole recommend → decide → cast → outcome lifecycle in an
auditable local ledger. Default mode recommends and waits for you; an
opt-in, double-gated auto mode can treat routine recommendations as
pre-approved — but a human always clicks the vote.

`lofty-pm` supplies the deterministic plumbing (scanning, parsing, state,
deadlines, reminders); a driving AI agent — this repo ships a Claude Code
skill for one — supplies the judgment (reading proposals, weighing them
against your stance, conversing with you). All mail access goes through the
[`gro`](https://github.com/open-cli-collective/google-readonly) binary and
all Lofty account/property data through the
[`lofty`](https://github.com/piekstra/lofty-cli) binary — this CLI holds no
credentials of its own and makes no direct calls to lofty.ai. `lofty-pm mcp` serves
what it has filed to Claude, read-only.

> **Not affiliated with or endorsed by Lofty.** An independent, unofficial
> tool. See [`docs/compliance.md`](docs/compliance.md) for the
> terms-of-service analysis behind its design, including why v1 never casts
> a vote itself.

## Install

```console
# from source (recent stable Rust; MSRV 1.89)
$ cargo install --git https://github.com/piekstra/lofty-pm-cli --locked

# or clone + build
$ git clone https://github.com/piekstra/lofty-pm-cli && cd lofty-pm-cli
$ make install        # cargo install --path . --locked --force
```

Prebuilt binaries for macOS (Apple Silicon and Intel) and Linux are attached
to each [release](https://github.com/piekstra/lofty-pm-cli/releases); once
installed, `lofty-pm self-update` upgrades in place.

Requires [`gro`](https://github.com/open-cli-collective/google-readonly)
(read-only Gmail access — required) and
[`lofty`](https://github.com/piekstra/lofty-cli) (property/market data —
optional; `lofty-pm` degrades to email-only analysis without it).
[`grw`](https://github.com/open-cli-collective/google-readwrite) is optional,
only used to label processed threads if you want that.

## Quickstart

```console
$ lofty-pm init                      # checks gro/lofty presence + auth, mailbox coverage
```

Then hand the CLI to an agent (Claude Code with the `lofty-pm` skill in
[`skills/`](skills/lofty-pm/SKILL.md), or any other runtime) and have it:

1. Administer the interview from `lofty-pm stance questions --json`, writing
   answers back with `lofty-pm stance set`.
2. Run `lofty-pm scan --json` on a cadence — a cron job, a scheduled agent,
   or `/loop 30m lofty-pm scan` in Claude Code.
3. Check `lofty-pm pending --json` for votes awaiting analysis, votes
   awaiting your decision, and deadline reminders, and act on them.

Everything the CLI does is inspectable directly, too:

```console
$ lofty-pm votes list --json
$ lofty-pm votes context <id> --json   # what an agent sees before recommending
$ lofty-pm decide <id> --accept        # prints the Vote Now link — you click it
$ lofty-pm ledger --json               # full audit history
```

## Modes

| Mode | Per-property | Behavior |
|---|---|---|
| **Recommend** (default) | `mode.<property> = recommend` | Agent recommends; you decide; never casts, even past the deadline (a missed deadline is recorded as `missed`). |
| **Auto** (opt-in) | `mode.<property> = auto` | `routine`-tier recommendations are treated as pre-approved without waiting for a separate go-ahead. `high-stakes` votes (sell, refinance, large special assessment) always escalate to recommend-mode behavior regardless. Every auto action still notifies, and a human still clicks the Vote Now link — auto mode never casts a vote by itself. |

Auto mode is gated twice before `lofty-pm config set mode.<property> auto`
will succeed: all three ToS verifications in
[`docs/compliance.md`](docs/compliance.md) must be recorded
(`lofty-pm verify list --json` → `auto_mode_unlocked: true`), and enabling it
requires an explicit `--acknowledge-risk` flag every time — it is an
at-your-own-risk opt-in, not a default.

## Use with Claude

`lofty-pm mcp` serves four read-only tools to Claude over MCP: `loftypm_pending`,
`loftypm_votes`, `loftypm_digest` and `loftypm_ledger`. They read the local store
`lofty-pm scan` fills, and reach neither Lofty nor your mailbox. Nothing can cast a vote,
record a recommendation or a decision, or change anything. A vote is cast by you, from
the link in your email; no tool returns that link, or any other URL in a proposal or an
email. Results carry `last_scan`, and say when that scan could not read some mail or was
narrowed, so run `lofty-pm scan` to pick up new mail. If `scan_narrowed` stays after that,
your query reached the search limit of 100 emails: `lofty-pm config set scan_query
<narrower query>` clears it.

**Claude Code.** After `lofty-pm scan`:

```sh
claude mcp add loftypm -- lofty-pm mcp
```

Or install the `lofty-pm-cli` plugin from
[claude-plugins](https://github.com/piekstra/claude-plugins), which runs the same
command.

**Claude Desktop.** Download `lofty-pm-<version>.mcpb` from the
[latest release](https://github.com/piekstra/lofty-pm-cli/releases/latest) and open it.
It holds no credential: the bundled `lofty-pm` reads the store your installed `lofty-pm`
keeps. Its one setting is that directory, `~/.local/share/lofty-pm` by default. A desktop
app does not see what your shell sets, so change it if you set `LOFTY_PM_DATA_DIR` or
`XDG_DATA_HOME` there.

Any other MCP client that launches a local server can run `lofty-pm mcp`.

**Claude on the web and mobile.** claude.ai reaches tools through a remote connector,
and your store stays on your computer, so `lofty-pm` serves the same tools from your
computer to a connector you deploy into your own Cloudflare account:

```console
$ lofty-pm mcp --new-token            # once: a bearer token, saved in the keychain and printed for the Worker
$ lofty-pm mcp --http --port 8752     # serve on 127.0.0.1 only
```

The connector, [cli-common's `connector/`](https://github.com/piekstra/cli-common/tree/main/connector#tools-from-your-computer-home-modules),
reaches that port through a Cloudflare Tunnel in your account; its README walks through
the tunnel, the VPC service, a launchd agent that keeps the server running, and the
`HOME_LOFTYPM_TOKEN` secret, with the threat model. `--http` binds 127.0.0.1 only,
refuses to start without the token, refuses any request whose `Host` is not loopback or
that carries an `Origin`, and serves the read-only tools only. Tool results do leave your
computer: property names, vote subjects and proposals, recommendations, your decisions
and email excerpts pass through Cloudflare and reach Claude
([`SECURITY.md`](SECURITY.md)). While the computer sleeps, the tools answer that it is
offline.

After `lofty-pm self-update`, restart the server: a running one keeps the old code, and
its launchd agent restarts it only when it exits. Its start line names the version.

```console
$ launchctl kickstart -k gui/$(id -u)/com.piekstra.lofty-pm-mcp-http
```

`self-update` does not reach a Claude Desktop extension, which runs its own copy.
Install the new release's `.mcpb` too.

## Conformance

Conforms to piekstra-cli spec v1 ([`cli-common`](https://github.com/piekstra/cli-common)):
`--json` with schema-versioned DTOs on every command, frozen exit codes 0–6,
`self-update`, `info` (`cli-info/v1`), shell `completions`, and the MCP profile.

## Development

```console
$ cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

See [`AGENTS.md`](AGENTS.md) for the fuller house rules.

## License

MIT. See [`LICENSE`](LICENSE).
