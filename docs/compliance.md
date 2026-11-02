# Compliance: terms-of-service posture

`lofty-pm` is an independent, unofficial tool. It is not affiliated with or
endorsed by Lofty. This document is the condensed record of the
terms-of-service research behind its design; it is not legal advice, and it
covers Lofty's ToS as written at a point in time — re-read the live terms
before relying on this if meaningfully more time has passed.

- **Source:** `lofty.ai/terms`, last updated **2026-05-01**. Note Lofty (the
  fractional real-estate marketplace) is a distinct company from `lofty.com`
  (a CRM product); this research is scoped to `lofty.ai` only.
- **Researched:** 2026-07-24, alongside current and archived Lofty
  help/governance articles and the official SDK surface (`@loftyaicode/sdk`).

## The three verdicts

### 1. Reading the user's own email + AI analysis — clearly permitted

The ToS's only automation-restriction clause (**§4.1**) prohibits "data
mining, robots, scraping, or similar data gathering or extraction methods of
content or information from any of our Products." The user's own mailbox is
not a Lofty Product — `lofty-pm` never touches lofty.ai itself to do this;
it reads mail through `gro`, a read-only Gmail client, and uses the `lofty`
CLI (the sanctioned API surface) for account/property data. §4.1 simply
doesn't reach this.

### 2. Recommend-then-human-confirms — permitted

Governance votes arrive by email with a "Vote Now" button (window typically
3–5 days; decided by 60% supermajority). A human clicking that link
satisfies any reading of the ToS that requires the account holder to
personally take the voting action. `lofty-pm`'s default mode — and the only
mode this skill is allowed to treat as final without a human click — never
goes further than handing the owner that link.

### 3. Fully automated casting — gray area

Not prohibited anywhere in the ToS, but also outside its sanctioned API
surface: API keys gate writes by scope (a "Trading" toggle exists), and no
voting scope or endpoint exists at all today. Two points cut in favor of
automated assistance: Lofty itself auto-executes votes from its mandatory
onboarding survey, and it offers a "defer to the property manager's
recommendation" option — so the platform does not treat per-vote personal
confirmation as sacrosanct. Against that: the risk of getting this wrong is
account-level, since the account's usage rights are a **revocable license
(§3.1)**, and liability for how the tool is used sits with the user under
the ToS's **indemnification clause (§6(c))**. This is why fully automated
casting stays out of scope for v1, and why the auto mode that does exist
never removes the human click (see below).

## Data-sourcing rule

Every mode of this tool stays inside the clearly-permitted zone by
following one rule, everywhere, no exceptions:

- Proposal content comes from **email**, via `gro` (read-only).
- Account and property data comes from the **official API**, via `lofty`.
- Lofty.ai pages are **never** scraped, browsed, or otherwise automated
  against directly.

## What v1 actually does

**v1 casts nothing.** Every path — recommend mode and auto mode alike — ends
with the CLI printing a Vote Now link and a human clicking it. There is no
code in this repo that submits a vote to Lofty. Auto mode changes when a
recommendation is treated as decided (see the skill's "Notify & decide"
duty); it does not change who casts the vote.

**Auto mode is an at-your-own-risk opt-in.** Enabling it for a property
requires all three items below to be verified *and* an explicit
`--acknowledge-risk` flag every time it's turned on — there is no
interactive prompt to click through by accident. `high-stakes` tier votes
(sell, refinance, large special assessment) always escalate to a normal
notify-and-wait regardless of mode.

## The three required verifications

Auto mode refuses to enable for any property until all three are recorded.
Each needs a human with a logged-in Lofty account to actually look at the
thing — the CLI just records that it happened:

1. **Operating agreement** (app → property → Documents): member voting
   mechanics, any proxy/attorney-in-fact language, how non-votes are
   counted.
2. **API click-through terms** (Settings → API Keys): whether generating or
   using an API key involves accepting anything beyond the base ToS.
3. **Vote Now attestation**: whether the voting flow itself contains a
   personal-attestation step (something like "I personally cast this vote").

Record each one after reviewing it:

```
lofty-pm verify record operating-agreement --note "reviewed 2026-08-01, no proxy language, non-votes count as abstain"
lofty-pm verify record api-terms --note "no separate click-through beyond base ToS"
lofty-pm verify record vote-attestation --note "Vote Now flow has no personal-attestation step"
```

Check status at any time:

```
lofty-pm verify list --json
```

`auto_mode_unlocked` in that output flips to `true` only once all three are
present. Recording a verification is a statement of fact by whoever reviewed
it — don't record one you haven't actually checked.
