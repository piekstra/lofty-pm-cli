---
name: lofty-pm
description: Use when monitoring Lofty governance votes, analyzing proposals against the owner's stance, administering the investor interview, or handling Lofty property emails
---

# lofty-pm

`lofty-pm` is deterministic plumbing: it reads mail, files votes, tracks
deadlines, and keeps an audit ledger. It never judges a proposal and it
never casts a vote. That's this skill's job. Read `docs/compliance.md` in
this repo before using auto mode — it explains why v1 casts nothing.

## Data-sourcing rule

> Proposal content comes from **email** (via `gro`, read-only). Account and
> property data comes from the **official API** (via `lofty`). Never scrape
> lofty.ai pages, never browse the site on the owner's behalf. This is what
> keeps every mode of this tool inside the clearly-permitted zone of Lofty's
> terms of service — see `docs/compliance.md`.

## The five duties

### 1. Setup & interview

Run `lofty-pm init` and fix any failing check before doing anything else —
most commonly a `gro` re-auth, or `gro_ref` needing to point at the profile
connected to the Lofty-registered mailbox (`lofty-pm config set gro_ref
<profile>`). `init` is a report, not a gate: it always exits 0, so read the
`checks` array yourself.

Administer the investor interview conversationally, **one question at a
time** — don't dump the whole list on the owner:

```
lofty-pm stance questions --json
```

For each field, ask, listen, then write it back:

```
lofty-pm stance set [--property <slug>] --by agent <field> <value>
```

(`--property` scopes the answer to one property; omit it for the global
profile. `--by agent` records that the agent — not the owner directly —
wrote the field, for provenance.)

Propose a re-interview when `stance show --json`'s `meta.<field>` timestamps
are more than 180 days old, or when the owner says something that
contradicts a stored answer.

### 2. Monitor

Run a scan on whatever cadence the runtime supports — a scheduled/cron
agent, or `/loop 30m lofty-pm scan` in Claude Code:

```
lofty-pm scan --json
```

Then check the work queue:

```
lofty-pm pending --json
```

If `scan` reports a `gro` auth error, surface it to the owner immediately —
never silently skip a poll and pretend nothing happened.

### 3. Analyze

For each vote in `pending`'s `awaiting_agent` list, pull the full analysis
bundle:

```
lofty-pm votes context <id> --json
```

Run the queries it suggests under `lofty_queries` (via the `lofty` CLI),
weigh the proposal against the bundle's resolved `stance`, write your
reasoning to a file, then record the recommendation:

```
lofty-pm recommend <id> --position <for|against|abstain|defer> \
  --confidence <0.0-1.0> --tier <routine|high-stakes> \
  --reasoning-file <path-or->
```

Real Lofty vote emails often state no machine-parseable deadline at all
(`parse.rs`'s `extract_deadline` intentionally returns `None` rather than
guess a time), so **after analyzing a new vote, determine its actual voting
deadline before moving on**: read it off the email text if a date is stated
there, and if not, assume Lofty's typical 3-5 day voting window from the
email's received date — flagging in your reasoning that the deadline is an
estimate, not a stated one. Record it with `lofty-pm votes amend <id>
--deadline <RFC3339>`; skip this and `pending`'s reminder ladder and the
missed-deadline sweep stay silently inert for that vote.

If the bundle's `context` is `"degraded": true` (the `lofty` CLI is
unreachable or unauthenticated), say so in the reasoning and lower the
stated confidence — don't recommend with the same certainty you'd have with
full data.

For a `needs-manual-parse` entry (template drift broke extraction), read the
vote's `raw` field (`votes show <id> --json`) yourself, then fix it up:

```
lofty-pm votes amend <id> --property <slug> --proposal-file <path-or-> \
  [--deadline <RFC3339>] [--link <url>] [--options "A/B/C"]
```

A tool result's `unread_emails` that stays after another scan names no email, by design:
`scan --json`'s `manual_parse` array lists them, to read by hand. A `scan_narrowed` that
stays means the query reached the search limit of 100 emails: `config set scan_query
<narrower query>` clears it.

If several scans in a row keep producing `manual_parse` entries (check
`scan --json`'s `manual_parse` array), tell the owner **once** that Lofty's
email template likely changed and `lofty-pm` needs a parser update — don't
repeat the same warning on every subsequent email.

### 4. Notify & decide

For everything in `pending`'s `reminders` list, and for any vote you just
finished recommending, notify the owner through the runtime's normal
mechanism. Include: position, confidence, deadline, and a one-line summary
of the reasoning — not the whole file. After notifying a reminder, silence
it so it doesn't repeat:

```
lofty-pm pending ack <vote-id> --level <N>
```

When the owner responds, record their decision and hand them the link the
command prints:

```
lofty-pm decide <id> --accept
lofty-pm decide <id> --override <position>
lofty-pm decide <id> --decline
```

**Never cast a vote yourself. Never click a Vote Now link on the owner's
behalf** — accept/override print the link; that's the handoff point, every
time, no exceptions.

**Auto mode** (per property, opt-in, double-gated) only changes the
notify/decide relationship — it never removes the human click. Treat a
recommendation as pre-approved only when *all three* hold:

- `lofty-pm verify list --json` reports `"auto_mode_unlocked": true`
- the vote's property is configured `mode.<property> = auto`
  (`lofty-pm config show --json`)
- the recommendation's tier is `routine` (never `high-stakes` — those
  always escalate to a normal notify-and-wait)

Even then: still notify the owner, and still make them cast via the printed
link. "Auto" means *you* don't wait for a separate go-ahead before treating
the recommendation as decided — it does not mean the vote gets cast without
the owner.

### 5. Close the loop

`scan` closes most votes automatically when a result email arrives. For a
result that's ambiguous or that `scan` otherwise missed, close it by hand
after reading the result email from the corpus:

```
lofty-pm votes close <id> --outcome "<what happened>"
```

Mention outcomes in the next conversation with the owner. If they
repeatedly override your recommendations in the same direction, say so and
propose a stance update rather than quietly continuing to get overridden.
