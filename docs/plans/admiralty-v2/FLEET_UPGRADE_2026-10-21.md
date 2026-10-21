# Admiralty upgrade: source audit and delivery cuts

Initial audit baseline: `5916726f8c085f2687ec756508914c5a713fa5bc`, 21 October 2026.
Updated implementation base: `2f5b93157c3b0a2194929f08d166d18c1abaad43`.
Target: Rust services/CLI/tools and Vite/TypeScript/React UI, with no Python or
Node requirement in the installed application runtime. Source changes, local
qualification, integration and production acceptance remain separate facts.

## Decision and scope

Keep the current Rust/SQLite/event architecture. Make the fleet explain what
needs a decision, distinguish unknown coverage from known problems, and give
agents the smallest useful evidence slice. Do not replace durable coordination
contracts with summaries of screen transcripts.

This PR adds that inspection surface and removes Python from the live launcher.
It is a bounded preparation PR on top of the Git graph work from PR #167,
now included in main. It preserves that implementation and the current deployment
runbooks; the integrator reviews this follow-on cut. No merge, activation, production restart, fleet
enrollment or provider authority is claimed.

## Architecture audit

| Source | Existing implementation | Preserve / remaining gap |
| --- | --- | --- |
| Core `coordination/`, `context.rs` | Typed messages, scoped capsules and exact counters | Preserve durable references; measure actual context fidelity and complete cost |
| Store `writer.rs`, inbox/outbox, replay, backup | SQLite event/projection state and exclusive ownership | Preserve durability; finish live writer/queue/WAL telemetry and indexed paging |
| Server `views.rs` | Shared snapshots/SSE, frame bounds and retained observations after errors | Preserve clocks and retained state; share cached projections across delivery paths |
| Server `legacy.rs`, `http_bytes.rs` | Bounded reads, progressive detail, gzip and Server-Timing | Preserve auth/public-read distinction; avoid repeated parse/clone work per consumer |
| Observer `git.rs`, `git_health.rs` | One Git scan per cycle, derived health and persisted alert deduplication | Never add Git subprocesses to a browser/agent query |
| Core `git_health.rs` | Full-OID ancestry, lower bounds, missing parents, divergence, conflicts and detached work | Incomplete ancestry cannot prove safety; known conflicts still require review |
| UI `features/live-views/` and `features/git-flow/` | Git/agent inspectors, project scopes and source evidence | Add a short entry point; retain investigation depth |
| CLI `native_{package,archive,install,recovery}.rs` | Explicit inputs, hashes, deterministic archive/private install and restore proposals | Keep custody/no-overwrite; signing, clean-machine distribution and activation need their own proof |

A second backend, database or telemetry service would add migration cost without
closing source-coverage and qualification gaps. The runtime is already largely
Rust. Retained compatibility/qualification Python is still real source debt.

## Implemented cut

| Problem | Change | Exact paths |
| --- | --- | --- |
| Embedded Python creates live-preview credentials | Native `admiralty live`; installed-bundle launch; development wrapper only builds/forwards arguments | CLI `src/live.rs`, `src/main.rs`, `scripts/dev/live.sh` |
| Fleet risk requires loading large detail or individual graphs | Lazy Fleet health route, source banner, review/unknown/staleness cards, search and evidence table | `apps/dashboard/src/features/health/`, `ui/App.tsx`, `router.ts` |
| Entire feed is expensive agent context | At most 16 KiB HTTP JSON; offline CLI 2–16 KiB budget; explicit omissions, source digest and exact counters | Server `src/fleet_brief.rs`, `legacy.rs`, `lib.rs`; CLI `src/brief.rs` |
| Overall unknown Git state can hide known conflicts | Independent review count and conflict-first selection; full-scope aggregate counts before truncation | Brief compiler and health UI |
| Observer logs are prose | Opt-in JSONL with fixed events/codes, duration, bytes, coverage and changed-alert counts | Observer `src/logging.rs`, `main.rs` |
| Local CI assumes root-disk scratch | Optional `ADMIRALTY_CHECK_SCRATCH_ROOT`; same matrix, limits and assertions | `scripts/check/all.sh`, local CI guide |

The bounded projection limits response/context bytes for health inspection. It does not claim
less source collection, Store durability work, model tokens or end-to-end tail
latency. A brief uses the collected bounded source on a blocking worker and
performs no extra Git scan or inference. The health route does not also open a
native fleet stream. Detailed evidence remains available on demand.

## Finish the 100% stack migration

This PR does not claim all Python is removed. `PYTHON_EXCEPTIONS.md`,
`PY_TO_RUST_PLAN.md` and the policy test own the 42 inventoried compatibility/
qualification files at this baseline. Complete them with behavior parity:

| Cut | Port / retirement scope | Required evidence |
| --- | --- | --- |
| Native live launch (this PR) | Embedded initializer → Rust CLI | Owner-only creation, no-follow, exact token reuse, refusal and actual CLI serving |
| Release qualification | `tests/qualification/release_evidence.py`, remaining `scripts/release/*.py` uses | Duplicate-key/bounds/hash/negative-gate and archive parity; no signing inferred |
| Smoke harnesses | `scripts/check/{canary-smoke,excursions-smoke}.py` → Rust testkit | Same actual HTTP/auth/bad-input/restart journeys; retain original failures |
| Fleet tools | Audit, queue metadata, session classification, packet/history helpers → Rust observer/CLI | Same-owner handback; read-only/no-transport parity, unknown drafts and corrupt/WAL cases |
| Legacy consumers | Retained dashboard and parity fixtures | Retire all consumers, prove no respawn, then delete sources and update policy inventory |

Change inventory/ceilings only when files are actually retired. Keep original
receipts and negative tests. Provider CLIs and project toolchains have separate
dependencies from Admiralty's own runtime.

## Clean deployment

Build Vite assets once with pinned Node/pnpm. Build the three Rust release
binaries with pinned Rust and Cargo.lock. Use the existing native staging/
archive/install pipeline for explicit reviewed inputs: binaries, dashboard,
28 schemas, contract guards, license and needed compatibility HTML. Install in
a new version directory and verify both manifest and archive digests. A hash
alone does not authenticate the publisher; signing/publication retain their
distribution gate.

For read-only observation use the installed `admiralty live` command documented
in the [operator guide](../../operators/fleet-health.md). Managed/native
operation retains its protected daemon configuration and admission rules. The
deployment owner pins the exact version path and service entry point. Keep
state, credentials and source observations outside the release tree and Git.
Bind loopback and apply the existing authenticated tunnel/session policy.

Before touching the primary service, review exact source/build/package digests,
backup watermark, active bundle pins, running PID/writer ownership, paths and
policy generations, canary render/refresh, rollback command and database
compatibility. Test an independent private instance first. `/healthz` is
liveness, not evidence that observations are current, the browser works,
providers have capacity or tasks are accepted. The current `deploy/README.md`, `deploy/FULL_RELEASE.md`, `deploy/VERIFY.md` and
`deploy/ROLLBACK.md` runbooks and single service writer remain binding. This PR
changes Rust behavior, so the full-runtime path applies rather than a UI-only
overlay.

Next release CI: stage the exact-source bundle, produce SBOM/notices/provenance,
install in fresh isolated state, exercise demo and protected preview, reject
tampering, verify SIGTERM/open-SSE shutdown, test backup/restore and retained
version compatibility, then publish signed manifests through the configured
publisher. Keep Linux x86-64/arm64/macOS evidence separate. Ordinary PR CI needs
no private providers, production credentials or service restart. A container is
optional distribution work; it does not automatically qualify fleet source
access, process custody or native provider execution.

## Additional live and Git metrics

Each metric needs clock, scope, unit, coverage and method. Keep independent
signals instead of one opaque health score. Sample expensive measurements at
collection time, not inside a UI request.

| Signal | Decision it supports | Source / rule |
| --- | --- | --- |
| Observer failures, lag, deadline overruns and per-source age | Detect a dead feed behind a healthy server | Observer `lib/fleet/git/usage/history.rs`; per-source duration/error counters, monotonic elapsed and original wall clocks |
| Source-to-view p50/p95/p99, transport bytes, reconnect/resnapshot counts | Locate collection/projection/network/render delay | Server `views/http_bytes.rs`, UI `events.ts`; correlate revisions, exclude bearer/payload text |
| Writer depth, queue wait, commit/sync latency, WAL bytes and checkpoint busy result | Find storage and long-reader bottlenecks | Store `writer/lib/backup.rs`; actual transaction turns, retain WAL/FULL |
| Oldest inbox/outbox age, contiguous ACK lag, retries/duplicates/collisions | Separate receipt from applied/accepted work | Store and runtime dispatch; recipient scope/cursor/reason, no transcript |
| Spool/log bytes, disk/inode headroom, failed appends and retention | Prevent evidence loss before capacity is exhausted | Session spool/Store custody; emergency control reserve and visible failed writes |
| Git history/frontier coverage, repository identity and remote freshness | Avoid presenting local refs as current upstream truth | Observer `git.rs`; shallow/truncated boundaries and last independently observed remote result |
| Upstream divergence, unpublished tips and oldest unintegrated branch | Find stranded work and integration debt | Extend observer explicitly; local remote-tracking ref is not a fresh forge query; unknown with incomplete history |
| Dirty integration checkout, conflicts, detached unique work, unknown working state | Preserve work and identify integration problems | Existing Git-health engine and this brief; no deletion/merge action from a summary |
| Exact-head/base review age, required-check coverage, flaky/queued CI and merge wait | Prevent stale approval and misleading green subsets | Existing forge/integration-quality code; required set plus queued/start/finish clocks and actual results |
| Worktree disk, pins, locks, ownership and retirement eligibility | Make hygiene useful without destroying unique work | Runtime workspace registry/retirement; slow-cadence disk scans; complete custody before cleanup |
| Time to first progress, no-progress/tool wait and unknown process owner | Find stalls while respecting legitimate long tools | Provider/session observation; diagnostics do not grant process-kill authority |
| Requested-vs-observed model, fallback, quota age and usage coverage | Expose configuration/capacity mismatches | Runtime providers/economics; preserve opening balances and unknown usage prefixes |
| Coordination-token share, accepted outcomes/total cost and context fidelity | Determine whether compression helps useful work | Existing context/economics/trials; include failed/retried/routing/summarizing work and unmatched liabilities |
| Installed/running revision, asset hash, config/policy generation and backup age | Detect mixed versions and unsafe rollback assumptions | Release/daemon diagnostics; identifiers and hashes, no secret bytes |

## Next performance cuts

1. Share cached validated initial/detail/brief bytes for an exact producer cut.
   The view hub already fingerprints state on its two-second refresh. Cache
   bounded immutable variants and precompressed bytes there, preserve source
   errors/clocks, and authorize every response independently. Bound cache bytes
   and waiters; do not key an unbounded cache by arbitrary request parameters.
2. Profile parse/clone/hash/projection/encode independently. `legacy.rs` reads
   per request; `views.rs` has its own source projection. This brief avoids a
   second parse inside its request, but does not yet unify those consumers.
3. Finish P03 indexed bounded task paging and Store telemetry. Test query plans,
   revision/cursor semantics and declared 100k-task loads before latency claims.
4. Extend #167's integrated worker/canvas/layout work without duplicating it. Keep one layout
   for canvas, minimap and fit bounds. Full-history indexing and authenticated
   integration overlays are separately scoped; avoid a competing graph library.
5. Virtualize long tables, window history and lazily mount charts/terminals.
   Preserve selections, keyboard navigation and visible omitted-data notices.
6. Benchmark fixed sizes and 1/10/50 subscribers, 10/100/2,000 projects and
   declared commit counts. Record p50/p95/p99, errors, CPU/PSS, event backlog,
   wire bytes and browser interactions with raw evidence. Smaller responses
   alone do not prove a faster whole system.

## Layout direction

Use **fleet health → project → evidence → action** as the navigation sequence.
Health shows decisions/unknowns; detail retains dense trends; Git Flow explains
ancestry; native task/incident pages retain authenticated scopes. Avoid mounting
every plot, terminal and command form on the first page.

Keep the main branch as the visual backbone in the existing graph cut. Label
ancestry, forge integration, required checks and actual deployment as distinct
evidence. Use a persistent large-screen inspector and a focusable small-screen
drawer. Status needs text/icons as well as amber/cyan. Keep source clocks,
unknowns and lower bounds next to each metric.

Next UI work: project deep links across health/detail/Git; local display
preferences; a data-quality/coverage panel; keyboard-accessible durable alert
acknowledgement; and a source-age timeline explaining outages/retained data.
Acknowledge alerts without changing measurements or granting action authority.

## Validation and handoff

New tests cover byte budgets, omissions/full totals, exact counters, stale/
future/unknown clocks, invalid reports, conflicts plus unknown ancestry, prose
exclusion, scope limits, private credentials/no-follow/reuse, HTTP access parity,
frontend validation and a Rust/React desktop/mobile browser journey for a supported sandboxed runner.
Existing workspace/unit/browser CI discovers them automatically. Run the full
`pnpm check` matrix before pushing and preserve original failures. Record actual
exits, tool versions, counts and evidence hashes in the delivery receipt.
Hosted checks, exact-head approval, merge and live rollout remain separate.
