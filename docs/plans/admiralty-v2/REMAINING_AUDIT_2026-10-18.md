# Admiralty remaining-work audit — 18 October 2026

Audited source: `7bb2784dc740faa3c07dd8c0ad72769dfa344927` (GitHub main observed during this audit). Live observations: 19:16–19:20 UTC / 13:16–13:20 MDT. This is an independent implementation and acceptance audit, not a release qualification. Three read-only reviewers examined runtime, product and evidence separately; integration checked the principal findings against source, all 32 dashboard-referenced receipts, GitHub jobs and live process/worktree metadata. No test suite was rerun, provider called, worker interrupted, production checkout edited, service changed or native execution enabled.

Read with [MASTER_PLAN.md](MASTER_PLAN.md), [WORK_PACKETS.md](WORK_PACKETS.md), [WORK_ORDER_REVIEW.md](WORK_ORDER_REVIEW.md) and the historical [EXECUTION.md](EXECUTION.md). The packet acceptance contracts remain authoritative. Old ledger references to held merge windows or queued CI describe their observation times, not current state.

## Finding

**The largest remaining work is connecting the implemented components into a complete, durable execution system.** Admiralty has real SQLite storage, replay, artifact and session primitives, provider parsers, conservative decision libraries, offline CLI flows, meaningful tests and a working detailed real-data React console. The native admission → execution → verification → acceptance path is incomplete. Fleet execution, deployment and recovery also require operational implementations and qualification.

The existing ledger has P00 in review and P01–P31 in progress. This audit establishes no complete parent-packet acceptance. P00 is chiefly evidence closeout; the other packets have varying amounts of implementation and qualification remaining. Counting open parent packets is not a percentage of engineering work remaining.

### Current operational facts

| Area | Observed fact | Implication |
| --- | --- | --- |
| Live server | `/healthz` is healthy and returns `native_execution_qualified:false`. Source also sets `MANAGED_EXECUTION_QUALIFIED=false`. | The new native engine has not earned activation. Existing Screen work is legacy execution. |
| Manager | The Admiralty Commodore's observed screen identifies Gemini 4 Argon; its AGY process is present in the Admiralty manager checkout at audited main. | The requested manager model is observed. This is screen/process evidence, not provider-backend attestation. |
| Workers | All 15 selected Admiralty Screen identities have stable live Codex descendants during the direct probe. Fourteen are in replacement acceptance worktrees; one remains in the older owned lane. Seven worker trees have tracked changes at that instant. | Preserve active and uncommitted work. Presence, token activity, new commits and dirty files do not establish accepted progress. |
| Resource custody | Fourteen replacement Codex processes allow a broad CPU set; the older lane is pinned to CPU 105. The observer does not inspect or grant cgroup/resource permits. | Reconcile actual admitted build/test capacity and effective child limits before adding concurrency. An allowed CPU set alone does not prove a permit violation or absence of other controls. |
| Dashboard claims | All 32 displayed `verified` entries reference shared-workspace reconciliation receipts for older source `82e00ca3`, observed at 16:06 UTC. All 32 receipts say `verified_shared_workspace_qualification`; their preserved packet status is `integrated`. Six lack a packet-specific receipt. The receipt text explicitly denies independent full-packet acceptance. | Display separate implementation, component verification, full acceptance and native-activation states. “32 verified” must not be read as “32 packets complete.” |
| Lane telemetry | All 15 Screen bindings are present, but 14 worktree-activity entries say `unsafe`, with empty current task/model assignment fields. | The old root-bounded metadata observer cannot establish progress in the replacement worktrees. Fix this with explicit registered roots and current bindings; do not weaken it to arbitrary home-directory traversal. |
| Source reference | Live rollout still points to merged PR49/head `1af0072c`, while audited main is `7bb2784d`. | Reconcile source, candidate, installed and running references independently. Merging does not deploy the pinned canary. |
| Pull requests | GitHub reports zero open Admiralty PRs at the initial observation. | PR cleanup is complete at that instant; substantial implementation still remains. |

Direct process metadata and raw receipt bodies are retained privately in ignored `target/quality/remaining-audit/`. They contain operational observations, not new authority or task acceptance.

## Critical gaps, in delivery order

1. **Complete the authoritative task lifecycle.** `crates/admiralty-core/src/events.rs::DomainEvent` has only submit, claim, start, candidate, queue-message and ACK events. `reducer.rs::SchedulingState` ends at `AwaitingVerification`. There is no authoritative accepted/rejected/failed/cancelled/retry/revision/dependency-release path. Consequently `server/src/lib.rs::fleet_from` always leaves native accepted counts at zero. Freeze versioned lifecycle and authority contracts, then add writer operations and recovery semantics before building dependent executors.
2. **Make compact inter-agent communication operational.** The scoped pending/ACK seam is real, but `store/src/inbox.rs::pending_page` scans global IDs and exposes global scan/cursor metadata to its trusted caller. Add a writer-maintained scope index before exposing it to agents. Authenticate actor/project/recipient/epoch grants; persist subscriptions, delivery outcomes, questions, deadlines and retry/expiry; activate the attention governor and bounded retrieval. Keep transport, provider and application ACKs separate. No network agent routes currently register these services.
3. **Finish one complete managed local task.** Wire atomic claim + budget/resource reservation + durable dispatch to an isolated workspace, independent session host, qualified provider, durable observations, immutable candidate, independent checks and accepted outcome. Prove restart/lost-reply behavior through the real boundaries. Show that task and its evidence in React. This is the central completion milestone.
4. **Connect own-model selection and escalation to that same path.** Own-model configuration, capacity preferences, Ornith→Luna fallback policy and parallel-candidate/synthesis proposals exist. They do not reserve real inference slots or automatically launch native workers. Integrate actual observed model/configuration, bounded attempts, failure evidence, reservation/settlement and independent candidate comparison. The separately validated xbabe4 OpenCode installation does not qualify Admiralty's native orchestration automatically.
5. **Implement verification, integration and publication executors.** Existing receipt, conflict and publication libraries primarily validate or propose. Add trusted independent check execution, exact candidate/base/policy binding, a durable integration writer and credential-isolated forge reconciliation. Prove timeout-after-success and stale-head handling with a configured disposable target before activation.
6. **Implement fleet and recovery boundaries.** The node binary currently refuses enrollment with exit78. Real enrollment/key custody, mTLS, replay/transfer, resource enforcement, placement, partitions, supervised upgrades and fenced restore are still required. General CI VM infrastructure is separate from native agent-runner isolation.
7. **Complete product and work-order journeys.** Keep the restored detailed view and plots; add live work intake, workplan revisions, coordination/attention, durable actions/jobs, integration, quality, assets, fleet operations, economics and experiments. WO1–WO6 are planned requirements, not features established by current tests.
8. **Qualify and release the working system.** Stabilize exact-source CI; add browser/accessibility and load tests, real provider continuity, clean install, signed/checksummed distribution, restore/rollback and the declared 24-hour soak. Measure context and cost savings prospectively before claiming the targets.

Shared state/contracts/migrations/API registration remain integration-owned. These steps are dependencies, not authorization for an unsafe production cutover or broad fleet cleanup.

## P00–P31 reconciliation

“Present” below means an inspected component or retained scoped proof, not full acceptance. Every row remains open under the original packet contract.

| Packet | Present | Remaining implementation or acceptance gate |
| --- | --- | --- |
| **P00** Sources/fixtures | Pinned manifests, 37-source/11-case verifier, distinct historical baselines and unknown-explicit inventory. | Final independent artifact/receipt closeout and current deployment inventory. Preserve synthetic labels and source provenance. |
| **P01** Workspace/CI | Rust/React workspace, binaries, deterministic clock/fault/provider/forge testkit, substantive CI. | Stable exact-source passing pipeline, registration audit and final matrix reconciliation. Node enrollment/help behavior remains incomplete. |
| **P02** Contracts | Typed tasks/messages, pure reducer, schema/TS generation and configuration precedence. | Full lifecycle/authority/policy negotiation; versioned work-order, intake, schedule and delivery contracts. |
| **P03** Writer/replay | Serialized SQLite writer, WAL/durability checks, atomic domain/outbox operations, replay, backup and workplan registry. | Actual effect admission/execution/reconciliation; writer/transaction/WAL observability; bounded growth/query performance. Reducer still clones the full projection per append. |
| **P04** Artifacts/custody | Durable content-addressed bytes; transactional custody/pin helpers. | Writer-integrated trusted filesystem observations and availability; scoped range reads, orphan/missing-object reconciliation, storage reserve and complete lifecycle. Standalone helpers are not an activated artifact service. |
| **P05** Coordination | Durable message/ACK facts, scoped internal pending/ACK service, question journal and bounded mailbox/topic components. | Scope index; actual actor/grant authentication; durable subscriptions/delivery/question/decision service, expiry/unknown/retry, priorities/coalescing and attention integration. |
| **P06** Context | Protected task capsule, epochs and pure bounded governor. | Other capsule forms, persisted exact render/baseline/epoch state, alias/fallback, structured tool/failed-approach memory, bounded retrieval and real role/purpose/token accounting. |
| **P07** Sessions | Real pipe host, private socket daemon, durable output/input journals and reconnect fixtures. | Restart adoption, admitted launch reconciliation and independent service supervision; PTY path where advertised. Existing module explicitly limits ownership to the host lifetime. |
| **P08** First provider | Codex launch/parser/usage/supervision helpers and controlled transport harness. | Register admitted real execution; bind executable/config/model/custody; qualify the declared supported tuple and complete task outcome. Conformance evidence is narrower than native acceptance. |
| **P09** Workspaces/resources | Read-only registered workspace checks and metadata observations. | Managed workspace/private-clone creation, rootless isolated runner, mounts/network/credentials, enforced CPU/memory/PID/disk/GPU/inference permits and qualified caches/images. |
| **P10** Economics/budgets | Native usage primitives, conservative reservation/settlement and cohort reducers. | Durable ledger/CAS and actual ingestion/correction lineage; atomic reserve/dispatch/settle/reconcile, resource custody and query service. |
| **P11** API/CLI | Loopback bearer-protected reads, bounded SSE, assets and offline commands. | Scoped actors/commands/idempotent durable jobs, browser auth/origin/CSRF design, terminal WS/input lease and agent tools. Current fleet token reads the whole projection. |
| **P12** React parity | Dense detailed real-data view, project tabs, colored models, hierarchy, profile/Grok assets, temporal plots and original legacy page. | Browser reconnect/E2E, URL-backed navigation/filters, large-fleet performance, accessibility/responsiveness evidence; replace full legacy polling where useful. |
| **P13** Workplans | JSON preview, immutable offline register/show/list and validation; searchable read-only task table. | TOML/bounded Markdown/adapters, ambiguity/import-diff and revision/dependency UI, full admission lint, canonical authority and live service wiring. |
| **P14** Scheduling | Pure readiness/routing logic and fixtures. | Atomic claim/budget/resources/outbox, durable scheduler/launch recovery, fair aging/finish mode under real work, environment waits and timezone/DST schedules. |
| **P15** Verification | Bounded receipt consistency/replay validation. | Frozen candidates/oracles, independent check execution in clean reserved runners, authenticated issuer/evidence custody and durable accept/reject transitions. |
| **P16** Jankurai | Pure staged-audit evaluation. | Actual tool/profile qualification, exact-index materialization, hook preservation, trusted execution, enrollment/ratchet and repair proposals. |
| **P17** Peer/tools | Interface compatibility/invalidation reducer. | Durable claims/interfaces; narrow authenticated MCP/CLI handlers; safe-turn capsules; coordination timeline/packet inspector and producer→consumer handoff without manager relay. |
| **P18** Integration | Conservative conflict graph and frozen-generation proposals. | Real Git merge probes, one durable train writer, combined checks, ancestry-aware proof caching, culprit isolation and stale-base recovery. |
| **P19** Publication | Exact-evidence admission/fencing/reconciliation contracts and offline forge regressions. | Credential-isolated executor, durable unresolved target slot, real forge reconciliation and disposable-repository qualification; per-target delivery evidence. |
| **P20** Retirement | Custody helpers and conservative retirement proposals. | Actual registered-asset preview/claim/recheck/quarantine/delete/receipt executor, stable identities, unique-work protection and crash recovery. Manual cleanup does not implement this product feature. |
| **P21** Claude | Launch proposal, strict stream parser and usage fixtures. | Admitted launch/input/resume/config/journal continuity, native nested-usage qualification and bounded real continuity receipt. |
| **P22** Other providers | AGY/Grok/generic/OpenCode SSH observation/proposal components; working legacy workflows. | Capability-specific native executable/version/transport/lifecycle qualification. Current Gemini manager and Ornith service observations remain separate from native admission. |
| **P23** Enrollment/transport | Pure enrollment/revocation contracts. | Persistent node keys/certificates, one-use enrollment service, mTLS, producer incarnation/contiguous ACK/replay/backpressure and resumable verified artifacts. |
| **P24** Placement | Pure capacity/partition proposals. | Authenticated live reports, reserved dispatch, partition authority enforcement and reconnect/reboot/claim reconciliation; real arbitrary-topology fault qualification. |
| **P25** Upgrade/recovery | Backup primitive and recovery manifest/fence/closure validation. | Complete signed bundles/service packaging; stage/activate/retire jobs; real drain/handoff/session survival; restore executor and independently verified old-executor fencing. |
| **P26** Explorer | Accounting/cohort primitives and restored legacy token/cost/history panels. | Native role/purpose/provider/project/cache explorer, manager share/harness floor/rotation, fixed-cohort/maturity views, definitions, dimensional units and bounded export. |
| **P27** Incidents | Bounded episode/recurrence reducer and sanitized local proposal builder. | Durable observation→diagnosis→bounded investigator→proposal lifecycle, scoped evidence and UI; separately authorized gateway publication. |
| **P28** Trials | Deterministic fixed-cohort assignments and independent decision proposals. | Bundle/dataset registry, actual controlled trials, independent promotion/rollback for new work, reproducible exports and experiment UI. |
| **P29** Operator UX | Recorded diagnostic CLI and legacy operational facts; requested hierarchy styling. | Train/check/review, quality, assets, enrollment/drain/update blockers; durable actions/jobs, command palette, Needs attention/delivery, E2E and accessibility. |
| **P30** Distribution | Isolated demo, source launchers, capability doctor and deterministic archive builder. | Clean-machine setup/install, complete binaries/assets/migrations/profiles, signing/SBOM/provenance, support matrix and full operator/contributor/security docs. Code/data/logo rights and release license remain unresolved. |
| **P31** Release proof | Actual isolated HTTP/CLI smoke and synthetic invariants/faults. | Complete managed task, actual providers/transport/restarts, load/24-hour soak, clean install, prospective cost/context study, restore/rollback, fenced canary/fleet cutover and deployed handoff. |

### Work-order additions

All six subpackets remain planned. Production crates/apps/packages contain no implementations of the named `WorkOrderRevision`, `IntakePreview`, `record_intake`, `find_duplicate_candidates`, `SchedulingProfile`, `DeliveryRequirement`, `DeliveryObservation` or `AttentionItem` seams.

| Subpacket | Remaining output |
| --- | --- |
| **WO1** | Versioned request/provenance/source-span/outcome/preview/duplicate/delivery contracts and bounded synthetic intake preview. |
| **WO2** | Sole-writer request/binding registry, work-order AND plan CAS, source custody, exact retry and bounded all-state duplicate search; restart/lost-reply proofs. |
| **WO3** | Inert pinned-snapshot todoq family adapters and provenance diagnostics. Imported text/commands must remain data. |
| **WO4** | Configured timezone/DST window resolution and distinct setup/toolchain/resource eligibility/waits. |
| **WO5** | Exact task/contract/candidate/target delivery joins; desired, merged, installed, running and released dimensions; stable attention reasons/ages. |
| **WO6** | CLI intake preview/record/show/search and React Intake/Attention/Delivery journeys through authorized services; intake itself must not silently dispatch. |

## CI and evidence quality

### Hosted CI is executing, but current source is red

The main workflow uses the general `neverhuman-lan` group with `self-hosted/Linux/X64/lan-ci` capability labels. Inspected current/preceding jobs actually ran on xbabe1, xbabe2 and xbabe3. The earlier dedicated ARM64 run `39147482922` passed on the shared xbabe4 pool. That historical ARM64 qualification does not prove the latest source passed or establish current pool health; the ordinary main workflow does not request ARM64.

| Source/run | Observed result | Required follow-up |
| --- | --- | --- |
| `7bb2784d`, [39157164561](https://github.com/neverhuman/admiralty/actions/runs/39157164561) | Contracts/frontend succeeds on xbabe1. Rust job on xbabe3 fails with GitHub annotation **“The self-hosted runner lost communication with the server.”** | Inspect runner/controller/network/resource evidence with its owner. Do not label this a proven code failure or blindly extend test timeouts. Re-run exact source after the diagnosed cause is repaired. |
| `4b295966`, [39157123935](https://github.com/neverhuman/admiralty/actions/runs/39157123935) | Contracts succeeds. Rust exits101: three `admiralty-cli/tests/offline_roundtrip.rs` tests exceed the bounded child timeout, including demo, work submit and workplan register. | Determine blocked child/I/O/resource cause and preserve failed evidence. A later runner-loss result does not establish these source paths are fixed. |
| `24937f66`, [39149864310](https://github.com/neverhuman/admiralty/actions/runs/39149864310) | Earlier complete workflow succeeds. | Retain source attribution; this success cannot certify later test/harness/source changes. |

Read-only org runner inventory returned HTTP 403 with the current CLI identity. This audit did not request broader privileges or substitute another identity. Assigned job metadata verifies actual hosts used; org-wide current runner count/controller health is an explicit observation limit.

### The latest rounds mostly add tests

Between source `96ab86317aaeaa553f79386098dc55ff05399d5d` and audited main: **147 commits; 115 changed files; 18,062 added lines**. Tests and Cargo manifests account for 17,079 additions (94.6%). Documentation/receipts account for 797. Four runtime/dashboard/tool files account for 186; the only Rust `src/` modification changes an existing test timing assertion. No core/store/server/CLI production logic or React feature source changed in that interval. Functional Python work concerns legacy observation/audit behavior.

This is evidence of active work and useful regression growth. It does not close the operational gaps above. Further numbered qualification rounds should require a new implementation or a distinct unresolved failure hypothesis.

Concrete cleanup findings:

- `tests/integration/codex_round14_p11_p12_p18.rs` contains an authenticated SSE/conflict test, but its only module reference is in the root `tests/integration.rs`. Root Cargo has `autotests=false` and registers other explicit targets. This test is not reached by the current workspace harness; register it centrally with its required dependencies.
- `tests/integration/codex_accept_p12_p18.rs::p11_p12_p18_round12_item5_redteam_audit_gate` tries an absolute host receipt first, then constructs a synthetic successful log/receipt when reading fails. Its assertions prove fixture digest/JSON consistency, not authenticated prior execution. It also remains host-dependent. Use an explicitly synthetic bounded fixture for the unit test and verify real source-bound execution receipts separately.
- Test names containing “end_to_end_cutover” do not imply real cutover. For example `codex_round16_p26_p31.rs` operates on in-memory reservations/economics; P23 exercises enrollment reducers; P25/P30 asserts recovery grants no restore/effect authority. Keep these regressions, label their boundary accurately, and add actual operational tests when implemented.
- Seven newly tracked packet receipts describe partial source `9531ba08` and explicitly deny packet acceptance/native qualification. The source tree lacks fresh complete Round 13–16 acceptance reconciliation. The 32 private dashboard receipts were read directly; they also explicitly disclaim full acceptance. Additional private worker output is not automatically accepted evidence.
- No tracked Playwright configuration/specs, accessibility suite, benchmark or 24-hour soak harness was found. Current CI runs meaningful Rust, frontend, contract, legacy, fixture, packaging and isolated smoke checks; browser/performance/soak acceptance remains additional work.

## Recommended next dispatch and completion proof

Keep implementation parallel through disjoint write scopes; reserve integration/verification capacity instead of adding workers without current permits. Use the existing packet contracts and avoid creating a new P32 parent from a test filename.

| Next bounded lane | Scope and handoff | Done when |
| --- | --- | --- |
| **Integration: lifecycle** | Core event/reducer/contracts, Store operations, migrations and generated bindings. Other lanes consume pinned interfaces. | Versioned success/failure/retry/cancel/revision/dependency transitions replay deterministically; authority and exact evidence enforced; lost reply/restart tested. |
| **Coordination** | Pending index, service authorization, durable subscription/delivery/question operations. Shared writer/API changes requested from integration. | Agent-scoped pages reveal no other scope; durable delivery/ACK/epoch/restart works; relevant capsule reaches worker without a full manager report. |
| **Execution** | Session/provider/workspace/resource implementations after lifecycle/admission interface freeze. | One admitted task actually launches under reserved resources, survives the supported restart, records real observed provider identity/usage and yields immutable candidate evidence. |
| **Verification/publication** | Trusted check runner and exact-candidate evaluation; later credential-isolated publication gateway. | Real clean-runner checks cause durable acceptance/rejection; stale proof cannot pass; remote ambiguity is reconciled exactly once. |
| **Product/intake** | WO1 preview contracts first; then bounded import/attention/delivery UI consuming pinned API. Preserve detailed console. | User submits/revises work, sees cause/owner/age and real progress, and reaches exact evidence without terminals; named browser tests pass. |
| **CI/quality** | Harness registration, synthetic receipt clarity, child timeout diagnosis and runner loss; centrally owned workflow only. | Exact candidate passes meaningful existing checks; failure evidence retained; no synthetic receipt counted as real acceptance. |

Before production activation, require the P31 staged-canary receipt: exact source/build/config, registered roots and permits, actor/effect authority, old/new admission ownership by task scope, supported restart/provider matrix, verified backup/restore/fence, rollback and actual installed/running versions. Do not replace this with a blanket test count.

Remaining evidence must name the criterion, exact source and policy, actual command/exits, environment, artifact digests, whether execution was synthetic or real, and explicit limitations. Keep source implementation, admission qualification, full packet acceptance and native activation as separate states. Historical passing suites need reruns only when their inputs or unresolved failure hypotheses change.
