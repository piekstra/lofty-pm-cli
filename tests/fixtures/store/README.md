# Store fixture

A synthetic `lofty-pm` data directory for the MCP tests. Nothing here came from a real
inbox or account: properties are `elm-st-12` and `oak-ave-34`, every amount is round,
and every link is on `example.invalid`.

| Vote | State on disk | Why it is here |
| --- | --- | --- |
| `v-decide` | `recommended` | waiting on the user's decision |
| `v-analyze` | `needs-analysis` | waiting on the analysis |
| `v-cast` | `accepted` | decided, not yet cast |
| `v-lapsed` | `recommended`, deadline in 2020 | reads as missed; no tool may write that |
| `v-closed` | `closed` | finished, has an outcome |
