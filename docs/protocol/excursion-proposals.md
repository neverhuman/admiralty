# Local excursion proposals

Scope includes the original positive contract revision and full contract/protected-oracle digests. Episode identities hash this scope and their originating event; proposals bind typed scope and the exact episode digest. Current projections are reconstructed from events and compared with stored receipts before reuse. Forged receipt keys, changed episodes, invalid evidence digests, unsafe identifiers and known causal clocks running backward fail closed. Cumulative history is capped at 1,024 events with explicit backpressure and replay at capacity; state/input/output use streaming JSON bounds.

Investigation proposals allow at most eight steps and one proposed model turn, with `execution_granted: false`; admission and reservations remain external. Titles use fixed text instead of echoing unfamiliar family strings. Required reproduction fields cannot be blank or contain terminal controls, and unsafe inputs are refused rather than silently stripping constraints.

The pure reducer and proposal builder now have a durable Store journal and an explicitly configured HTTP adapter. They group events into episodes only when an explicit causal parent links them, and retain independent later recurrence as a separate episode. An identical event ID and canonical content is replay-safe; reusing an ID with different content is an integrity error. Causal traversal is iterative and depth bounded. Unknown parents, cross-scope inputs, cycles, and mismatched lineage families are rejected.

Inputs use the existing project, root, and task identifiers. The serialized input is limited to 1 MiB, collections to 1,024 items, event identifiers to 128 bytes, and reproduction values to 4,096 bytes. Optional tick values are canonical decimal `u64` strings. Clock observations are caller supplied; absence stays unknown. Missing attribution remains unknown, and time proximity is never treated as causation.

Proposal output contains only immutable SHA-256 evidence references, a bounded reproduction contract, a rule-based diagnosis for a small recognized signature set, and an uncertainty statement. Reproduction fields are allowlisted; embedded credential patterns, PEM markers, and URL-like values are refused. This is narrow screening, not perfect secret detection. Raw transcripts and arbitrary evidence text are never copied into proposals. Sanitization preserves caller supplied negation, failed criteria, scope, units, uncertainty, candidate fix, and measurable outcome when those permitted fields are included.

Unfamiliar signatures produce a bounded investigator proposal; no task is launched. The output always has `external_publication_allowed: false`. The pure builder has no provider, issue, pull request, filesystem, or publication effects and does not establish acceptance or execution authority. A caller must independently authenticate evidence references, retain canonical events, and apply any separately configured publication grant through its gateway. Manual review is required before acting on a proposal.

## Durable service and operator access

Store records observations and sanitized proposals on its existing sole writer.
Every command carries the exact `scope`, a stable `command_id`, an
`expected_revision` and an operation tagged `observe` or `propose`. Observations
carry `incidents`; proposals carry `episode_id` and a `reproduction` contract.
The writer reconstructs the scope's command history, checks each retained receipt
against its reconstructed snapshot, and rechecks installed authority immediately
before commit. Exact command retries return their original receipt even after
later commands; changed command bytes collide. A stale revision, rejected input,
revoked policy or SQL failure records nothing. No journal entries are evicted.
Unkeyed hashes detect inconsistency; they do not authenticate a database owner
who rewrites the entire history.

Limits are 1 MiB per command, 2 MiB per reconstructed snapshot, 1,024 commands and
8 MiB of command/receipt JSON per scope, 32 proposals per scope, 128 scopes and
32 MiB of total journal JSON. These are application bounds, not SQLite/WAL disk
quotas. Scope history is verified on access; initialization does not execute
proposals or claim a fleet-wide incident audit.

The private `--native-config` optionally accepts `excursions`, an array of
explicit grants. Each contains a full `ExcursionScope`, `actions` chosen from
`read`, `observe`, `propose`, and `evidence_digests` authenticated by the operator
for that exact scope. The existing owner bearer credential is usable for these
actions only when those grants are explicitly installed. Neither operator
project access nor agent inbox grants imply incident access. Configuration is a
startup snapshot; edit the private file and restart to change it. Trusted embedded
callers can revoke the in-memory policy immediately. Granting a digest attests
the reference's scope; the incident service never fetches its content.

POST `/v2/incidents/scopes` with `{}` lists readable granted scopes;
POST `/v2/incidents/read` accepts one full scope;
POST `/v2/incidents/commands` accepts a command. All require a bearer header and
`Content-Type: application/json`. Browser requests additionally require matching
loopback Origin/Host and `x-admiralty-requested-with: incidents`. Responses are
bounded; successful data responses are `no-store`. No incident routes are
installed when no grants are configured.

The Incidents dashboard loads those granted scopes, displays recurrence,
lineage, evidence references and uncertainty, and prepares a sanitized local
proposal with reproduction, approaches already tried, suggested change and
measurable follow-up. An uncertain response retains the same command for retry.
Tokens remain in page memory; the UI does not fetch transcripts or publish to a
forge. Historical proposal digests remain bound to their original episode
snapshot when later observations extend the episode. Deterministic diagnosis
and the existing eight-step/one-model-turn investigation proposal are recorded
locally; scheduling any investigator remains a separate authorized operation.
