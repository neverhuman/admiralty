# Native coordination, scheduling and import components

These APIs are registered components. They install no production authority,
transport, scheduler, provider or publication service.

## Coordination headers

`admiralty_core::coordination::envelope::CoordinationEnvelopeV2::parse` accepts
at most16KiB and16 nesting levels. Version2 is explicit; existing v1 remains
unchanged. Complete routing, task revision, causation/correlation, four-field
cursor, policy/scope generation, epoch/baseline, protected facts, payload
SHA/size, kind, priority and expiry remain on the machine wire. Nullable metadata
must be present even when null. Decimal counters retain their exact u64 strings.

`validate_for` compares independently retained complete bindings and header
digest. `verify_payload_bytes` checks exact bytes; it establishes neither body
semantics nor artifact custody. Expiry uses supplied observation time and never
silently drops a message. Claimed authority and priority issue no permission.
The23-root schema/TypeScript/guard pipeline includes this header. Native package
staging explicitly includes its validator sibling. The previous22 schemas,
22 types and19 individual/base guard modules retain their bytes.

## Scheduling

`admiralty_core::scheduling_profile` resolves explicit profiles and observation
times with pinned `jiff0.2.38`, `jiff-tzdb0.1.9` and actual IANA2026e TZif bytes.
System timezone, environment and global database fallback are disabled.
Profiles bound32 windows,366 blocked dates and64KiB. Gap handling skips the
window; folds choose earlier/later or reject explicitly. Overnight holidays and
shift identity use the local start date. New resolution validates the current
zone even when the date is otherwise inapplicable.

Retained UTC endpoints, offsets, timezone identity and an independently retained
record digest support historical replay without consulting the current bundle.
Eligibility considers a bounded five-date neighborhood and keeps unsupported
neighboring instants from invalidating an otherwise valid observation. Setup,
environment, resource, quota and decision waits remain independent. Externally
granted overrides waive only the window and never other admission checks.
Every eligibility result has `execution_granted=false`. Durable window storage,
current policy/observation authentication and actual scheduler admission remain
integration work. Scheduling types are not yet public generated roots.

## Read-only todoq observations

`admiralty_core::import::todoq::parse_pinned_snapshot` consumes supplied family
TOML and explicit Markdown frontmatter records using the real TOML grammar.
The adapter was studied against pinned driver revision
`b26ddcbfc12e6be527093dd3afa3e63040c03d1a`. Fixtures are synthetic; private queue
bodies were not copied or fetched. Full snapshots are bounded to1MiB and1000
records, with explicit depth/node/edge/diagnostic caps.

Repository/ref/commit/blob/path values remain supplied claims; raw SHA checks
do not prove Git tree membership. Missing scope, dependencies and criteria stay
unresolved. Imported commands, status, claimant and merge text grant no effect,
lease or acceptance. Every page retains family context and maps each page-local
source index to the exact source bytes, pin and span; original record offsets
and snapshot-bound continuations remain explicit. Authenticated acquisition,
reviewed enrichment, recording and dispatch are separate operations.

## Current attention authorization

Protected Store attention mutations require a startup-installed
`AttentionAuthority` and an opaque in-process actor proof. Matching JSON labels,
context hashes or read credentials do not supply that permission. The sole
writer validates the actual retained capsule/task bindings, checks current
authority before historical retries and checks again before committing a new
command or returning its recorded result. Unbound Store mutations deny.

`NativeAttentionWriterAuthority` composes the real credential registry and an
independently installed exact action policy. Registry contention and revocation
deny; registry locks do not span an external policy callback or queue wait.
The policy still owns current role, source, task and evidence admission. Changes
outside the writer after its last callback remain the issuer's synchronization
responsibility. This is not an atomic transaction across an external policy DB.

Legacy history remains readable and replayable with its original receipt
formula. New commands retain their complete versioned binding while private
proofs remain in memory. Request context and observed pending evidence remain
separate: recording a wake neither compiles that batch into the context nor sends
provider input. No startup/router issuer or transport is activated here.

## Advisory native resource observations

An observed run retains its original cgroup directory descriptor. Fixed bounded
reads preserve first, latest and final PID/memory/event observations. Missing,
malformed, unbounded or collected metrics stay explicitly unknown. Sampling
does not alter CPU, memory, task, permit, process or closure checks. Native
executor polling samples at most every250ms; closure freezes its observations
for stable journal retries. Kernel collection can still precede a final read.

The opaque closure's `authoritative_binding()` retains its original five-field
serialization. New closed journals use explicit version2, a separate legacy
authority digest and advisory data; `native-evidence-v2:` identifies the full
new journal bytes. Probe authority comparisons use the original binding, and
the task-loop probe retains resource evidence before changed-source preflight.
These fixture-tested diagnostics do not retrospectively prove the prior
provider's EAGAIN cause or qualify a new Linux/provider run.
