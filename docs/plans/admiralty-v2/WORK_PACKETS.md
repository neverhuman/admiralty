# Admiralty implementation work packets

Read [MASTER_PLAN.md](MASTER_PLAN.md) first. Root: `/Users/bentaylor/Documents/ChatGPT/admiralty`. Baseline: `1e8ed17eff27ed3ab80dc6860fc8eda301406aee`. These are full acceptance contracts; current partial implementation and qualification state is recorded in EXECUTION.md.

## Execution contract

Before claiming a packet, read its dependencies' integrated receipts and current local instructions, obtain the actual base SHA and verify a clean owned worktree. Receive only this packet, relevant plan sections and pinned interfaces. Do not load all source proposals into a worker's context.

Each packet below supplies ordered steps, write scope, read-first references and acceptance. Unless named otherwise, workers must not edit root manifests, lockfiles, shared `mod.rs`, migrations, protocol/generated contracts or CI. Request those edits from the integration owner with exact required content. Target files may be created only within the packet's scope. Do not edit production runtime or invoke live providers/forge effects to make a test pass.

P01 registers root integration tests in `admiralty-testkit` using named `[[test]]` entries; integration owns registration as packets add them. Test names below are target contracts, not currently available tests. Commands are run from repository root with `rtk`. A packet's standard validation is formatting, its named unit/integration tests and affected compile/type checks. Full workspace/UI/qualification checks run on integrated release candidates.

Every packet outputs an implementation commit/candidate and `receipts/<packet>/<commit>.json` with criteria, exact commands/exits, environment, computed digests, limitations and next action. A packet taking more than two focused implementation days should be split by its existing step boundaries without relaxing acceptance. Record the split in EXECUTION; do not expand its write scope informally.

## Foundation and early coordination

### P00 — Pin sources and establish regression fixtures

**Depends on:** none. **Owner:** evidence lane. **Release:** R0.

**Read first:** `README.md`; `data/history/README.md`; `dashboard/{collector,extras,token_yield,globalres}.py`; `tools/{sc_audit,export_history}.py`; `tools/bin/{classify_screen.py,kick}`; `captains/{COMMON,MODEL_ROUTING_PLAYBOOK,CPU_SLICES}.md`; SOURCE_REVIEW.

**Write scope:** `tests/fixtures/legacy/`, `docs/architecture/legacy-metric-dictionary.md`, `docs/operators/legacy-inventory.md`. Do not change original exports.

1. Write source manifest with base SHA, file hashes, row counts, mirror/duplicate relationships and source time bounds. Separate the old 449-row baseline from current 503-row export.
2. Create minimal sanitized fixtures for lifetime-window leakage, mixed-model attribution, unknown-price zero fill, coverage accumulator, stale capture, quoted approvals, missing scope, docs-only implementation, affected-test omission and wrong-checkout PASS.
3. Label reconstructed historical incidents synthetic. Add a read-only deployment inventory worksheet for paths, symlinks, rules, actual versions, session/process identities and cron/input owners; leave unknown fields explicit.

**Validate:** P01's eventual `rtk cargo test -p admiralty-testkit --test legacy_import`; before P01, parse fixture JSON/NDJSON and reconcile counts/hashes with a read-only script. **Pass:** idempotent expected inputs, no private transcripts/keys, no invented exact attempts/acceptances. **Stop:** mismatched source identity or secret scan finding.

### P01 — Bootstrap workspace, fake processes and CI

**Depends on:** none. **Owner:** integration. **Release:** R0.

**Read first:** MASTER_PLAN §§3–4, 13–14; `.gitignore`; existing README.

**Write scope:** root `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`; six crate manifests and minimal entrypoints; `crates/admiralty-testkit/src/`; `scripts/{dev,check}/`; `.github/workflows/ci.yml`; targeted `.gitignore` additions. No legacy refactor.

1. Pin stable Rust/MSRV, workspace dependencies and lint policy; select compatible frontend Node/package-manager versions from tested tooling, not from old plans.
2. Build deterministic fake provider modes: success, quiet tool, malformed/split stream, cumulative usage/reset, crash, blocked input and noisy output. Add fake clock, fault triggers and forge simulator without network credentials.
3. Register named integration harnesses as packets land. Establish formatting, Clippy, tests, schema drift and frontend checks; CI paid-provider jobs stay disabled by default.
4. Create binary entrypoints `admiraltyd`, `admiralty-node`, `admiralty-session` and `admiralty`; running help must not enroll or mutate a project.

**Validate:** `rtk cargo test --workspace --locked`; `rtk cargo run -p admiralty-cli -- --help`; deterministic fake-provider replay. **Pass:** clean clone builds, synthetic results reproducible, all generated/runtime zones explicitly ignored. **Artifact:** pinned toolchain and fixture seed.

### P02 — Freeze domain, coordination and configuration contracts

**Depends on:** P00, P01. **Owner:** integration/domain. **Release:** R0.

**Read first:** MASTER_PLAN §§5–6, 8; current policy source files; P00 metric dictionary.

**Write scope:** `crates/admiralty-core/src/{ids,contract,state,events,policy,reducer}.rs`; `protocol/`; schema generator; `schemas/`; `packages/contracts/`; `configs/{defaults,profiles,examples}/`; `docs/protocol/`.

1. Implement `TaskContract`, `PlanRevision`, state dimensions, `Cursor`, `AuthorityRef`, `CoordinationEnvelope`, `InterfaceContract`, `VerificationReceipt`, usage native-unit types and typed errors.
2. Implement pure `reduce(state,event,time)` with explicit recorded randomness; define permitted transitions and immutable revision rules. Terminal/provider output cannot directly emit trusted acceptance.
3. Generate one public JSON Schema/TS/runtime-validation pipeline. Encode large counters as precision-safe values; required unknown semantics fail negotiation.
4. Add config precedence and `effective_policy()` with provenance. Node/project/provider/role identities are data; private restrictions cannot be overridden by public defaults.

**Validate:** `rtk cargo test -p admiralty-core contracts`; `rtk cargo run -p admiralty-cli -- contracts check`; schema round-trip/unknown-required-field/large-integer fixtures. **Pass:** deterministic reducer hash, no production host constants, consumers agree on one versioned contract.

### P03 — Authoritative writer, replay, outbox and read projections

**Depends on:** P02. **Owner:** store/integration. **Release:** R0.

**Read first:** MASTER_PLAN §6; official SQLite WAL/backup guidance linked there; P02 event contracts.

**Write scope:** `crates/admiralty-store/src/{writer,inbox,outbox,projections,backup}.rs`; all numbered migrations, owned centrally; `tests/integration/{store_replay,store_crash}.rs`.

1. Create ordered migration families in MASTER_PLAN. Implement bounded writer requests, short transactions, local WAL/FULL/foreign-key setup and linked-engine/source-ID diagnostics.
2. Implement `ingest_observation`, `commit_domain_event`, `reserve_and_enqueue`, contiguous producer ACKs and content-collision quarantine. Use no silent replacement of immutable source facts.
3. Implement `rebuild_projections` with no effect execution, event/recovery cursors and indexed bounded queries. Expose writer depth, transaction latency and WAL reader/checkpoint health.
4. Add online backup primitive and artifact-closure metadata interface; full restore authority is P25. Never copy the live main DB file alone as backup.

**Validate:** `rtk cargo test -p admiralty-testkit --test store_replay`; `... --test store_crash`. **Pass:** crash-point matrix loses no acknowledged control/usage fact, identical replay preserves projection hash, changed-payload ID reuse is rejected. **Stop:** linked SQLite lacks qualified WAL fix.

### P04 — Durable artifacts, pins and asset registration

**Depends on:** P03. **Owner:** store/artifacts. **Release:** R0.

**Read first:** MASTER_PLAN §§6.5, 9.2; P03 transaction API.

**Write scope:** `crates/admiralty-store/src/artifacts.rs`; `tests/integration/artifact_custody.rs`; `docs/architecture/artifact-lifecycle.md`. Request schema changes from P03 owner.

1. Implement `publish_artifact`, verified retrieval, digest/size identity and explicit availability. Sync temporary object and rename/parent boundary before authoritative availability commit.
2. Implement `register_asset`, `acquire_pin`, `release_pin`, `claim_retirement` and custody metadata; claim and new-reference acquisition must interlock transactionally.
3. Add orphan/missing-object reconciliation, access-scoped range reads, retention classes and emergency storage reserve accounting. No destructive executor yet.

**Validate:** `rtk cargo test -p admiralty-testkit --test artifact_custody`. **Pass:** crash between each publication boundary and concurrent pin/retirement cannot produce missing acknowledged evidence or delete a live reference.

### P05 — Durable mailboxes, subscriptions, questions and decisions

**Depends on:** P03, P04. **Owner:** coordination. **Release:** R0.

**Read first:** MASTER_PLAN §§5.1–5.6; P02 envelopes; P03 writer/inbox/outbox.

**Write scope:** `crates/admiralty-core/src/coordination/{message,mailbox,topics,decisions}.rs`; `crates/admiralty-server/src/services/coordination.rs`; `tests/integration/coordination_delivery.rs`.

1. Implement `route_message`, `subscribe`, `deliver_batch`, `ack_delivery` and immutable causation/correlation. Validate task/project authority before topic resolution.
2. Implement question/decision lifecycles with expected revisions, deadlines, supersession and scoped escalation. Deterministic questions read current facts without inference.
3. Implement priority reserves, bounded queues, progress coalescing, duplicate-question detection and attention-budget hooks. Preserve underlying events and important intervening transitions.
4. Add recipient replay, explicit expiry and unknown-delivery state. Keep transport/provider/application ACKs distinct. No terminal injection in R0.

**Validate:** `rtk cargo test -p admiralty-testkit --test coordination_delivery`. **Pass:** loss/reorder/duplicate/storm/restart scenarios converge or visibly require resnapshot; no cross-project leakage, silent decision or broadcast storm.

### P06 — Context compiler, structured results and context governor

**Depends on:** P02, P04, P05. **Owner:** context. **Release:** R0.

**Read first:** MASTER_PLAN §§5.7–5.11; P00 failures; P02 contract generator.

**Write scope:** `crates/admiralty-core/src/coordination/context.rs`; new `coordination/{render,tool_results,tried,governor}.rs`; `tests/fixtures/context/`; `tests/integration/context_fidelity.rs`.

1. Implement the five capsule compilers with must-carry tags, role views, bounded retrieval references, immutable prompt-component hashes and clear unknowns.
2. Implement recipient epochs/baseline digests, collision-checked aliases and complete protected-capsule fallback for opaque clients. Persist exactly rendered text and source facts.
3. Add structured compiler/test-result summaries, repeated-diagnostic grouping and task failed-approach memory. Keep raw artifacts retrievable by range.
4. Implement token estimates/exact counts where available, harness-floor provenance, role/purpose accounting and rotation-cost proposal. Do not force a paid handoff or assume provider hidden context is editable.

**Validate:** `rtk cargo test -p admiralty-testkit --test context_fidelity`. **Pass:** render/parse fact round-trip and mutation suite; missing negation/unit/authority/revision fails; compaction/lost-baseline requests fresh capsule; no savings claim from bytes alone.

## Managed local work

### P07 — Independent session host and durable launch/input journal

**Depends on:** P01, P02, P03, P04. **Owner:** runtime/session. **Release:** R1.

**Read first:** MASTER_PLAN §7.1–7.2; P03 outbox; P04 spool custody interface; systemd guidance linked in plan.

**Write scope:** `crates/admiralty-runtime/src/session/`; `src/bin/admiralty-session.rs`; `tests/integration/{session_continuity,input_uncertainty}.rs`.

1. Implement `spawn_or_reconcile`, OS process identity, process-group lifetime, pipe/PTY owner and independent local control socket.
2. Implement raw-offset spool and semantic journal with bounded queues/rings; prioritize control/usage events and explicit storage-pressure behavior.
3. Implement input epochs, human preemption, turn gating and journaled delivery. A possible write followed by crash is unknown, not blindly replayed.
4. Exercise coordinator/node restart with a live fake child; no descriptor loss, duplicated input or implicit service-wide termination.

**Validate:** `rtk cargo test -p admiralty-testkit --test session_continuity`; `... --test input_uncertainty`. **Pass:** same physical child and continuous output offsets survive supported control-service restarts; unreconciled launch cannot spawn duplicate ordinary work.

### P08 — First qualified real provider and liveness detector

**Depends on:** P06, P07, P09. **Owner:** providers. **Release:** R1.

**Read first:** MASTER_PLAN §7.3–7.4; current installed CLI help/schema under a read-only probe; P01 fake provider.

**Write scope:** `crates/admiralty-runtime/src/providers/{mod,fake,codex}.rs`; new `providers/{capabilities,detectors}.rs`; `configs/profiles/providers/`; `tests/fixtures/providers/codex/`; `tests/integration/provider_codex.rs`.

1. Define `ProviderAdapter` capability-qualified launch/parse/resume/input/usage contract. Select the first installed structured Codex transport after probe; do not copy stale flags from proposals.
2. Parse fragmented/malformed/unknown events with bounds; bind actual model/effort/configuration and identify nested usage or unsupported fields honestly.
3. Implement seven liveness clocks and versioned detection packs with corroboration, hysteresis and negative golden fixtures. Unknown/silent tool states never grant input/kill authority.
4. Record exact CLI digest/schema and context-floor evidence. Qualify one bounded real task only under the configured task budget; parser/fault tests remain offline.

**Validate:** `rtk cargo test -p admiralty-testkit --test provider_codex`. **Pass:** supported tuple declared precisely; quiet compiler, spinner, quoted permission and provider wait classify conservatively; no silent model fallback. **Artifact:** capability matrix and real-task receipt.

### P09 — Private runners, managed workspaces and resource permits

**Depends on:** P04, P07. **Owner:** workspace/runtime. **Release:** R1.

**Read first:** MASTER_PLAN §7.5; imported private custody/resource rules; Git worktree docs.

**Write scope:** `crates/admiralty-runtime/src/workspace/{registry,git,runner,resources}.rs`; `packaging/containers/`; `tests/integration/runner_isolation.rs`.

1. Implement registered trusted-local worktree mode and private-clone rootless OCI mode; resolve actual Git dirs and reject shared writable metadata in isolated mode.
2. Implement scoped mounts/credentials/network, managed-root handles, process/cgroup identity and CPU/memory/PID/disk/GPU permits. Keep inference slots separate from heavy compute.
3. Seed verified warm mirrors/toolchain images with lifetime pins; isolate mutable build output and key compiler caches by complete environment/trust domain.
4. Expose unsupported platform/enforcement states rather than pretending every host has Linux cgroup/OCI guarantees.

**Validate:** `rtk cargo test -p admiralty-testkit --test runner_isolation`. **Pass:** sibling refs/hooks/secrets/host mounts cannot be changed under isolated mode; actual OOM/throttling and private CPU exclusions observed. **Stop:** runner cannot enforce its advertised boundary.

### P10 — Usage, valuation, reservations and cohorts

**Depends on:** P03, P08. **Owner:** economics. **Release:** R1.

**Read first:** MASTER_PLAN §9.1; P00 accounting fixtures; provider native usage semantics.

**Write scope:** `crates/admiralty-core/src/economics/`; `crates/admiralty-server/src/services/metrics.rs`; `tests/integration/economic_conservation.rs`; `docs/architecture/metrics.md`.

1. Implement native usage segments, opening balances, reset/overlap selection, revision lineage and exact/estimated/unknown coverage.
2. Implement checked valuation, versioned prices, separate currencies/quota, role/purpose allocation and unallocated account activity.
3. Implement `reserve_budget`, `settle_usage`, retained in-flight liabilities and permitted overshoot. Expiry/cancel requests cannot release unknown exposure.
4. Implement fixed assignment-cohort queries including failed/cancelled/reopened roots, manager costs and maturity. Preserve legacy heuristic metrics separately.

**Validate:** `rtk cargo test -p admiralty-testkit --test economic_conservation`. **Pass:** allocation sums and reservation conservation across duplicate/reset/model-change/late-correction/zero-success/tiny-price fixtures; no unknown becomes zero.

### P11 — Authenticated API, snapshots, SSE and CLI scaffold

**Depends on:** P02, P03, P04, P05. **Owner:** API/integration. **Release:** R0, terminal wiring completed in R1.

**Read first:** MASTER_PLAN §§5.8, 10.3–10.4; P02 schemas; current `dashboard/serve.py` allowlist.

**Write scope:** `crates/admiralty-server/src/api/`; server `main.rs`; `crates/admiralty-cli/src/{main,doctor,init,export}.rs`; `tests/integration/api_stream.rs`.

1. Implement scoped read/query and command envelopes, local browser authentication, origin/CSRF controls, command idempotency and expected-revision conflicts.
2. Implement consistent snapshot/cursor plus SSE replay/resnapshot, bounded readers and authorization/recovery-generation changes. Large counters stay exact.
3. Serve versioned static assets with correct SPA fallback/API errors; add CLI JSON errors/exit codes and durable job IDs. Static GET failures do not become fake success pages.
4. After P07, wire authorized offset-based terminal WS/input lease; after P17, register narrow agent tools. Integration alone owns this wiring.

**Validate:** `rtk cargo test -p admiralty-testkit --test api_stream`. **Pass:** no gap between snapshot and subscription; stale cursor/access revocation resyncs; slow viewer cannot block capture or DB checkpoints; auth failures deny artifacts.

### P12 — Fast React shell and read-only dashboard parity

**Depends on:** P02, P11. **Owner:** frontend/overview. **Release:** R0.

**Read first:** MASTER_PLAN §10; `dashboard/index.html`; P00 metric dictionary; supplied dashboard images.

**Write scope:** `apps/dashboard/` bootstrap, app shell, `src/api/`, `src/components/`, `src/styles/`, `src/features/overview/`; frontend package/lock changes owned by integration. Do not claim other feature directories.

1. Build strict TS/Vite/React shell, generated contract client/runtime validation and cursor-aware store. Use fixtures before live shadow data.
2. Recreate project cards/stage bars, exceptions, resource pressure, PR/CI, quota and compact trend strip; link dedicated details instead of embedding every terminal.
3. Add density/theme, keyboard focus, URL-backed filters, loading/empty/error/stale/unknown states and precision-safe values. Preserve row identity on deltas.
4. Mount terminal/graph/chart libraries only on demand and build a synthetic large-fleet fixture.

**Validate:** frontend lint/typecheck/Vitest/build; Playwright `overview.spec.ts`, `stream-reconnect.spec.ts`. **Pass:** 4, 20 and 100 project fixtures remain readable/interactive; source meanings match legacy or show explicit changed definition; no polling full status every 15 seconds.

### P13 — Workplan registry, legacy adapters and project UX

**Depends on:** P00, P02, P03, P11, P12. **Owner:** workplans/product. **Release:** R1.

**Read first:** MASTER_PLAN §8.1; exact four collector functions and source paths in SOURCE_REVIEW; effective policy imports.

**Write scope:** `crates/admiralty-server/src/services/legacy.rs`; new `admiralty-core/src/plans/{import,lint,revision}.rs`; `configs/profiles/legacy/`; CLI `project.rs` and `work.rs`; dashboard `features/{projects,workplans}/`; `tests/integration/plan_import.rs`.

1. Implement `PlanSource` for typed JSON/TOML and bounded Markdown preview; use declarative source/state maps for existing project formats, retaining parser version and original source IDs.
2. Implement immutable plan revisions, DAG/cycle checks, interface dependency levels, unknown criteria and explicit canonical-authority selection.
3. Implement packet lint rules from MASTER_PLAN §8.1 and source/sibling-key suggestions without claiming static completeness.
4. Build import diff/ambiguity resolution, searchable task table, optional DAG, revisions and blocker categories. Compatibility status exports are read-only views.

**Validate:** `rtk cargo test -p admiralty-testkit --test plan_import`; Playwright `workplan-import.spec.ts`. **Pass:** repeated import is idempotent, checkboxes/legacy PASS cannot become trusted acceptance, ambiguous scope is reviewable before admission.

### P14 — Atomic admission, fairness, cheap-capable routing and recovery

**Depends on:** P06, P09, P10, P13. **Owner:** scheduler. **Release:** R1, multi-project tuning R2.

**Read first:** MASTER_PLAN §§6.3, 7.4, 8.2; P02/P03 guards; P10 liabilities.

**Write scope:** `crates/admiralty-core/src/scheduling/`; server `services/coordinator.rs`; `tests/integration/scheduler_admission.rs`.

1. Implement readiness plus one transactional claim/budget/resource/outbox admission. Record source read set and recheck before commit.
2. Implement weighted project fairness/aging, critical-path ordering, independent inference/compute permits and conservative bootstrap WIP.
3. Implement finishing reserves and project-scoped finish mode with hysteresis/expiring overrides; no default new-lane fanout behind a red train.
4. Implement qualified routing and failure-specific recovery; record eligible alternatives and actual observed execution. One bounded new-hypothesis repair is a default, not endless cheap retries.

**Validate:** `rtk cargo test -p admiralty-testkit --test scheduler_admission`. **Pass:** racing admissions cannot overspend/double-claim; unknown process holds ownership; env/scope failure does not blindly escalate; project starvation and blocked-integration fixtures pass.

### P15 — Immutable candidate, oracle and trusted verifier

**Depends on:** P04, P09, P13. **Owner:** verification. **Release:** R1.

**Read first:** MASTER_PLAN §8.3; `tools/sc_audit.py` exact-checkout rules; P02 receipt contract.

**Write scope:** `crates/admiralty-runtime/src/verification/{candidate,oracle,receipts,checks}.rs`; `tests/integration/exact_candidate.rs`.

1. Implement `freeze_candidate`, scope/deliverable inventory and oracle closure. Preserve full Git object format, tree/base/parents and environment identity.
2. Run named argv checks in a clean runner under reserved verification resources, recording outputs/exits/timeouts and exact artifact hashes.
3. Implement `evaluate_acceptance` requiring current contract, exact candidate, trusted issuer, required delivery stage and non-revoked dependency evidence.
4. Detect protected evaluator/test/profile/workflow relaxation; route legitimate changes separately. Verification cache keys bind all consequential inputs.

**Validate:** `rtk cargo test -p admiralty-testkit --test exact_candidate`. **Pass:** self-report, forged issuer, wrong checkout, stale policy/env and changed oracle fail; correct fixture produces reproducible receipt.

### P16 — Jankurai profile, staged gate and quality projections

**Depends on:** P09, P15. **Owner:** quality. **Release:** R1.

**Read first:** MASTER_PLAN §8.3; actual Jankurai executable/help/profile/schema/release tuple; existing hook chain and Git index semantics.

**Write scope:** `crates/admiralty-runtime/src/verification/jankurai.rs`; `configs/profiles/quality/`; `tests/fixtures/jankurai/`; `tests/integration/jankurai_staged.rs`; `docs/contributors/quality.md`.

1. Qualify tool and Admiralty Rust/SQLite/React profile with known-bad/good fixtures; report unsupported audit scope explicitly.
2. Implement exact-index materialization, required full-context fallback and hook-chain preservation without stashing/resetting user changes.
3. Bind score/findings to exact tree/base/profile/policy/auditor; keep working/staged/integration/main products distinct.
4. Add project enrollment receipt, hard findings, ratchet and a repair-task proposal source. No hidden test threshold reduction to meet a score.

**Validate:** `rtk cargo test -p admiralty-testkit --test jankurai_staged`. **Pass:** partial stage, alternate index, unborn HEAD, linked tree, hooksPath, rename/binary and oracle-change matrix passes. **Stop:** profile cannot audit the claimed stack.

### P17 — Peer interface coordination and agent tool integration

**Depends on:** P05, P06, P08, P11, P13, P14, P15. **Owner:** coordination/agent integration. **Release:** R1.

**Read first:** MASTER_PLAN §5; P05 question states; P13 dependency levels; P15 trusted versus provisional evidence.

**Write scope:** `crates/admiralty-core/src/coordination/{claims,interfaces}.rs`; new `admiralty-runtime/src/agent_tools/`; dashboard `features/{coordination,agents}/`; `tests/integration/peer_handoff.rs`. Request P11 API registration centrally.

1. Implement scoped `InterfaceContract`, provisional artifact ACK/rejection, dependent invalidation and single-checkout writer claims. Interface disagreement routes to its owner before captain escalation.
2. Implement narrow bounded tool handlers and Rust MCP facade plus equivalent CLI JSON operations. Certify client capabilities individually; unsupported MCP clients use CLI fallback.
3. Deliver capsules at safe turn boundaries with recipient epochs, attention budgets and evidence retrieval. Keep proposed decisions distinct from authority.
4. Build coordination timeline and packet inspector with canonical expansion, pending questions, cause chain, recipient delivery state and role overhead.

**Validate:** `rtk cargo test -p admiralty-testkit --test peer_handoff`; Playwright `coordination.spec.ts`. **Pass:** API producer/UI consumer exchange immutable schema without manager relay; stale dependency invalidates; access/storm/compaction fixtures pass; no broad shell/SQL tools exposed for status.

## Parallel delivery and fleet operations

### P18 — Conflict analysis and frozen integration generations

**Depends on:** P09, P14, P15, P17. **Owner:** integration runtime. **Release:** R2.

**Read first:** MASTER_PLAN §8.4; official Git merge-tree; project merge policy.

**Write scope:** `crates/admiralty-runtime/src/integration/{conflict,train}.rs`; `tests/integration/integration_train.rs`.

1. Implement path/hunk/common-base risk and real merge probes; preserve ancestry in cache keys. Add build/symbol impact interfaces without blocking on full indexing.
2. Implement one integration writer, small bounded generations, freeze/review/next-generation flow and combined semantic validation.
3. Implement culprit isolation/bisection under a capped tool budget, changed-base invalidation and explicit stale assumption events. No automatic arbitrary conflict rewrite.
4. Emit train queue/age/throughput for P14 finish-mode and P29 UI. Workers continue only in independent lanes while review waits.

**Validate:** `rtk cargo test -p admiralty-testkit --test integration_train`. **Pass:** same-tree/different-ancestry, three-candidate interaction, hot lockfile and stale-base cases do not reuse invalid proof; no concurrent target writer.

### P19 — Exclusive publication gateway and forge reconciliation

**Depends on:** P03, P15, P16; P18 for multi-candidate live train. **Owner:** publisher. **Release:** R1 simulator/candidate path, R2 live qualified path.

**Read first:** MASTER_PLAN §§6.4, 8.4; `README.md`; `captains/COMMON.md`; owner-only exceptions from private policy.

**Write scope:** `crates/admiralty-runtime/src/integration/{publisher,reconcile}.rs`; `tests/integration/publication_gate.rs`; `docs/operators/publication.md`.

1. Implement one merge/publication admission predicate shared by API/UI/executor: exact current head, configured reviewer/checks, permitted method, scope generation and owner-only restrictions.
2. Isolate upstream write credentials from workers/verifiers. Use durable intents and one unresolved effect slot per target.
3. Reconcile timeout after possible remote success; delayed permits/head/base changes cannot reopen a competing mutation slot.
4. Keep local proposals/candidates usable without remote credentials. Live qualification uses an explicitly configured disposable repository, not existing owner-only PRs.

**Validate:** `rtk cargo test -p admiralty-testkit --test publication_gate`. **Pass:** wrong/dismissed reviewer, new head, pending check, stale executor and ambiguous merge produce no unauthorized/duplicate effect. **Artifact:** exact-head policy and reconciliation receipts.

### P20 — Safe retirement executor and captain asset accountability

**Depends on:** P04, P09, P15. **Owner:** assets. **Release:** R1 trusted-local qualification; isolated fleet backend qualified by R3.

**Read first:** MASTER_PLAN §9.2; P00 foreign-cleanup fixture; P04 pin/retirement state machine.

**Write scope:** `crates/admiralty-runtime/src/workspace/cleanup.rs`; `tests/integration/cleanup_custody.rs`; `docs/operators/cleanup.md`.

1. Implement preview/claim/recheck/quarantine/delete/receipt over registered assets only, using stable directory/file identities and no-follow behavior.
2. Preserve dirty, ignored, private, unpushed, unmerged, process-held, foreign and backup/candidate-held data. Git worktree removal never uses force.
3. Recover crashes during retirement/quarantine. Unknown or unregistered discovery creates an inventory/proposal, never auto-custody.
4. Emit owned debt and disposition metrics; score cannot improve by deleting evidence or misattributing another owner's assets.

**Validate:** `rtk cargo test -p admiralty-testkit --test cleanup_custody`. **Pass:** pin race/mount swap/symlink/active PID/ignored data/foreign root fixtures preserve unique work. Tests use disposable roots only.

### P21 — Second provider and native nested-usage qualification

**Depends on:** P08, P10, P17. **Owner:** providers. **Release:** R2.

**Read first:** MASTER_PLAN §7.3; actual installed Claude transport/help; provider usage overlap semantics.

**Write scope:** `crates/admiralty-runtime/src/providers/claude.rs`; `tests/fixtures/providers/claude/`; `tests/integration/provider_claude.rs`; provider compatibility documentation.

1. Implement qualified structured stream parsing, launch/input/resume semantics and actual configuration binding.
2. Normalize native cache-read/cache-write/output categories and parent/child inclusion without double count; opaque native subagents are labeled, not presented as fully managed peers.
3. Add golden negative/approval/turn-boundary/context-floor/version-canary fixtures and one bounded real task/continuity receipt.

**Validate:** `rtk cargo test -p admiralty-testkit --test provider_claude`. **Pass:** capability and economic matrix honest; coordination tools work or have declared fallback; same core invariants pass as first adapter.

### P22 — Existing agy/Gemini, Grok and generic process pathways

**Depends on:** P08, P10, P17, P21. **Owner:** providers. **Release:** R4; observed legacy support available earlier.

**Read first:** actual provider binaries/version/help, existing `agent_proc`/model classifiers and current routing policy; MASTER_PLAN §§7.2–7.4.

**Write scope:** `crates/admiralty-runtime/src/providers/{agy,grok,process}.rs`; corresponding fixture directories and `tests/integration/provider_matrix.rs`.

1. Certify structured protocol where actual products expose it; otherwise implement conservative qualified PTY/process lifecycle and explicit unknown usage.
2. Preserve currently operating workflows as observed legacy sessions until a native driver passes. Do not conflate provider Agent Client Protocol support with Admiralty's internal coordination schema or MCP facade.
3. Add per-version golden/capability matrices, upgrade canary and bounded real checks for advertised transports.

**Validate:** `rtk cargo test -p admiralty-testkit --test provider_matrix`. **Pass:** each claimed feature has evidence; missing auth/usage/resume yields specific unsupported status. Unavailable real qualification blocks that capability claim, not the offline demo or qualified providers.

### P23 — Node enrollment, authenticated transport and durable transfer

**Depends on:** P03, P04, P07, P09. **Owner:** fleet transport. **Release:** R3.

**Read first:** MASTER_PLAN §§4, 6.5, 10.4; P02 protocol negotiation; P07 local journal.

**Write scope:** `crates/admiralty-runtime/src/fleet/{enrollment,transport,transfer}.rs`; `src/bin/admiralty-node.rs`; CLI `node.rs`; `tests/integration/fleet_transport.rs`. Protocol edits go through integration.

1. Implement one-use expiring enrollment, persistent node UUID/key/certificate, revocation/rotation and explicit capacity/runner capability inventory.
2. Implement mTLS streams with producer incarnation/sequences, contiguous durable ACK, missing-range replay, bounded backpressure and protocol compatibility.
3. Transfer artifacts resumably with final digest/size validation and access/custody checks. Disconnection never grants new authority.

**Validate:** `rtk cargo test -p admiralty-testkit --test fleet_transport`. **Pass:** duplicate enrollment, replay, partial transfer, certificate revoke, node reboot and sequence-gap fixtures fail closed or reconcile without lost acknowledged events.

### P24 — Fleet placement, partitions and arbitrary node topology

**Depends on:** P14, P19, P20, P23. **Owner:** fleet scheduling. **Release:** R3.

**Read first:** MASTER_PLAN §§7.5, 8.2, 11.1; private node/custody inventory; P23 capability reports.

**Write scope:** `crates/admiralty-runtime/src/fleet/placement.rs`; new `fleet/partition.rs`; `tests/integration/fleet_partition.rs`; `docs/operators/fleet.md`.

1. Implement placement by capability, locality, reservations, account pools and current pressure; no assumptions about node count or names.
2. Partition policy permits only already-granted isolated computation within budget and with local spool reserve. Publication/new authority stops without its controlling boundary.
3. Reconcile reconnect/reboot/stale permits before releasing claims or moving ordinary work. Distinguish shutdown request from confirmed termination.
4. Qualify synthetic four-node topologies with random UUIDs and the intended installation inventory as private data. Never probe protected/unregistered hosts.

**Validate:** `rtk cargo test -p admiralty-testkit --test fleet_partition`. **Pass:** no duplicate writer, stale publication, lost contiguous ACK or released unknown liability under partitions/clock skew/controller disconnect.

### P25 — Release bundles, maintenance handoff and disaster recovery

**Depends on:** P03, P04, P07, P19, P23, P24. **Owner:** operations. **Release:** R3.

**Read first:** MASTER_PLAN §§6.5, 11.2; actual service manager semantics; P23 compatibility records.

**Write scope:** `crates/admiralty-runtime/src/operations/`; `packaging/{systemd,launchd}/`; `tests/integration/{upgrade_continuity,disaster_restore}.rs`; `docs/operators/{upgrades,recovery}.md`.

1. Implement signed complete bundles, stage/check/activate/retire jobs and lifetime pins for CLI/runtime/adapter/prompt/policy/oracle/image.
2. Gate on every active version, provider mutable state and replay protocol; defer incompatible updates with named blockers. New-work aliases do not mutate active attempts.
3. Implement quiesce/drain/backup/fence/handoff/reconcile/resume with independently supervised sessions; test actual service restarts and rollback.
4. Restore artifact closure and actual watermark under a fresh external recovery generation; conclusively fence old credential-bearing executor before effects resume.

**Validate:** `rtk cargo test -p admiralty-testkit --test upgrade_continuity`; `... --test disaster_restore`. **Pass:** long structured/PTY fixture jobs survive supported ordinary upgrades; missing blob, old publisher or incompatible matrix blocks activation/effects honestly.

## Complete product and release

### P26 — Economics and coordination-overhead explorer

**Depends on:** P10, P12, P17. **Owner:** frontend/economics. **Release:** R2 core, R4 comparison polish.

**Read first:** MASTER_PLAN §§5.9, 9.1, 10; P10 metric dictionary; P06 context decomposition.

**Write scope:** dashboard `features/economics/`; dedicated `e2e/economics.spec.ts`; metric-query additions requested from API owner.

1. Build role/purpose/provider/project/cache breakdown, manager share, task spend, harness-floor estimates and context-rotation outcomes.
2. Add fixed-cohort comparison, failed roots, maturity/rework, coverage/nulls and separate currencies/quota. No summed cross-unit fleet money score.
3. Add metric definition/provenance panel and bounded export; preserve gaps and version discontinuities in charts.

**Validate:** frontend typecheck/Vitest and named Playwright spec. **Pass:** huge counters exact, missing costs unavailable, all-in versus coordination savings explicit, legacy series visibly distinct.

### P27 — Excursions, bounded inquiry and local improvement proposals

**Depends on:** P06, P10, P15, P18, P19. **Owner:** improvement/incidents. **Release:** R4.

**Read first:** MASTER_PLAN §9.3; P00 historical/synthetic fixtures; current publication grants.

**Write scope:** server `services/improvement/{excursions,proposals}.rs`; dashboard `features/incidents/`; `tests/integration/excursion_lineage.rs`.

1. Implement episode/family fingerprints and cause/evidence graph; avoid fail-plus-escalate double incidents while retaining recurrence and rework.
2. Implement deterministic diagnosis, bounded investigator tasks, tried-approach context and uncertainty. RCA is not an unrestricted fleet transcript reader.
3. Generate sanitized local proposals with reproduction, suggested patch and measurable follow-up. External issue/draft-PR posting requires an existing configured grant and gateway reconciliation.

**Validate:** `rtk cargo test -p admiralty-testkit --test excursion_lineage`; incident UI tests. **Pass:** duplicates, recurrence, unknown attribution, secret-bearing evidence and ambiguous publication handled without leaking or fabricating root cause.

### P28 — Prompt bundles, trials, feature datasets and promotion

**Depends on:** P06, P10, P25, P27. **Owner:** improvement/evaluation. **Release:** R4.

**Read first:** MASTER_PLAN §9.3; P02 protected-oracle rules; P10 fixed cohorts; source review cautions about causal claims.

**Write scope:** server `services/improvement/{experiments,datasets}.rs`; new `admiralty-core/src/evaluation/`; dashboard `features/experiments/`; `tests/integration/experiment_validity.rs`; `docs/architecture/improvement.md`.

1. Register immutable prompt/policy bundles and point-in-time feature manifests with extraction cost, freshness, missingness and decision-phase rules.
2. Implement replay trials and fresh/held-out controlled assignments with cluster contamination, maturity, multiplicity, negative outcomes and all-in cost. Accepted future tests are not automatically independent hidden tests.
3. Implement independent promotion/rejection/insufficient-evidence and champion rollback affecting new work only. Authority/evaluator changes require their configured review.
4. Export reproducible decisions/actual execution/usage/outcomes datasets. Learned routing remains optional until a measured shadow/canary benefit beats transparent rules.

**Validate:** `rtk cargo test -p admiralty-testkit --test experiment_validity`. **Pass:** leakage/invalid propensity/repeated peeking/shared-manager/immature-label fixtures cannot auto-promote; one bounded candidate can be honestly accepted or rejected and rolled back.

### P29 — Integration, quality, assets and fleet UX polish

**Depends on:** P12, P16, P18, P19, P20, P24, P25. **Owner:** frontend/operations. **Release:** R2 relevant views; R4 complete.

**Read first:** MASTER_PLAN §10; actual API/jobs; supplied screenshots and logo provenance.

**Write scope:** dashboard `features/{integration,quality,settings}/`; new `features/assets/`; their named E2E tests. Does not edit economics, experiments, workplans or coordination feature ownership.

1. Build exact-candidate/check/reviewer train view, scoped quality trends, cleanup preview/receipts, node enrollment/drain and staged-update blockers.
2. Add consistent durable-action/job states and command palette registrations through integration-owned app wiring. Preserve authorized one-step reversible actions.
3. Complete keyboard/screen-reader/contrast/reduced-motion checks, responsive layouts, large tables and error/empty/offline flows. Graphs remain optional views.

**Validate:** frontend checks; Playwright integration/quality/assets/settings specs; automated accessibility plus manual keyboard checks on named browsers. **Pass:** user can find blocker/evidence and resolve an authorized action without reading terminals.

### P30 — Distribution, brand, documentation and contributor path

**Depends on:** P01, P11, P12; full package completion requires P21, P25, P28, P29. **Owner:** release/product. **Release:** incremental from R0, complete R4.

**Read first:** MASTER_PLAN §§10.4, 14; actual supported OS/provider matrix; existing README merge rules; supplied logo original.

**Write scope:** `scripts/{package,release}/`; CLI `demo.rs`; `docs/{operators,contributors,brand}/`; dashboard `public/brand/`; root README/CONTRIBUTING/SECURITY/CODE_OF_CONDUCT/license files; `.github/` release/templates owned centrally.

1. Implement fully isolated fake-provider demo and first-five-minutes doctor/init/setup flows; separate simulated data/keys/ports from live state.
2. Package binaries/static assets/migrations/profiles and signed/checksummed release manifest, SBOM/provenance. Test archive install before optional package-manager/installer automation.
3. Preserve original logo provenance, create tested derived sizes and obtain deliberate small-size mark. Record maintainer-approved code/data/logo rights; do not assume public source is licensed.
4. Write complete install/provider/fleet/workplan/coordination/update/backup/recovery/contribution and adapter conformance guides. Name/trademark/package availability is a release check, not permission to rename all code mid-build.

**Validate:** clean-machine archive install; `admiralty demo` offline; `doctor` missing-tool flows; reproducible package/assets compatibility and secret/dependency/license checks. **Pass:** no mandatory account/telemetry/Node server, useful demo, truthful support matrix. **Stop:** unresolved rights block public distribution, not private implementation progress.

### P31 — Qualification, prospective savings and actual fleet rollout

**Depends on:** P00 through P30. **Owner:** integration/release. **Release:** R4, subsets run at each earlier gate.

**Read first:** MASTER_PLAN §§11–14; every integrated receipt; live inventory/effective private policy; P30 recovery runbooks.

**Write scope:** `tests/qualification/`; `benchmarks/`; `docs/operators/deployment.md`; plan receipts and EXECUTION ledger. Production changes follow a concrete deployment manifest with exact scope and current authority.

1. Run the complete invariant/fault matrix, declared load/query profiles, 24-hour synthetic soak, clean install and bounded real transport continuity checks. Publish actual supported limits.
2. Run paired/frozen context-fidelity comparisons and a prospective complete-cost coordination trial. Target ≥90% task-level token consumption, ≤10% coordination initially/5% stretch and ≥70% less coordination context; retain inconclusive/negative results and coverage.
3. Execute staged migration: inventory/backup, read-only shadow on separate port/data root, observed sessions, new-task canary, one project at a time, arbitrary enrolled nodes, then familiar UI endpoint. Disable exactly corresponding legacy mutation scopes before takeover.
4. Rehearse rollback and restore; reconcile unresolved effects and active bundle pins. Produce final fleet handoff with identities, versions, roots, policies, service units, endpoint, backup watermark and limitations.

**Validate:** all MASTER_PLAN §13 commands and release evidence bundle. **Pass:** full completion contract, all core capabilities evidenced, no protected live job interrupted, no unowned scope adopted. Economic target misses prevent a savings claim; invariant misses block affected rollout.

## Dependency order and safe parallel packets

These are potential assignments for implementation, not agents launched during planning. The integration owner enforces the latest write scopes. Never parallelize packets merely because their names differ.

| Available after | Disjoint packet group | Shared boundary retained by integration |
| --- | --- | --- |
| Start | P00 and P01 | P00 fixtures/docs; P01 workspace/testkit |
| P04 complete | P05 and P07 | Runtime `mod.rs`, schemas and manifests |
| P05/P07 complete | P06, P09 and P11 | API/tool registration, resource protocol and migration edits |
| P06/P09/P11 complete | P08 and P12 | Provider trait and generated TS |
| P08/P12 complete | P10 and P13 | Metric/configuration schemas and app wiring |
| P10/P13 complete | P14 and P15 | Acceptance/admission public contract |
| P14/P15 complete | P16, P17 and P20 | No shared verification/runtime wiring edits |
| P16/P17 complete | P18, P19 simulator path, P21 and P23 | Provider trait/node protocol generation; P19 live train waits for P18 |
| P19/P23 complete | P24 and P27 after their other prerequisites | Fleet transport and incident service stay disjoint |
| P24/P25 stable | P26, P27, P29 after their prerequisites | Separate feature folders and API registration |

P19 begins with a single-candidate simulator after P15/P16; its live multi-candidate path waits for P18. P31 qualification subsets run early, but the packet closes only after full rollout evidence. P25 and P30 jointly need service packaging; P25 owns service units, P30 consumes them without rewriting them concurrently.

## Release package map

- **R0:** P00–P06, P11 core and P12; P30 synthetic-demo/documentation seed. No live input or publisher authority.
- **R1:** P07–P10, P13–P17, P19 single-candidate simulator path and P20 local retirement. One real bounded task and peer coordination demonstrated; publication can remain candidate-only under policy.
- **R2:** P18, P19 live gateway, P21, P26 core and P29 integration/quality/assets views. Useful parallel multi-project delivery.
- **R3:** P23–P25 and corresponding P20/P29 isolated-fleet capabilities. Partition, restore and continuity proven before adding intended nodes.
- **R4:** P22, P27, P28, complete P26/P29/P30 and P31. Core product, improvement lifecycle and public release evidence.

Release subsets never mark an unfinished packet complete. Record which criteria are verified and which remain planned in EXECUTION and the packet receipt.
