# Remaining Admiralty work — 19 October 2026

**Audited source:** `b2b9e7a6c33bd57fd7b56079a846482f7ea25964`

**Parent state:** 32 packets; P00 and P01 accepted at their foundation scope; P02–P31 remain open.

**Purpose:** source-pinned remaining-work audit and proposed worker handoffs, not an implementation assignment or acceptance receipt.

## Objective, boundaries and assumptions

Identify the concrete unmet parent criteria after the recent native composition work, and define small next steps that can proceed without competing writers. Preserve the distinction between an implemented component, a qualified runtime capability, an installed authority and full packet acceptance.

Non-goals are new implementation, test execution, provider calls, live fleet inspection, deployment, cleanup, status promotion, changing the master plan or granting execution/publication authority. No tests or provider calls were made for this audit. Documentation checks do not qualify a runtime capability.

The audit assumes the completion contract in [MASTER_PLAN.md](MASTER_PLAN.md), the criteria in [WORK_PACKETS.md](WORK_PACKETS.md), and the amendments in [WORK_ORDER_REVIEW.md](WORK_ORDER_REVIEW.md). [EXECUTION.md](EXECUTION.md) and exact source/receipts take precedence over a component count or a worker's completion message. A passing shared wave receipt does not establish every criterion of every packet it mentions.

All implementation claims below refer to the audited commit, even if main subsequently advances. At this planning handoff, PR85/PR86 follow-ups are outside that source boundary; re-resolve their actual heads, reviews, tests and merge state separately. No result or acceptance from those pending follow-ups is inferred here. The existing root-owned `native_codex_plan.rs` work is also separate from this audit baseline.

## Audit method and current release boundary

The read-only review used the exact checked-out source, public packet/integration receipts, architecture notes and the latest execution ledger present at the audited commit. It inspected actual Store dispatch/startup registration, runtime interfaces and frontend source rather than inferring implementation from proposed filenames. Historical receipt limitations were reconciled against newer source: an old receipt saying an interface was absent is not repeated after that interface has landed.

The Rust web/React and Rust observer cutover is **demonstrated done**, as recorded in the audited ledger: actual daemon/observer paths, release-manifest artifacts, primary HTTP service, compatibility redirect, detailed real-data view and thirteen temporal controls were checked. This document does not make a new live observation or claim the audited Git head is the currently deployed revision. The owned retired Python producer jobs are distinct from shared fleet processes and other projects.

General native coding-provider qualification is **still open**. [Protected startup](../../../crates/admiralty-server/src/startup.rs) explicitly installs task intake/pending-operation and inbox authority, not provider execution or verifier acceptance authority. The [actual Linux task-loop receipt](../../../receipts/integration/native-task-loop-linux-20261019T0053Z.json) proves a deterministic child, source materialization, real checks, wrong-oracle exit 23, effect ACK, Store acceptance/replay and an empty owned slice afterward. It explicitly does not qualify a coding provider, arbitrary CPU assignments, production startup authority, cross-restart continuity or a parent packet.

Recent work that must not be described as wholly missing includes exact-key dispatch; original opaque workspace/closure retention; all five capsule compilers; durable role renders and baseline CAS; authenticated native intake/inbox operations; dense real-data dashboard parity; and actual browser journeys. Those improvements narrow the remaining work without closing its parent contracts.

## Thirty open parent packets

Every row is open. The final column identifies concrete unfinished acceptance work; it is not an exhaustive implementation specification for that parent.

| Packet | Existing component evidence at the audited source | Concrete unmet criterion |
| --- | --- | --- |
| P02 | [Lifecycle/event contracts](../../../crates/admiralty-core/src/events.rs), configuration and generated schema/TS/runtime guards | Freeze WO1 request/provenance/outcome/delivery contracts before dependent lanes implement them. |
| P03 | [Store sole writer, replay and atomic outbox](../../../crates/admiralty-store/src/lib.rs), including exact-key claims | Complete writer-depth/transaction/WAL health, bounded-growth qualification and durable usage-fact admission; task-loop proof is narrower than the complete crash matrix. |
| P04 | [Asset/pin interlock](../../../crates/admiralty-store/src/custody.rs) and [content-addressed objects](../../../crates/admiralty-store/src/artifacts.rs) | Integrate trusted filesystem observations and availability/pins through Store; scoped ranges, orphan/missing reconciliation and storage reserve remain. |
| P05 | [Scoped durable inbox](../../../crates/admiralty-store/src/inbox.rs), authenticated agent read/application ACK and question-journal component | Wire durable subscriptions/question/decision service, delivery executor, priorities/coalescing and attention coupling. |
| P06 | [Role compilers](../../../crates/admiralty-core/src/coordination/context.rs), [durable exact renders/baseline CAS](../../../crates/admiralty-store/src/context_renders.rs) | Persist/replay governor decisions; connect authenticated safe-turn delivery, structured tool/failed-approach memory, retrieval and role/purpose accounting. |
| P07 | [Session daemon](../../../crates/admiralty-runtime/src/session/daemon.rs), raw journals and input fencing | Qualify coordinator/node restart continuity through installed independent supervision; daemon restart adoption and advertised PTY path remain unsupported. |
| P08 | [Codex preparation/parser](../../../crates/admiralty-runtime/src/providers/codex.rs) and [controlled transport harness](../../../docs/operators/codex-qualification.md) | Complete a genuinely admitted coding-provider task through current executor, candidate, independent checks and acceptance; safe-fixture conformance is narrower. |
| P09 | [Resource gate](../../../crates/admiralty-runtime/src/resource_gate.rs), managed preparation and bounded source candidates | Qualify rootless isolation, mounts/network/credentials, disk/GPU enforcement, caches and arbitrary/exclusive CPU reservations beyond the fixed-slice proof. |
| P10 | [Usage/economics](../../../crates/admiralty-core/src/economics.rs), [reservation conservation](../../../crates/admiralty-runtime/src/reservations.rs) and model settlement proposals | Implement durable usage/correction ledger and atomic reserve→dispatch→settle composition with real ingestion and cohort queries. |
| P11 | [Protected startup](../../../crates/admiralty-server/src/startup.rs), native commands and v2 snapshot/SSE | Complete authorized terminal WS/input lease, scoped tools and durable action/job coverage; startup deliberately grants no general execution/verifier authority. |
| P12 | Dense live React console, thirteen plots and actual browser journeys | [Legacy watcher](../../../apps/dashboard/src/legacy.ts) still fetches whole snapshots every 15 seconds; complete streaming/delta bridge and declared performance evidence. |
| P13 | [Immutable workplan registry](../../../crates/admiralty-store/src/workplans.rs), native task intake and observer adapters | Implement WO1–WO3 source-bound request records, historical duplicates and inert pinned todoq import, plus import/revision/ambiguity UX. |
| P14 | [Scoped NativeTaskLoop](../../../crates/admiralty-server/src/services/native_task_loop.rs), claim/outbox co-commit and model proposals | Compose authoritative budget/resource/admission CAS; weighted fairness/finish mode and timezone/DST eligibility remain. |
| P15 | [Bounded source candidates](../../../crates/admiralty-runtime/src/workspace/source_candidate.rs), actual checks and deterministic Linux acceptance | Install current independently authenticated verifier/custody policy, reserved verification resources and generic protected-evaluator/change handling; PR85 is separate pending work. |
| P16 | [Staged-audit predicates](../../../crates/admiralty-runtime/src/audits.rs) and negative fixtures | Implement/qualify actual Jankurai/profile execution, exact-index materialization, hook preservation and enrollment/ratchet path. |
| P17 | [Interface reducer](../../../crates/admiralty-core/src/interfaces.rs) and narrow agent services | Persist interface/claim invalidation; qualify actual safe-turn peer delivery, MCP/CLI fallback and producer→consumer handoff without manager relay. |
| P18 | [Conflict/hunk/common-base analysis](../../../crates/admiralty-core/src/conflicts.rs) and source-candidate hunk extraction | Add real merge probes, durable exclusive train writer, combined checks, culprit isolation and changed-base proof invalidation. |
| P19 | [Publication predicates/reducer](../../../crates/admiralty-core/src/publication.rs) | Add credential-isolated executor, durable unresolved target slot and actual forge lost-response reconciliation; preserve per-target landing/release observations. |
| P20 | [Retirement state machine](../../../crates/admiralty-core/src/retirement.rs) and durable pin claims | Build registered-asset preview→recheck→quarantine/delete executor with actual identity/unique-work/crash protection. Manual cleanup is not that product feature. |
| P21 | [Claude adapter](../../../crates/admiralty-runtime/src/providers/claude.rs) and parser fixtures | Qualify current admitted launch/input/resume continuity and native nested usage with a bounded real receipt. |
| P22 | Existing [Gemini](../../../crates/admiralty-runtime/src/providers/agy.rs), [Grok](../../../crates/admiralty-runtime/src/providers/grok.rs), process/OpenCode pathways and legacy observation | Qualify each advertised native executable/version/transport capability through admission; legacy observations remain separate from grants. |
| P23 | Node/enrollment contracts and transport fixtures | [Node executable](../../../crates/admiralty-runtime/src/bin/admiralty-node.rs) explicitly disables enrollment; persistent keys/certificates, mTLS and resumable transfer service remain. |
| P24 | [Freshness-aware placement proposals](../../../crates/admiralty-core/src/placement.rs) | Activate authenticated complete topology, reserved dispatch and actual partition/reboot/reconnect reconciliation. |
| P25 | [Native backup/recovery rehearsal](../../../crates/admiralty-cli/src/native_recovery.rs) and release staging | Complete signed bundles, live session-preserving upgrade/handoff and independently verified old-executor fencing before restored effects. |
| P26 | [Real producer token/cost views](../../../apps/dashboard/src/ui/TokenEfficiency.tsx) | Build native role/purpose/cohort/maturity explorer, harness-floor/rotation outcomes, dimensional definitions and bounded export. |
| P27 | [Pure excursion lineage/proposals](../../../crates/admiralty-core/src/excursions.rs) | Persist observation→bounded investigation→sanitized proposal lifecycle, scoped retrieval and incident UI. |
| P28 | [Pure trial assignment/decision models](../../../crates/admiralty-core/src/trials.rs) | Add immutable bundle/dataset registry, controlled execution, independent promotion/rollback and reproducible exports/UI. |
| P29 | [Actual native command journeys](../../../apps/dashboard/src/ui/CommandJourneys.tsx) and dense evidence views | Complete train/quality/assets/enrollment/drain/update actions and WO5/WO6 Needs-attention/delivery journeys. |
| P30 | [Native package staging](../../../crates/admiralty-cli/src/native_package.rs), improved README/demo/doctor | Prove clean-machine install and supported-target provenance/signing/SBOM; complete guides and settle code/data/logo rights and license before public distribution. |
| P31 | [Actual Linux task-loop proof](../../../receipts/integration/native-task-loop-linux-20261019T0053Z.json) | Qualify coding providers, restart/fleet continuity, 24-hour soak, performance, prospective complete-cost trial, restore/rollback and staged native takeover. |

## Proposed next worker packets

These are proposed handoffs, **not new assignments**. Root must acknowledge scope, owner, base and registered CPU/build limits before dispatch. In particular, new `intake.rs` has no implementation ownership claim from this document. Do not start a competing writer for the already-owned native provider plan.

The three implementation scopes below are disjoint. Root serializes shared registration after each interface freezes: crate exports, Store `Request`/writer dispatch and migration numbering, schemas/generated contracts, server startup/routes, CLI/App wiring and receipt/ledger acceptance. No worker edits those shared paths without an explicit ownership transfer. A packet depending on an unfinished interface may implement bounded inert components, but cannot claim activation.

### A — Complete the owned native provider plan and prove its composition

**Objective:** prepare the existing native executor's exact immutable plan from the existing observed Codex adapter, then integrate one policy-authorized coding task through the retained native lifecycle. Parents P08/P09/P14/P15; this is not their blanket acceptance.

**Ownership/dependencies:** the existing runtime owner keeps new `crates/admiralty-server/src/services/native_codex_plan.rs` and its module-local tests. Root owns later registration/startup integration and qualification. Read the actual assignment before editing: this file is already being implemented outside the audit baseline. Depends on current resource/lease/reservation authority, registered workspace custody, scoped NativeTaskLoop and repaired/registered PR85 authority. Missing authority is a named blocker, never a caller JSON grant.

**Read first, in order:**

1. Applicable instructions, [execution ledger](EXECUTION.md), [P08/P14/P15 criteria](WORK_PACKETS.md), current assignment and actual main.
2. [Codex observation/preparation API](../../../crates/admiralty-runtime/src/providers/codex.rs): `CodexCliObservation`, `prepare_launch`, `PreparedCodexLaunch` and `CodexLaunchTuple`.
3. [Native executor](../../../crates/admiralty-server/src/services/native_executor.rs): `NativeExecutionPlan`, `try_register_plan`, `PlanRegistrationFailure`, `ClosedExecution`; [NativeTaskLoop](../../../crates/admiralty-server/src/services/native_task_loop.rs).
4. [Resource gate](../../../crates/admiralty-runtime/src/resource_gate.rs), [workspace preparation](../../../crates/admiralty-runtime/src/workspace/preparation.rs) and [source-candidate custody](../../../crates/admiralty-runtime/src/workspace/source_candidate.rs).
5. [Native lifecycle authority](../../../crates/admiralty-server/src/services/native_lifecycle.rs), actual repaired PR85 source and the [Linux task-loop receipt](../../../receipts/integration/native-task-loop-linux-20261019T0053Z.json).

**Non-goals/forbidden writes:** no new CLI dialect/parser, weak path-only custody, permissive authority, direct Store append bypass, provider fallback, general startup activation, fleet cutover or publication. Do not edit S3 catalog, S2 browser files, shared resource/custody modules or other workers' runtime files. Preserve dirty production checkouts, private configuration and journals.

**Ordered steps and hard edges:**

1. Finish the owned adapter to consume actual `PreparedWorkspace` and current task/attempt/contract/context/policy bindings. Use existing observed executable/help/configuration and explicit model/effort; missing observed model metadata remains unknown.
2. Reuse `prepare_launch`; derive exact `LaunchSpec`, environment and digests for `NativeExecutionPlan`. Bind node, lease, reservation and permit identities; retain original opaque capabilities on every rejection. Credentials come from the configured protected startup boundary, not prompt/argv/serialized trust flags.
3. Keep registration, claim and exact-key dispatch idempotent. Rejected pre-spawn preparation has no child; uncertain journal/launch outcomes retain their plan, reservation and ownership. Do not relaunch after elapsed time or an idle Screen.
4. Through root's separate integration, consume actual acknowledged closure and original prepared workspace. Use ownership-returning source-candidate APIs, actual independent checks and repaired catalog authentication before a trusted lifecycle decision. Revalidate at effect/grant boundaries; an EOF or successful provider exit is only evidence of execution.
5. Freeze the component for independent review. Root then performs one fresh, bounded, explicitly configured real coding task and retains denied/revoked/duplicate/reopen evidence. The deterministic probe is a prerequisite, not this qualification result.

**Validation/artifacts:** after root registers the actual module, run the nonzero `native_codex_plan::tests` selection, affected native executor/task-loop tests, strict server Clippy and formatting. Tests must cover mismatched contract/task/attempt/provider/model/config/environment, stale/revoked permit, bounded/escaped prompts, original custody returned on failure, no effect on denial, exact retry and unknown closure. A zero-test filter or fake-provider-only pass is incomplete. Root's live receipt must bind exact source/archive/binary, observed provider tuple, authorization, real runtime identity, candidate/check objects, Store transitions and cleanup; no secrets or provider transcripts in Git.

**Completion/stop:** complete this component only when its owned source, registered tests and independent review pass. Keep general qualification false until the real evidence and startup policy support the advertised scope. Stop affected effects on missing current authority, resource ownership, executable drift, ambiguous launch/ACK/closure, incomplete capture, unsafe custody or changed protected oracle; preserve work and continue unrelated safe packets.

### B — WO1 bounded request contracts and deterministic preview

**Objective:** supply the missing frozen request/provenance/outcome/duplicate/delivery interface needed by downstream intake lanes. Parents P02/P06/P13. Root acknowledgement is required before ownership is assigned.

**Exclusive proposed writes:** new `crates/admiralty-core/src/intake.rs` and new `crates/admiralty-core/tests/intake_contracts.rs`. Root alone registers exports/schema roots and regenerated outputs. No Store, server, CLI, UI, manifests or generated-file edits. Proposed paths are not evidence of implementation.

**Dependencies/non-goals:** existing validated task/workplan types and bounded serialization; freeze the interface before WO2/WO3/WO4/WO5 start implementation. No model calls, prose inference, automatic paragraph splitting, source-queue writes, admission or acceptance. Historical duplicate search is a request/result contract here, not an invented complete database scan.

**Read first, in order:** [WO1 and its preserved boundaries](WORK_ORDER_REVIEW.md), [master §§5/6/8.1](MASTER_PLAN.md), [contracts](../../../crates/admiralty-core/src/contracts.rs), [workplan preview](../../../crates/admiralty-core/src/workplans.rs), [IDs](../../../crates/admiralty-core/src/ids.rs), [bounds](../../../crates/admiralty-core/src/limits.rs), [configuration/provenance conventions](../../../crates/admiralty-core/src/config.rs) and existing schema/contract test registration. Use actual current types; do not create a second competing task codec.

**Ordered steps and hard edges:**

1. Freeze closed versioned types for bounded source identity/payload, UTF-8 byte spans, explicit request records, proposed outcomes, unresolved fields, duplicate candidates, selected record bindings and `DeliveryRequirement`. Separate attribution claims from independently supplied principal/scope. Start at no more than the existing 1 MiB / 1,000-task workplan bounds, with tighter per-field/item/depth/page caps.
2. Implement pure `preview_intake(source, format, authorized_scope, policy)` or the root-approved equivalent. Validate raw size/depth/encoding before decoding; stream-bound typed inputs before cloning/hashing/serializing. Preserve exact raw and canonical identities with distinct hash domains.
3. Keep one plain-text request across paragraphs, blank lines, copied timestamps and code fences. Only an explicit batch format creates multiple work-order boundaries. Return exactly one outcome or explicit unresolved diagnostic per proposed task; do not invent criteria, dependencies, authority or requester identity.
4. Preserve immutable constraints, negations, units, named checks, revisions and original spans for later capsules. Advisory duplicate similarity is separate from exact identity/collision. Missing/inaccessible scope stays unresolved.
5. Obtain independent interface review; root registers the production module, generated schema/runtime consumers and actual test target. Handoff the frozen API/limits/examples to downstream lanes without dispatching work or changing parent status.

**Validation/artifacts:** after actual registration, run `rtk proxy cargo test -p admiralty-core --test intake_contracts --locked`, affected contract/schema roundtrips, strict core Clippy and formatting. Require a nonzero test result. Cover Unicode byte offsets, multiline prose/code fences, copied metadata, explicit batches, empty/duplicate/unknown fields, excessive nesting/size/counts, hostile references, missing criteria/scope, stable digests and protected-fact mutations. Old v1 bytes/semantics must remain closed and compatible unless a separately reviewed protocol version changes them.

**Completion/stop:** a reviewed immutable interface, bounded deterministic preview and registered passing fixtures complete WO1 only. No dispatch, durable record or native qualification is claimed. Stop on accidental task multiplication, authority derived from source text, rewritten negation/units, silent loss of source provenance, unbounded allocation or old-contract drift.

### C — Durable attention decisions on Store's sole writer

**Objective:** preserve accepted attention input, consumed budgets and requested deliveries across replay/restart, so compact coordination can be activated later without forgetting a wake or creating another one. Parents P05/P06; provider delivery remains separate.

**Exclusive proposed writes:** new `crates/admiralty-store/src/attention.rs`, dedicated module-local and/or new `crates/admiralty-store/tests/attention_journal.rs` tests, and one root-allocated dedicated migration. Root alone changes Store exports, `Request`/public methods/writer state/init/match registration. Root reserves the migration number first; do not assume a number remains free as main advances. Do not edit core governor, context compiler, other Store modules, server routes or provider code without a separate scope transfer.

**Read first, in order:** [master §§5.2–5.11/6](MASTER_PLAN.md), [context-governor replay requirements](../../../docs/architecture/context-governor.md), [AttentionGovernor](../../../crates/admiralty-core/src/governor.rs), [Store writer](../../../crates/admiralty-store/src/lib.rs), [context-render persistence](../../../crates/admiralty-store/src/context_renders.rs), [inbox](../../../crates/admiralty-store/src/inbox.rs), [outbox](../../../crates/admiralty-store/src/outbox.rs) and [atomic effect rules](../../../docs/architecture/atomic-effects.md).

**Dependencies/non-goals:** freeze a narrow writer request/API with root before coding. Reuse existing pure governor and exact context-render references; source artifact availability/pins and authenticated delivery activation remain independent prerequisites. No second SQLite writer, filesystem/network I/O in authority callbacks, hidden model turn, provider injection, reset-to-forget-history, implicit ACK or token-savings claim.

**Ordered steps and hard edges:**

1. Define bounded commands/receipts for immutable governor configuration/scope/epoch, accepted `AttentionUpdate` and recorded logical tick, and explicit delivery requests. Independent caller scope is required; serialized authority labels are data. Bind every immutable command field, including configuration digest and expected prior state.
2. Append accepted updates and delivery requests through Store's existing sole writer with exact source idempotency/collision checks. Persist the ordering/configuration/ticks needed to reconstruct consumed budget. Input-only replay is insufficient because `take_delivery` changes attention state.
3. Produce bounded `Quiet`/`Deferred`/`Wake` proposal receipts. Where a wake gets an outbox intent, use the same transaction rather than nested writer transactions or a later best-effort insert. A stored wake is not transport/provider/application delivery.
4. Preserve exact historical replay without restoring an old budget/baseline or issuing another intent. Retain ambiguous delivery, critical transitions and underlying source references; queue coalescing cannot erase the corresponding evidence history. Unknown/backwards clock and capacity exhaustion fail closed without eviction.
5. Reconstruct/validate the journal at reopen and rebuild. Validate stored variable-length columns before allocation and verify hashes/sequence/projection consistency. Apply the same integrity gate before backup publication. Root wires the frozen production module; later service integration authenticates actions and performs safe-turn delivery.

**Validation/artifacts:** register the real module/writer methods before claiming results. Run the new nonzero `attention_journal` target (if used), affected Store/inbox/outbox/context tests, `coordination_delivery`, strict Store Clippy and formatting. Use actual Store APIs to prove consumed-budget restart replay, exact lost-reply retry, historical replay after later decisions, concurrent CAS, late SQL-trigger rollback of both decision and intent, storm coalescing, independent recipient reserves, unknown delivery, malformed/oversized stored data and capacity without eviction. Record any fixture-only coverage explicitly.

**Completion/stop:** complete the durable component when registered actual Store tests and independent review pass. Live governor/delivery and P06 acceptance remain open. Stop on duplicate wake/effect, cross-scope disclosure, budget reset, lost critical transition, clock substitution, half-committed decision/outbox or corrupt acknowledged history.

## Validation environment, receipts and handoff

Run only affected checks after source changes; do not replay unchanged full workspace/browser suites solely to increase a count. Root performs broader exact-source integration checks after candidate import. Every proposed named target/filter above must actually exist and execute nonzero cases; missing targets, all-filtered tests and ignored helpers are not passes.

Before running commands, root assigns a canonical owner-only scratch directory, an owned build target and CPU/jobs limits. Workers use task-specific variables such as `admiralty_packet_tmp`, `admiralty_packet_target` and `admiralty_packet_jobs`; no empty/default path or idle Screen is a capacity grant. Prefix shell commands with `rtk`, for example:

```sh
rtk proxy env TMPDIR="$admiralty_packet_tmp" CARGO_TARGET_DIR="$admiralty_packet_target" CARGO_BUILD_JOBS="$admiralty_packet_jobs" cargo test -p admiralty-core --test intake_contracts --locked
rtk proxy env TMPDIR="$admiralty_packet_tmp" CARGO_TARGET_DIR="$admiralty_packet_target" CARGO_BUILD_JOBS="$admiralty_packet_jobs" cargo clippy -p admiralty-core --all-targets --locked -- -D warnings
rtk proxy cargo fmt --all -- --check
rtk proxy git diff --check
```

Commands are future validation instructions, not results from this audit. Use the appropriate owned package/target for packets A/C; do not run B's test merely because it appears in the example.

Every handoff includes exact base/head and owned-file list/hashes; source/object provenance; actual command, environment, nonzero passed/failed/ignored/filtered counts; raw log path/hash; retained failures and repair rationale; independent review; unresolved custody/authority/activation gaps; and the next ready action. Public receipts omit credentials, private source bodies, runtime configuration and transcripts. Preserve unique branches/worktrees and opaque handles until the receiving owner confirms custody.

Workers report component completion separately from runtime admission, installed activation and parent acceptance. Root alone reconciles these handoffs into [EXECUTION.md](EXECUTION.md), authoritative rollout status and later deployment evidence. This document changes none of those states.
