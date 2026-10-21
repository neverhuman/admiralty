# Admiralty operations upgrade: source review and implementation plan

Reviewed baseline: `5916726f8c085f2687ec756508914c5a713fa5bc` on `main`.
Request: Rust/Vite/TypeScript/React, cleaner deployment, efficient performance,
agent-consumable logs, Git health and clearer live-data layout.

## 1. Findings and decisions

Admiralty already has the right application architecture: Rust domain, SQLite
Store, runtime, Axum daemon/CLI/observer, generated contracts and React/Vite.
Rebuilding these layers would discard substantial correctness work. Source
preview capability must remain separate from managed execution qualification.

| Finding at this baseline | Consequence | Decision |
| --- | --- | --- |
| Rust Git-health evaluator already works over cached graph/checkout observations | Another per-browser Git scanner would add cost and conflicting meanings | Reuse those reports; improve fleet-wide discovery |
| Compact context, durable role renders and attention deduplication already exist | A second summarization broker would create competing state | Keep authoritative coordination there; add an explicitly lossy operator brief |
| `scripts/dev/live.sh` invokes inline Python just to initialize credentials | The recommended live launch still has a Python runtime dependency | Implement Rust `preview-init`, reusing daemon custody rules |
| Observer success/failure output is mostly prose and exposes only overall duration | Agents must parse text; bottlenecks are difficult to locate | Versioned JSON lines and five stage wall-time measurements |
| Publication, Git source and ledger clocks have different meanings | A green connection indicator can obscure old evidence | Display publication and Git clocks independently |
| Detailed console includes large evidence/history while native Overview describes a different scope | Operators must navigate several views to decide what needs attention | Add a dedicated, lazy-loaded operations route with drill-down |
| Source/working/install/running identities and full release evidence remain separate | A source PR cannot claim a new deployed release | Keep this PR source-only; document a serialized deployment handoff |
| An active Git workspace PR (#167) owns graph/camera work | Overlapping graph changes risk duplicate integration work | Do not modify graph geometry/canvas/viewport or the active branch |

This source lane owns the new operations feature and browser suite, minimal
App/router wiring, CLI preview module/dispatch, observer telemetry/timing, live
launcher and these operator/review notes. It changes no Store migrations, public
contract generator/schema, grants, provider executor, production configuration or
services. A draft handoff may coexist with the existing active integration PR;
the maintainer should serialize integration with #167.

## 2. Implemented slice

The operator guide is [fleet-operations.md](../../operators/fleet-operations.md).

### Frontend

`apps/dashboard/src/features/operations/` contains the projection/packet compiler,
view, scoped polling and styles. `#/operations` appears in the main navigation.
It reads the existing initial detail projection, avoiding commit graphs/history,
and does not open an unused native fleet/event feed. Requests do not overlap;
hidden pages stop polling. Rendering is bounded at 50 project rows and 64 host
summaries. Attention, incomplete coverage, disabled policy and source clocks
remain distinct; counts can overlap.

An explicit JSON brief retains source identities, reason codes and omissions.
It does not summarize transcripts or model-generate an explanation. The full
evidence link is retained, and copying is a user action. No token-saving or
decision-fidelity claim is made from a smaller byte count.

### Native observer

`crates/admiralty-observe/src/telemetry.rs` emits fixed, allowlisted event records.
`lib.rs` measures existing adapter stages without new Git/subprocess calls.
`main.rs` defaults publication logs to JSON and offers text compatibility.
A stable health digest removes time-only churn from the change signal while
every heartbeat still carries its actual source clock. Existing durable Git
alerts and role inbox delivery remain authoritative for notifications.

### Preview startup

`crates/admiralty-cli/src/preview.rs` replaces Python-only credential preparation.
Tests cover idempotence, exact daemon compatibility, no secret in result, private
modes, refusal of shared paths, intermediate/final symlinks and hard-linked tokens,
and preservation of refused existing credentials. The shell wrapper remains
orchestration glue; serving, observation and credential preparation are Rust.

## 3. Remaining work to reach the requested stack

This PR removes inline Python from the ordinary preview launch. It does **not**
claim that the repository is Python-free. The retained `.py` inventory includes
compatibility collectors, audit utilities, demo encoding, release qualification
and their tests. [PY_TO_RUST_PLAN.md](../../../PY_TO_RUST_PLAN.md) and
[PYTHON_EXCEPTIONS.md](../../../PYTHON_EXCEPTIONS.md) already assign retirement.

| Next bounded migration | Existing Rust destination | Required evidence before retiring Python |
| --- | --- | --- |
| Isolated canary and excursions harnesses | `admiralty-testkit` / integration tests | Existing positive/negative behavior, timeouts, child cleanup and diagnostic parity |
| Source packaging and release-evidence verifier | CLI native package/archive/install modules | Deterministic bytes where specified, malformed input, integrity/no-overwrite, platform and actual archive round trips |
| Fleet audit and queue metadata helper | CLI audit module + core policy | Unknown source handling, stale/partial reports, read-only behavior and all original regression controls |
| Rollout/screen/history helpers | Observer and Store exports | Fixture parity, source identity/age, bounded reads, no source mutation |
| Legacy compatibility tests/sources | Rust integration tests | Consumer inventory, archived originals, no remaining owned process/respawn and passed replacement tests |
| Demo encoders/verifiers | Rust CLI/tool binary | Pixel/frame/duration parity, bounded image decoding and format attribution |

Do not delete legacy tests merely to get a zero-Python file count. JS/TypeScript
build tooling and small shell launchers remain development glue; there is no
Node production service. Existing external Git/SSH/screen/OS/provider tools need
an explicit supported-tool inventory, not an unsupported claim of zero binaries.

## 4. Deployment upgrade sequence

Use the existing Rust release stage → archive → private version install path;
do not introduce a second installer or use `vite preview` as production serving.
The Rust daemon serves the Vite production assets. Upstream Vite makes this
production/preview distinction explicit:
<https://vite.dev/guide/static-deploy>.

1. Build the pinned Rust and Vite artifacts from one clean source SHA in CI.
   Record Cargo.lock/pnpm-lock hashes, target triple, toolchain versions and
   public-contract digest. Keep build tooling outside the runtime payload.
2. Feed the existing explicit staging allowlist: daemon, CLI, observer, dashboard,
   required schemas/guards, license and reviewed third-party notices. Derive
   artifact hashes from bytes; source labels alone are not provenance.
3. Generate an SBOM and authenticated build provenance tied to archive and raw
   manifest hashes. Signing identity and publication destination must be explicit.
   A checksum copied from the same unreviewed source is only an integrity pin.
4. Qualify actual clean Linux x86-64 and macOS arm64 private installations with
   no Python/Node runtime present. Verify doctor, demo, native static assets,
   observation ingestion, signals and shutdown; retain refusal controls.
5. Have the sole deployment owner back up state, record schema/watermark and
   active bundle pins, stage the new version, then perform an isolated canary.
   Existing sessions/unknown effects must be reconciled before replacement.
6. Activate through a separately reviewed platform service definition. Preserve
   old version/config and define a migration-compatible rollback; an older binary
   must not open an incompatible migrated database. Do not infer activation from
   successful extraction or a healthy HTTP process.
7. Verify the actual running source/binary/assets/contract identity, rendered UI,
   source freshness, authenticated commands and protected read boundaries.
   Record one deployment receipt and hand custody back to that owner.

Future service packaging should use one unprivileged controller, a separate
observer, private state/config paths, bounded restart/backoff, explicit limits and
structured stdout/stderr. Split session-host lifetime from controller upgrades.
Systemd hardening and launchd settings need actual platform qualification against
Git/SSH/provider/file access; do not ship an untested blanket sandbox policy.

## 5. Metric backlog: what to track next

Every metric needs unit, source clock, collection clock, scope, coverage, sample
window and versioned definition. Unknown is null; partial observations retain
their denominator/lower-bound meaning. Avoid a universal weighted health score.

| Domain / metric | Definition / evidence | Next implementation location |
| --- | --- | --- |
| Git integration drift | Existing proved ahead/behind and lower bounds against configured ref | Reuse `admiralty-core::git_health`; chart only comparable complete samples |
| Unresolved Git work | Conflicts, detached unique work, dirty integration checkout, unknown working state | Existing report/issue codes; retain preservation semantics |
| Review readiness | Exact head approval, required checks and base-change invalidation | Integration projection; never infer from latest generic green status |
| Branch/check-out age | Source-observed last change and ownership, with known/unknown activity | Observer history; age is a prioritization signal, not a stalled-worker verdict |
| Delivery lead time | Contract ready → trusted accepted at its required stage | Store event cohorts, including failed/reopened tasks |
| CI reliability | Queue wait, execution p50/p95, retries, fail categories and last exact-head result | Forge evidence adapter + integration view |
| Collector health | Stage durations, cycle overrun, error counts, source gaps and last successful publication | Stage measurements in this PR; durable rolling aggregation next |
| HTTP responsiveness | Bounded route-template/status counts, service p50/p95/p99 and timeouts | Server middleware; no token, raw URL/query or task text labels |
| Streaming quality | Active clients, replay/resnapshot counts, lag, slow-consumer drops and reconnect reasons | Native/visual SSE services; avoid per-client unbounded labels |
| Store pressure | Writer queue depth/wait, transaction latency, busy/locked errors and actual WAL/checkpoint result | Store writer instrumentation; do not perform checkpoint work on metric reads |
| Persistence safety | Last verified backup watermark/age, restore rehearsal, disk headroom and spool reserve | Store/asset projections; retain unknown effect outcomes |
| Fleet capacity | Host and worker CPU/memory/disk/GPU coverage with measured process ownership | Observer/fleet projections; capacity does not grant reservations |
| Agent context | Rendered UTF-8 bytes, actual provider tokens if observed, retrieval count/latency and cache coverage | Existing durable context render/usage records |
| Compression quality | Protected-fact round trip, negation/unit/revision preservation and paired decision outcomes | Context compiler qualification; lower bytes alone do not pass |
| Agent economics | Cost per accepted root including review, coordination, failed/retried attempts and unallocated spend | Existing economics cohorts/usage coverage, no heuristic acceptance |
| Deployment drift | Built/staged/installed/running source/binary/assets/schema identities and migration compatibility | Release/install receipts + authenticated runtime information |

Do not emit high-cardinality task IDs, paths, commit subjects or prompts as metric
labels. Keep them in bounded event/evidence records with scoped retrieval.
OpenTelemetry's log model distinguishes source and observation time and supports
severity and trace correlation; a future adapter should preserve those meanings:
<https://opentelemetry.io/docs/specs/otel/logs/data-model/>.
This PR emits local JSON; it does not add an external telemetry service or claim
OTLP wire-format compliance.

## 6. Design and performance follow-ups

Keep the operations page for triage, Git Flow for repository investigation,
Fleet Atlas for relationships, and detail for dense project evidence. Preserve
distinct native task and producer observation scopes. Add saved URL filters and
a command palette after the new route has real operator feedback. Keep the
current landing route until that comparison is measured.

The largest frontend opportunity remains the eager main chunk: detailed evidence,
schema validators and several views load before they are used. Split the detail,
model and task screens at route boundaries, measuring first render, keyboard
interactivity and gzip/Brotli bytes before/after. Do not remove validators to make
the bundle smaller. The new operations route is already lazy-loaded; its shared
Git-health parser can remain a shared chunk.

The initial source endpoint still returns other current summary fields and full
health reports. This is not a minimal dedicated operations wire projection.
If measured payload/parse cost warrants it, add a versioned typed projection
generated through the existing schema pipeline, ETag/304 keyed to its actual
representation, cached encoding and a bounded replayable delta channel. Preserve
source clocks independently of transport receipt time and verify parity first.

Performance qualification should compare the same source and browser at 10, 100
and 2,000 projects: compressed response bytes, request count, parsing/projection
time, DOM nodes, first render and p95 interaction. Include stale, partial, denied,
reconnecting and missing-source cases. Establish idle server CPU/RSS and p95
cycle/service latency before setting budgets. The current stage times and build
sizes are observations; they are not fleet-scale throughput results.

## 7. Review and acceptance

Tests added here cover malformed report isolation, unknown/disabled distinctions,
old/future clocks, bounded rendering/briefs, omitted evidence, no raw source prose,
route behavior, private credential refusal/idempotence and log digest semantics.
The browser suite uses the actual isolated Rust server and checks filtering,
pagination, access removal, request scope, dark/light/mobile accessibility and
screenshots. Existing local/hosted CI discovers these suites without another
workflow or service controller.

Run `pnpm check` before publication. Record the actual source/dirty state, command
exits, tool versions, original failures and environment limitations. An unavailable
browser sandbox, filesystem capacity or platform primitive is not a product pass.
Hosted checks and exact-head independent review remain separate gates. Keep this
handoff draft until those gates and the existing integration slot are resolved;
never bypass the normal matching-head merge rule.
