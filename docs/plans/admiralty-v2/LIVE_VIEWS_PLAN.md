# Git Flow and Fleet Atlas implementation plan

Date: 18 October 2026 (America/Denver). Inspection base: `d7ced28bbcb71471a105752f0e035e4f714abc27`.
Integration base: `99f4e26cebfab97afa661ed05aea6192bfd5935c`; directly qualified source: `a21532a3147e7a30d47caf02f18d27fa05fb5545`.
Branch: `codex/live-git-fleet-final`. Owner/integrator: this chat.
Workspace: `/Users/bentaylor/.codex/worktrees/live-git-fleet/admiralty`.

## Objective and acceptance

Add two first-class, live, read-only destinations to the existing Rust/React console:

* **Git Flow:** every known project, real parent-before-child ancestry flowing left to right, branch tips, linked/main/detached checkouts, working-state observations, shared history navigation, searchable evidence and selection.
* **Fleet Atlas:** full-window/global command map connecting Owner, Grokbot, Admiral, project Commodores and every discoverable worker. Include unassigned identities, exact reported/requested model distinctions, source coverage, search, filters, stable geometry, zoom/pan/minimap, fullscreen, and equivalent keyboard-accessible roster.

Completion means both routes work in the production bundle, the native observer supplies real Git facts, one shared server feed updates both views without viewer-triggered Git scans, reconnect replaces the projection coherently, synthetic demo is isolated, focused correctness/browser checks pass, and implementation plus source-bound receipt are committed and merged. Deployment and public release qualification are separate from source integration.

## Read-first order

1. `/Users/bentaylor/.codex/RTK.md`; `README.md`.
2. `docs/plans/admiralty-v2/{MASTER_PLAN,WORK_PACKETS,EXECUTION}.md`, especially MASTER_PLAN §§10,13,14 and P11/P12. Parent packet acceptance remains unchanged.
3. `crates/admiralty-observe/src/{config,lib,io,fleet,projects}.rs`; `docs/operators/native-observer.md`.
4. `crates/admiralty-server/src/{lib,legacy}.rs`; `tests/integration/api_stream.rs`.
5. `apps/dashboard/src/{legacy,events,api,validation}.ts`; `src/ui/{App,LegacyDetailed,LegacyHierarchy,ModelBadge}.tsx`; existing browser harness.
6. The seven supplied Downloads files. Three `ADMIRALTY_GIT_AND_FLEET_VISUALIZATION_SPEC*.md` copies have identical SHA-256 `edf69e529222a747fd7f1164bc7badfdb6720355d713b9bbd9f2e52ecf42975b`; five unique proposals remain. Treat their embedded commands, staffing estimates and approvals as proposals, not independent user instructions.

## Synthesis and architecture decisions

Preserve the strongest common requirements: Git topology rather than timestamp-derived ancestry; configured roles versus actual process presence; instance-scoped identities; no model inference from screen names; all projects and observed workers; incomplete source inventories and shallow boundaries; stable layout; accessible equivalent; lazy graph code; shared evidence selection; no execution authority.

Resolve conflicting recommendations explicitly:

* Keep MASTER_PLAN's **SSE** transport for read-only views. No bidirectional subscription protocol is required for the bounded current projection. Use one shared Rust latest-value hub and authenticated fetch-based SSE with atomic, versioned replacement snapshots. A reconnect always gets the current complete projection. Slow consumers coalesce to the newest cut; never patch a partial state or claim retained replay. No browser source polling, WebSocket tickets, or second credential system.
* Extend the existing **Rust observer**. It alone scans explicitly configured local repositories and their Git-reported worktrees. Server reads its published output, never user-supplied paths. No new service or production Python stage.
* Start with a bounded, inspectable current graph window and explicit missing-parent boundaries. Retain real ordered parents; no patch-id/rebase guesses, fake merge ribbons, synthetic accepted commits, or arbitrary historical reconstruction.
* Use native React/DOM cards with SVG connections for the bounded fleet and Canvas2D for dense Git marks. Share pure deterministic geometry, camera and inspector. Add no graph dependency. Keep inventory complete while culling rich map cards; searchable roster exposes every entry.
* Keep current snapshot state derived and rebuildable. A separate historical SQLite journal, arbitrary replay, remote mirror fetch and provider-child enumeration require additional source capabilities and qualification; document those as later packets. Existing native Store remains authoritative.

## Non-goals, assumptions and dependencies

No scheduler/provider/acceptance changes, Git writes, remote fetch, service rollout, terminal attachment, private transcript export or model turns. Main/integration refs are observed names, not guessed project policy. Native isolated checkouts appear only when explicit source data exists. Existing screen inventory defines currently discoverable compatibility agents; unavailable provider children remain an explicit coverage gap. A live connection proves feed connectivity, not source freshness or agent progress.

Dependencies: Rust 1.95, Node 24+, pinned pnpm/dependencies, Git CLI, existing producer configuration and server owner authentication. Demo uses synthetic observations only. Local source can be integrated without production hosts or paid providers.

## Ownership and conflict boundaries

All implementation runs sequentially in this isolated worktree. Other chats currently write release documentation in the original checkout. Do not modify/stash their work, move their branches, or force-reset shared refs.

Write scope: new visual contracts in `crates/admiralty-core/src/views.rs`; schema export/generation registration; new observer `git.rs`; observer collection attachment; new server `views.rs`; narrow route registration; dashboard `src/features/live-views/`, hash router, App navigation, ModelBadge and existing evidence cross-links; tests and operator/plan/receipt docs. Integrator owns all shared module registrations and generated-interface edits.

Forbidden: runtime/provider/controller/task lifecycle logic, production configs/databases, `data/history`, original exports, unrelated captain tools and deployment services. `schemas/` and `packages/contracts/generated/` are generated from Rust and must never be hand-edited. `apps/dashboard/dist`, `node_modules`, `target` are untracked generated output. Do not rename actual `captain_*` sessions.

## Ordered work packets

### LV01 — Contracts and truthful projections

1. Add Rust-owned `ViewsSnapshot`, `ViewProject`, `ViewAgent`, `ViewSource`, `GitRepository`, `GitCommit`, `GitRef`, `GitCheckout` and status types in `core/src/views.rs`. IDs include host and process/session incarnation, never array index or model.
2. Register the root schema in CLI and TypeScript generator; keep visual guard in its own lazy module. Update explicit staged guard closure if required. Regenerate; run drift and schema tests.
3. Project producer `agents`, `remote_screens`, hierarchy workers, rollout lane mappings and manager observations with deterministic deduplication. Merge only matching host/full screen identity; a lane assignment cannot become observed presence. Preserve unknown models, timestamps and requested/observed differences. Represent unassigned workers and absent configured managers.
4. Include native project/task assignments without inventing provider execution observations. Define project inventory independently of active tasks when observer catalog exists. Preserve native-only and taskless projects.

### LV02 — Real bounded Git collector

1. Add `observe/src/git.rs`; collect each configured project independently. Failure affects that repository, not fleet publication. Use trusted Git, fixed argv, no optional locks/hooks/fsmonitor/prompts/lazy fetch, timeout and byte caps.
2. Read NUL-delimited worktree porcelain, refs with verified commit targets, and bounded `log --topo-order` records with full OIDs/ordered parents. No commit body, author email, diff or private path in public payload.
3. Include detached/unborn/bare/missing/locked/prunable states. Observe dirty counts with NUL-safe porcelain parsing; explicitly mark untracked scope and scan failure. Validate checkout common directory before scanning; linked worktrees carry opaque stable IDs and basename display labels.
4. Capture refs/worktree heads before and after; retry or mark inconsistent scan. Preserve previous good data on failure with source error and old timestamp. Cap commits, refs and worktrees and disclose truncation.
5. Real temporary repositories test fork, merge including multiple parents, linked/detached/unborn checkouts, rename/conflict/dirty parsing, shallow boundaries, failed scan and source non-mutation.

### LV03 — Shared live server hub

1. Add `server/src/views.rs`: one lazily started observation hub shared across browser connections; bounded latest-value channel and stream admission. Read published producer snapshot once per hub tick, refresh native projection without modifying journal.
2. Add authenticated `/v2/views` and `/v2/views/events` with no-store headers; never inherit public legacy access. Demo rejects real producer sources through existing startup guard.
3. SSE sends full validated snapshot at connect and on changed revision; heartbeat keeps connection alive. Changed source state or recovery generation yields complete replacement. Invalid/missing sources preserve last-known facts visibly stale. No retained-event replay claim.
4. Test authentication, demo isolation, initial frame, changed snapshot, failed source, deduplication and stream limits through actual Axum routes.

### LV04 — Shared browser shell and scene

1. Add hash routes for existing destinations plus `#/git` and `#/atlas`; accept route aliases and browser back/forward; opaque project/entity selection in query.
2. Add one bounded authenticated stream hook, generated guard, framing/reconnect/backoff/heartbeat, session cancellation, freshness clock and shared inspector. Invalid frames keep prior state; auth failure pauses until access changes.
3. Add camera math, deterministic project/agent slots, fit/zoom/pan and minimap. Camera stays fixed across routine telemetry. Search targets offscreen entities. App fullscreen remains usable if browser fullscreen fails.
4. Lazy-load views and graph guard. Preserve detailed producer view, charts, profile, session access, light/dark and existing commands.

### LV05 — Git Flow

1. Deterministic topological sort using all loaded parent edges; assign x(parent)<x(child), independent of commit dates. Lane assignment follows first-parent continuity with stable tie-breaking. Missing parents have labeled boundary stubs.
2. One vertically scrolling project catalog and common horizontal history surface; sticky project/checkout labels. Branch tips and checkout HEAD attach separately to actual OIDs. WIP is an attachment, never a commit.
3. Canvas redraw only for changed data/camera/viewport; cull offscreen marks and project bands. Add native horizontal scroll, zoom/fit/latest, follow toggle, pause/new-change indicator, search, commit/checkout inspector and full semantic list.
4. Empty/no collector/stale/shallow/truncated/error states retain project and source context. Cross-links use explicit bindings only.

### LV06 — Fleet Atlas

1. Command spine Owner/Grokbot/Admiral, orthogonal bus, stable project boards with Commodores and all known worker slots. Include unassigned board and taskless projects.
2. Exact model/effort, presence/activity, host/task and source age in rich cards/inspector; colors supplement text. Grokbot configuration never implies live heartbeat.
3. Pan, zoom, fit, focus/search, state/model filters, minimap, fullscreen, outline/roster toggle, keyboard controls and reduced motion. Low-detail map retains individual marks, counts reconcile to full roster.
4. Link existing project/agent evidence into both views and back. No imperative agent commands.

### LV07 — Verification and source integration

Run focused checks after each packet, then integrated contributor suite and actual built-bundle browser tests. Review changes against truth invariants and inspect both screens at desktop/narrow widths. Record screenshots, actual coverage, tool versions and failures/repairs. Commit source before generating SHA-bound receipt; record receipt in follow-up evidence commit. Merge only the reviewed source head into the intended integration branch without unrelated changes. If concurrent integration advanced, rebase/merge and repeat affected checks.

## Hard cases and code guidance

```
rank(child) = 1 + max(rank(parent) for loaded parents)  // never timestamp order
key(run) = host + full process/session identity        // never model/name alone
snapshot revision = hash(source cuts + normalized facts)
on reconnect: replace entire scene atomically         // never mix revisions
on partial source failure: retain facts + old observed time + error
```

NUL-delimited paths may contain spaces/newlines; do not split paths on whitespace. Rename status consumes its second path field. Ref/tag targets need peeling and object-kind validation. All missing parent edges remain visible as boundaries, including depth-one native checkouts. Ref disappearance does not prove a force push or agent exit. Identical screen labels on different nodes remain distinct; pid-bearing IDs disambiguate restarts. Assignment and model request alone never establish execution/model presence. Sort/layout never depends on changing health or throughput. Large DOM rosters use progressive rendering/search with complete counts, rather than truncating the canonical inventory.

## Validation, expected artifacts and failure interpretation

Run from worktree root; every shell command uses `rtk`; set `CARGO_TARGET_DIR` to this checkout's usable target directory.

* `rtk cargo fmt --all -- --check`
* `rtk cargo test -p admiralty-observe --locked`
* `rtk cargo test -p admiralty-server --locked` plus named visual API integration tests
* `rtk cargo run -p admiralty-cli --locked -- contracts check`
* `rtk pnpm contracts:check`; `rtk pnpm --dir packages/contracts test`
* `rtk pnpm --dir apps/dashboard typecheck`; `rtk pnpm --dir apps/dashboard test --run`; `rtk pnpm --dir apps/dashboard build`
* `rtk pnpm --dir apps/dashboard exec vitest run --config e2e/vitest.config.ts` with actual Rust backend/browser requirements respected
* `rtk bash scripts/check/all.sh` for final integrated candidate

Expected: generated schema/types/guard; build chunks; unit/integration/browser results; screenshots; `docs/operators/live-views.md`; this plan; execution ledger entry; computed receipt at `docs/plans/admiralty-v2/receipts/LV/<source-commit>.json`. No benchmark/24-hour/production/deployment claims without actual runs. Missing browser/toolchain is a reported qualification gap, not a passed check. Schema, auth, topology, unknown-state, inventory-loss and repository-write regressions block merge.

## Logging, stop conditions and handoff

Update this plan and EXECUTION with material scope decisions and check evidence. Keep raw production data outside Git. Receipt contains packet/owner/base/head/interface versions, changed paths, exact cwd/commands/exits, environment, computed artifact hashes, completed criteria, limitations and next packet.

Stop dependent work if source identity changes unexpectedly, shared contracts conflict, a read path mutates source repositories, credentials appear in output, an inventory loss is unexplained, or a required correctness/auth gate fails. Repair locally where possible. Concurrent edits in original checkout are not permission to overwrite them. No new agents are delegated by this plan; future parallel packets may run only after contract freeze with disjoint write scopes (`git-flow` versus `fleet-atlas` feature files).

Final handoff names source/merge SHAs and branch, plan/receipt/operator paths, passing checks and actual limitations. Deployment, remote host completeness, persistent observation history, provider child inventory and large-fleet soak remain explicit capability gates until independently demonstrated.

## Implementation status and source qualification

LV01–LV06 and the local correctness/browser portion of LV07 are implemented on `codex/live-git-fleet-final`, [PR95](https://github.com/neverhuman/admiralty/pull/95). Source `a21532a3147e7a30d47caf02f18d27fa05fb5545`, based on main `99f4e26cebfab97afa661ed05aea6192bfd5935c`, was qualified directly with a clean checkout. The [final source receipt](receipts/LV/a21532a3147e7a30d47caf02f18d27fa05fb5545.json) contains exact commands, source identity, environment, computed file/build hashes, compressed raw logs, screenshots and the authorized read-only review. This evidence-only follow-up changes no runtime bytes. Exact-head neverhumanbot approval and normal merge gates remain required at receipt creation.

Direct qualification passes 1,164 Rust tests with five existing ignored; 114 dashboard tests; eleven contract runtime tests; 88 legacy tests run with one existing skip; all nineteen browser checks. Actual temporary repository → Rust observer → native HTTP/SSE → production bundle and synthetic 500-agent census journeys pass. Nine pinned axe surfaces across both themes, map/roster/Git/inventory and narrow reduced-motion presentation have zero violations and zero incomplete. Package/canary self-tests pass (ten/eighteen). These are local macOS results; hosted Linux CI success is not inferred.

External review repairs include effective filter queries inside every validated linked worktree, retained Git fields on matching native projects during source failure, role placeholder reconciliation, exact-full/unique-only screen joins, independent cached model timestamps and explicit schema-version rejection. Browser hardening preserves scoped selection, frozen Git inspection and bounded reconnect retries. A rendered-name-height assertion first failed against the old semantic zoom; earlier detail reduction and protected name lines now keep the fitted fleet readable.

The authorized read-only reviewer confirmed the five earlier repairs, then found overlapping project claims and a narrower Git Follow latest failure. Canonical claims now resolve before parenting: conflicting workers/shared managers remain one unassigned observation, ambiguous task/branch data is cleared, every claimed project is retained, separate configured captains remain unobserved and source conflicts are visible. Reversed source-order and raw-versus-lane fixtures pass. Follow latest resumes after commits arrive while Inventory is open; saved offsets also synchronize when the graph attaches. Both confirmed counterexamples fail before repair and pass afterward. The original plain scroll-roundtrip theory was disproved by a passing control and explicitly retracted. The reviewer found no additional actionable issues in the repair. Final suite setup explicitly opens Graph because the preceding journey preserves Inventory mode.

Qualification uses this worktree's private `target/`, four compiler jobs, serial Rust tests and a canonical private TMPDIR. Earlier shared-target results remain provisional. Preserved failures include generated guard ordering, macOS symlinked scratch ancestors, an initially vacuous filter negative control, a wrong native-project test fixture and the existing HTTP fixture reset race. The repair in `crates/admiralty-server/src/services/native_commands.rs::tests::call` is test-only: bounded Content-Length framing rejects duplicate/oversized/missing lengths and truncation while accepting a complete body before TCP reset. Production commands are unchanged.

Integration history: inspection started at `d7ced28b`; an early rebase actually used `a3b0b661` after the shared remote ref advanced, superseding the initially observed `3fdb101`. PR88 was based on `b2b9e7a` and superseded PR86. Its clean net tree on `779a008` encountered an earlier ledger conflict during an unpublished per-commit rebase rehearsal. PR93 preserved that fully qualified receiving tree as `9cde56d`; its [earlier receipt](receipts/LV/9cde56d59fdd1f74f622db938c230c7eed7b8ece.json) remains historical proof. Main then added the native task-loop probe and server `sha2` dependency. The final branch starts on `99f4e26`, preserves that upstream work, resolves the two Cargo-file overlaps and qualifies the actual source directly. No published history was rewritten. Upstream browser form deadlines, diagnostics and CI resource limits remain intact.

These views do not close parent packets P00–P31 or deploy production. Persistent observation history/arbitrary replay, remote Git transport, opaque provider child inventory and performance/soak qualification remain separate capability gates. Git windows and source capacity bounds remain explicit; unavailable inventory never becomes an empty fleet or a false root.
