# Security Policy

## Reporting a vulnerability

Please report security issues privately via GitHub's
[security advisories](https://github.com/piekstra/lofty-pm-cli/security/advisories/new)
rather than a public issue.

## What this tool holds

`lofty-pm` holds no portal credentials. Mail is read through `gro` and Lofty data through
`lofty`, each with its own credentials. What it keeps is a plain-JSON store under
`$LOFTY_PM_DATA_DIR`, else `${XDG_DATA_HOME:-~/.local/share}/lofty-pm` (`$USERPROFILE` for
`~` where `$HOME` is unset): votes, filed email
text, your stance answers, recommendations, your decisions and an audit ledger. A vote's
record holds its Vote Now link. Anyone who can read that directory can read all of this.

## The MCP server

`lofty-pm mcp` serves read-only tools to an MCP client (Claude Code, Claude Desktop) over
stdio; it opens no port unless run with `--http` (below). **What the tools return goes to
Claude on every path**, as part of your conversation: property names, vote subjects and
states and deadlines, recommendations and your decisions, how votes came out, excerpts of
filed emails, and the audit ledger. A proposal's text, a vote's options, tier and
confidence, a recommendation's reasoning and a ledger entry's detail are added when the
model asks for `detailed`.

These are never returned, in any format: a vote's link, any URL in the text of a proposal
or an email (a Vote Now link reaches the ballot), the raw email, and your stance answers.
Message ids are not returned either, with one exception: a vote's id is `v-` and the id of
the email that announced it, because that is the name `lofty-pm` knows the vote by. It
opens nothing without access to your mailbox.

A tool reads files under the data directory and nothing else: no mail, no Lofty, no
subprocess, no config, no keychain. The server opens the store with a type that offers
reads alone and creates nothing, so no tool can change the data directory, or bring one
into being. A vote past its deadline reads as `missed` without the sweep `lofty-pm
pending` writes being run. An argument is compared against what the store lists and never
becomes a path, so it cannot reach a file outside the store. An error does not name a file:
those paths sit under your home directory. `tests/mcp_readonly.rs` drives every tool
through the real binary and fails on any change to the directory or any subprocess.

Text in results came from email. The server tells the model to treat it as data and never
as instructions, and no tool could act on such directions anyway.

The Claude Desktop extension (`.mcpb`) holds no credential. Its one setting is the store's
directory, which is passed as `LOFTY_PM_DATA_DIR`. Over stdio no keychain item is read at
all.

`lofty-pm mcp --http --port <PORT>` serves the same tools over HTTP for the user's own
Cloudflare Worker, which reaches it through the user's own Cloudflare Tunnel (cli-common's
`connector/`). It binds 127.0.0.1 only, with no flag for another address, and refuses to
start without a bearer token of 32 or more characters, from `LOFTY_PM_MCP_TOKEN` or the
keychain item `mcp-http-token` (`piekstra.lofty-pm`), which `lofty-pm mcp --new-token`
makes from 256 random bits and replaces only with `--overwrite`. Every request must carry
the token (compared in constant time) and a `Host` of `127.0.0.1` or `localhost`, and must
not carry an `Origin`; there are no CORS headers. Only the read-only tools are served (all
four are).

**Tool results also cross Cloudflare on this path**, through your tunnel and your Worker,
on their way to Claude. Treat the connector as you would anything that reads your votes
and decisions. Cloudflare is your own account's, and we run no shared service.
