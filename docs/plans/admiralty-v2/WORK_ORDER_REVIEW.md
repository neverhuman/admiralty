# Work orders, intake and delivery: review and implementation amendment

Reviewed 18 October 2026 against Admiralty source `c0ebc0935657c21adb254ff57c777c701ff10498` and the current P00–P31 plan. This amendment refines existing work; it does not replace the execution ledger or claim runtime activation.

## 1. Decision and source boundaries

The supplied “Engineering Specification: Work Order → ToDo → Work Tracking” adds useful detail to the operator journey. Admiralty already plans immutable contracts, dependencies, independent verification, dimensional progress, resource diagnosis and safe upgrades. The concrete additions are a durable request-to-task relationship, safe intake boundaries, historical duplicate discovery, optional shift profiles and an evidence-linked operator delivery view.

The attachment is descriptive source material. Its installation names, hosts, commands, nightly default, timezone, supervisor rules and owner-release procedure are not instructions for this rollout. No named source installation is accessed and no source queue is changed by this amendment. Admiralty uses configured project/node identities and the authority already granted to each operation; routine authorized work does not wait on an owner.

The preceding private todoq study examined the driver and all four requested queue snapshots. It strengthens the implementation scenarios below, particularly checked preparation, fresh-base regating, candidate preservation, interrupted accounting, partial landing and draining. Private task bodies, infrastructure paths and requester identities must not enter public fixtures or this plan.

| Source idea | Existing coverage | Concrete decision |
| --- | --- | --- |
| Self-contained work order | P02/P06/P13 contracts and capsules | Add immutable origin, rationale, source digest and request→task revision links; essential requirements stay in task context |
| One outcome per ToDo | P13 packet lint | Preview decomposition and source spans; record only explicit item boundaries |
| Multi-paragraph intake | No explicit boundary contract | A paragraph is content, never an implicit new task; structured batch input is separate |
| Search all states before adding | P13 only rejects duplicate identities | Add bounded all-state duplicate suggestions without silently merging intent |
| Night versus immediate | P14 general fairness/routing | Optional configured scheduling profile, resolved timezone/DST semantics and auditable overrides |
| Queue, gate, review, merge and release | P02/P15/P19/P25 conceptual separation | Join independent, source-bound observations into operator views, per repo and deployment target |
| Needs-you/release views | P29 generic blocker/action UX | Needs attention with responsible actor, reason, age, next action and freshness |
| Explicit worker environment | P08/P09 capability and isolation | Checked noninteractive preparation and toolchain identity are launch prerequisites |
| Resource failures resembling idle | P08/P14/P24 detection | Preserve infrastructure wait/pressure independently from agent failure; no automatic capability escalation |
| Merged driver versus installed driver | P25/P30 bundles | Show desired, installed and observed-running versions with drain/activation receipts |
| Tracking trailers | Provenance and exact-tree verification | Useful lookup hints; never acceptance, model identity or release evidence |

## 2. Objective, constraints and completion

**Objective:** a user can enter a request once, inspect its proposed tasks and duplicate candidates, record it without accidental duplication, and follow the resulting work to the delivery stage its contract requires. Admirals/Commodores see compact changed exceptions; deterministic code handles recording, indexing, readiness and routine status.

**Non-goals:** replacing SQLite with a Git queue; migrating or writing to the studied queues; launching imported setup/check commands; fixed nightly operation; a new conversational manager loop; automatic external release grants; hiding the detailed dashboard; claiming native qualification from preview tests.

**Assumptions/dependencies:** preserve the existing closed `TaskContract` v1 and immutable task identity rules unless integration deliberately versions them. The Store remains the sole writer. P05 pending delivery, P07 supervision, P09 permits, P10 accounting and P14 executor admission remain necessary for live execution. Intake can ship offline before those gates. Default policy and supported timezone data must be explicit, not inferred from a source queue.

**Completion criteria:** synthetic request→preview→record→show round-trip; lost-reply replay and conflicting replay; authorized duplicate search supporting every lifecycle state with explicit complete/partial coverage; independent outcomes with exact source spans; inert todoq import; deterministic scheduling windows; operator views preserving unknown/stale/partial facts; and one separately qualified attempt/delivery loop with exact-source receipts. Exhaustive advisory similarity scanning is not an unbounded prerequisite to recording; exact identity replay protection remains atomic. Each subpacket records its own scope; the parent packets stay open until their complete criteria pass.

**Stop conditions:** inaccessible required request evidence; ambiguous project/repository authority; changed command identity/content; unresolved essential criteria; unauthorized source traversal; malformed/oversized input; unknown toolchain/setup outcome; uncertain external effect; incompatible public schema or active bundle. Hold only the affected admission/action, preserve evidence and continue unrelated ready work. Do not turn these states into generic owner approvals.

## 3. Proposed data and behavior contracts

Names below are target contracts, not claims of existing APIs. Freeze them through WO1 before other lanes code against them.

- `WorkOrderRevision`: stable ID/revision, project scope, title/outcome, rationale, raw source artifact/digest, capture principal/time, attributed requester plus attribution trust, optional origin reference, constraints, requested timing and parser version. Keep attributed requester distinct from the authenticated submitter. Unknown attribution remains unknown.
- `WorkOrderTaskBinding`: immutable work-order revision → task ID/contract digest, outcome index and bounded source spans. One request can produce several independently testable tasks. Superseding a request creates a revision; it cannot rewrite an already admitted task's contract.
- `DeliveryRequirement`: separately versioned immutable binding to the exact task/contract and required repository/deployment target set, with required stage per target and explicit optional/not-applicable targets. Keep it outside closed `TaskContract` v1 until a deliberate schema version change. Completion requires matching evidence for every required target at the selected contract/requirement generation; unknown is not not-applicable.
- `IntakePreview`: exact raw and canonical digests, proposed outcomes/bindings, unresolved fields, duplicate candidates and source snapshot/cursor. A simple text/file input means one request, including blank lines. JSON/TOML batch arrays or explicit item records define multiple requests. Never infer split boundaries from blank lines, punctuation, copied timestamps or list formatting.
- `DuplicateCandidate`: task identity/revision, reason, lifecycle/freshness and authorized evidence. Exact external source identity and command identity support replay protection. Similar wording or content alone is advisory: identical text can be intentionally requested twice. Search all states with bounded pages/work, return continuation/incomplete coverage explicitly, and recheck exact identity under the writer transaction before record. Do not claim exhaustive similarity search from the first page.
- `SchedulingProfile`: policy revision, immediate/scheduled eligibility, IANA timezone, windows and optional shift identity/branch template. Record UTC window endpoints, local shift date, timezone database version and override provenance. Define a deterministic gap/fold policy before admission; reject an invalid/unsupported timezone. Priority does not confer authority or reserve capacity by itself.
- `DeliveryObservation`: request/task/attempt/candidate/contract bindings, source identity/digest/time, per-repo target and per-deployment target, check/review/merge/build/stage/install/running/release facts. Preserve observed versus verified issuer/trust and unknowns. Derive a bounded read projection rather than expanding one overloaded status enum.
- `AttentionItem`: deterministic reason code, scoped subject, responsible actor/role, first/last observation, freshness, evidence references and next authorized action. Deduplicate recurring observations without resetting age. Unassigned responsibility remains explicit; do not fabricate an owner or route every exception to the human.

Illustrative flow:

```text
request + origin -> bounded preview -> explicit item/contract enrichment
  -> Store CAS + command/source dedup -> immutable request and task bindings
  -> separately authorized admission -> attempt/candidate/check/review
  -> per-repo merge observations -> per-target install/running/release evidence
```

The record command binds the exact selected preview manifest, outcome/contract digests, parser/policy revisions, scope/authority generation and both expected work-order and plan revisions. Revalidate hard preconditions and current authorization in the writer turn; a changed decomposition, check profile or stale authority cannot silently reuse a prior selection. Commit the request, selected bindings, new plan revision and retry receipt atomically, or record none. A caller retry with identical identity/content returns the prior receipt. Conflicting content returns a typed collision; a lost reply cannot create a second request. Registration never implies dispatch. Imported instructions remain inert data until a separate approved contract and qualified executor admit them.

The original source must be bounded inline durable content or a verified P04 artifact with an interlocked lifetime pin before the record receipt is acknowledged. A URI plus digest alone does not establish custody. Trusted offline CLI recording uses the existing private, exclusively owned Store and records its local principal/context; it does not require a network service. Remote/API/UI mutations require authenticated, current action/scope authorization. Neither recording mode activates execution.

Capsules carry the selected outcome, rationale needed to decide, immutable constraints/criteria, ownership, named check profile and only relevant failed-attempt deltas. Large original prose and transcripts remain behind scoped content references. Emit an attention delta only when actionable state changes; healthy unchanged state consumes no manager turn.

## 4. Read first, ownership and sequencing

All paths are relative to `/Users/bentaylor/Documents/ChatGPT/admiralty`. Run commands there with `rtk`. The initial master plan includes proposed paths; the inventory below uses actual current paths. New paths are explicitly labeled. Rebase the packet assignment against the actual integration head before editing.

The integrator reads in this order. Each worker receives its assigned subpacket, applicable instructions, relevant plan sections and frozen interfaces; do not load the entire plan, private study or fleet history into every worker's context.

1. `/Users/bentaylor/.codex/RTK.md`, applicable `AGENTS.md`, `README.md`, `MASTER_PLAN.md`, `WORK_PACKETS.md`, `EXECUTION.md` and this amendment.
2. `crates/admiralty-core/src/{contracts,workplans,events,reducer,ids}.rs`; especially `TaskContract::validate`, `preview_workplan` and `preview_workplan_with_existing`.
3. `crates/admiralty-store/src/workplans.rs`, `Store` request/writer dispatch in `crates/admiralty-store/src/lib.rs`, and `docs/architecture/durable-workplans.md`.
4. `crates/admiralty-cli/src/workplans.rs` (`WorkplanCommand`, `run`), `crates/admiralty-server/src/lib.rs` (`FleetResponse`, `fleet_from`, routes), `apps/dashboard/src/{api,validation}.ts` and `src/ui/{App,LegacyDetailed,LegacyHierarchy}.tsx`.
5. `crates/admiralty-core/src/{scheduler,publication,verification}.rs`, `docs/architecture/{scheduler-prototype,provider-supervision}.md`, `tests/integration/{plan_import,scheduler_admission,exact_candidate,publication_gate,upgrade_continuity}.rs`.

**Integration-only paths:** workspace/Cargo manifests, lockfiles, crate `lib.rs` exports and Store request dispatch, CLI root command registration, server routes/public projections, schema generator/`schemas/`/`packages/contracts/`, dashboard app navigation, workflow wiring and receipt/ledger acceptance. A lane proposes wiring changes; one integrator applies them after its interface is frozen. Generated contracts are regenerated, never hand-patched. Shared files require sequential ownership transfer.

**Forbidden/read-only zones:** dirty production checkout, live queue repositories and their refs, other projects' files/services, credentials, live runtime/database/history/profile files, active Screen buffers, imported gate/setup executables, existing protected evaluator inputs and unrelated worker trees. Use synthetic fixtures; external forge read access for a future importer requires configured scope. CPU/build slots remain separately registered; no lane assumes spare capacity from an idle Screen.

| Subpacket | Existing parents | Exclusive implementation scope | Dependencies |
| --- | --- | --- | --- |
| WO1: intake contracts/preview | P02/P06/P13 | New `crates/admiralty-core/src/intake.rs`; new core `tests/intake_contracts.rs` | Interface review first |
| WO2: durable request records | P03/P13 | New `crates/admiralty-store/src/intake.rs`, next unused migration number, new Store `tests/intake_registry.rs` | Frozen WO1; integrator owns Store wiring |
| WO3: read-only compatibility parser | P13 | New `crates/admiralty-core/src/import/todoq.rs`, synthetic `tests/fixtures/todoq/` | Frozen WO1; no live queue writes |
| WO4: scheduling profile | P14/P09 | New `crates/admiralty-core/src/scheduling_profile.rs`, new core `tests/scheduling_profile.rs` | Frozen WO1 schedule contract |
| WO5: delivery projection | P19/P25/P29 | New `crates/admiralty-core/src/delivery.rs`, new core `tests/delivery_projection.rs` | Frozen WO1 IDs; existing verification/publication contracts |
| WO6: operator journey | P11/P13/P29/P30 | New CLI `src/intake.rs`; new dashboard `src/ui/{Intake,Attention,Delivery}.tsx` and named tests | Stable WO2/WO5 API; split CLI and React lanes |

WO2/WO3/WO4/WO5 may run in parallel after WO1 freeze because their write scopes are disjoint. Schema, Store wiring and route registration stay serialized with integration. WO6 can build fixture-only UI early, but cannot present fixture state as live or enable remote mutations before service authorization exists. Trusted offline commands retain the private Store ownership boundary above. No concurrent edits to `App.tsx`, `main.rs`, migrations with the same number, or testkit registration. Do not dispatch speculative duplicate builders into the same tree.

## 5. Ordered implementation packets

### WO1 — Preview with provenance and explicit boundaries

1. Define bounded closed versioned types in new `core/src/intake.rs`; validate raw bytes before parsing, UTF-8/source spans, counts, ID lengths, references and attribution trust. Start with existing workplan limits (1 MiB document/1,000 tasks) or tighter; add explicit per-field/link/diagnostic/page caps.
2. Implement pure `preview_intake(source, format, authorized_scope, policy)` with no I/O, inference, clock lookup or task execution. Preserve raw/canonical identities and one plain-text request across paragraph breaks. Only explicit batch records multiply work orders.
3. Implement deterministic missing-field/explicit-outcome diagnostics and compact capsule bindings. A bounded planner may propose semantic decomposition separately, but arbitrary prose cannot be reliably split by formatting heuristics. Keep its proposal reviewable under existing task policy; suggestions never synthesize missing authority, required checks, requester identity or dependencies.
4. Define duplicate-query, immutable record request/receipt and `DeliveryRequirement` contracts; exact identity collisions and advisory matches are distinct. Integrator reviews schema version/migration strategy and registers exports/tests.

Validate `rtk proxy cargo test -p admiralty-core --test intake_contracts --locked` after registration. Cover prose/blank lines/code fences/copied timestamps, Unicode byte spans, negations/units, missing criteria, hostile references, duplicate fields, excessive nesting, oversize/decomposition bounds and stable source digests. Artifact: frozen versioned interface and passing synthetic fixtures, not a dispatch receipt.

### WO2 — Atomic record, historical duplicate query and replay

1. Add immutable request/revision/binding rows, project-scoped origin indexes and command receipt constraints in the next unused Store migration; inspect actual migration allocation first. Retain old v1 plan/task digests and backup/rebuild compatibility.
2. Implement writer-owned `record_intake`, `get_work_order` and bounded `find_duplicate_candidates`. Match all lifecycle states within authorized projects; expose scan continuation/coverage and redact inaccessible source references. No second SQLite writer or ad hoc live connection.
3. Record selected task bindings and plan registration in one transaction with both expected work-order and plan revisions, exact selected preview/outcome/contract digests, parser/policy revisions, authority generation and canonical command/source identity. Require durable inline source or a verified/interlocked P04 source pin before receipt. Do not nest an independent registration transaction; extract a reviewed transaction-aware helper if needed. Revalidate exact duplicate identity, current authorization, hard stale assumptions and task contract closure inside that transaction. Advisory duplicate scans may remain partial with visible continuation/coverage; they do not require an unbounded history scan.
4. Prove rollback at each write seam, lost response replay, two racing identical/different commands, preview/selection tampering, stale policy/authority/request/plan CAS, absent source payload/pin, immutable history, reboot/rebuild and backup/restore. Registration produces no launch outbox or acceptance event.

Validate `rtk proxy cargo test -p admiralty-store --test intake_registry --test workplan_registry --locked`. Artifact: exact-source receipt plus raw failure/replay evidence. Failure interpretation: any partial row/binding, silent changed-content replay or inaccessible duplicate leakage blocks record activation.

### WO3 — Inert todoq family observation

1. Parse bounded family TOML and Markdown task records into observations plus proposed intake, preserving repo/ref/commit/blob/path/parser identity and source statuses. Accept a supplied pinned snapshot; transport credential/configuration is separate integration work.
2. Preserve multi-repo scope/dependency edges and unresolved references; report malformed/placeholder records without inventing criteria or an active lease from historical claimant text. Missing family-to-repository mappings remain unresolved.
3. Render setup/gate text as inert escaped content. Do not execute hooks, commands, imported instructions, queue claims, pushes, schedules or release actions. An explicit enriched contract is needed for later native admission.
4. Add synthetic fixtures for four family shapes, multiple repositories, duplicate IDs/keys, missing dependencies, cycles, changed-source replay, empty files and `done` with `merged=false` or absent acceptance evidence. Do not copy private task bodies.

Validate `rtk proxy cargo test -p admiralty-core import::todoq --locked` once integration registers the module, plus the existing `plan_import` harness. Artifact: reproducible observation/import preview with provenance and diagnostics; no migration or execution claims.

### WO4 — Scheduling windows and environment waits

1. Implement a pure resolved-window calculation over an explicit instant, versioned timezone data and frozen profile. Keep priority, eligibility, available permits and authority separate.
2. Specify deterministic DST fold/gap behavior, shift date at local window start, cross-midnight/UTC date changes, restart/replay and explicit override expiry. Record resolution rather than recomputing historical dates under newer timezone rules.
3. Integrator connects results to `propose_ready_tasks` and atomic admission; publish reason codes for waiting window, absent toolchain, failed setup, memory/disk pressure, quota and missing decisions. Setup has checked exit/timeouts and an explicit noninteractive environment identity. These failures do not silently escalate to a stronger model.

Validate `rtk proxy cargo test -p admiralty-core --test scheduling_profile --locked` and affected `scheduler_admission` fixtures. Artifact: deterministic window/eligibility decisions, no actual scheduler daemon qualification.

### WO5 — Honest delivery, running versions and attention

1. Implement bounded pure joins by exact task/contract/attempt/candidate/target identities. Keep implementation/check/review/merge and package/stage/install/running/release observations independent with provenance and freshness.
2. Require exact matching evidence for derived delivery claims; a passing gate alone cannot imply review, acceptance or release. The frozen `DeliveryRequirement` controls completion across every required target/stage. Test two targets with only one released, optional/not-applicable versus unknown, and reopened/superseded requirement generations. Treat tracking trailers as lookup hints only.
3. Preserve per-repository partial landing and uncertain remote outcomes. Bind rescue artifacts to immutable candidate identity; source main cannot overwrite unique unlanded work. Integration connects this to existing frozen publisher intents and reconciliation, never force-pushes a rescue ref by default.
4. Join desired bundle, installed bundle and observed session execution identity without replacing observed values with requested ones. Provide drain/pending update/rollback reasons; a merged package change is not evidence active sessions run it.
5. Derive `AttentionItem`s and compact actionable deltas from changed facts. Record missing toolchain/resource pressure/failed check/review wait/decision blocker/unknown external effect as distinct reasons. An unchanged observation does not reset waiting age or request another manager turn.

Validate `rtk proxy cargo test -p admiralty-core --test delivery_projection --locked` and affected `publication_gate`, `upgrade_continuity`, `integration_train` harnesses. Artifact: source-bound pure projection evidence; actual merge, installation and live drain remain separately qualified executors.

### WO6 — Simple operator journey without dashboard regressions

1. CLI lane adds proposed `intake preview`, `intake record`, `work-order show` and bounded all-state search through existing writer/authorization APIs. Registration/help never launches work; no direct DB mutations beside Store. Use structured arguments/files, not interpolated shell strings.
2. React lane builds new components against frozen generated contracts. Preview shows request boundaries, proposed outcomes, unresolved criteria/scope and historical duplicate candidates; record returns an immutable receipt. Do not make a human resolve harmless advisory matches repeatedly when current policy already authorizes the selection.
3. Add Needs attention and delivery drill-down as ordinary project/global saved views. Show reason/owner/age/freshness/evidence/next authorized action, unknown states and per-target delivery. Keep keyboard/URL navigation and source scope. Integrator wires routes and navigation after both lanes freeze.
4. Preserve the black dense live console, colored models, owner/Grok hierarchy, Admiral/Commodore terminology, temporal plots and `/legacy/`. Render imported prose safely. CLI/React tests must verify no hidden dispatch or publication occurs while previewing/recording.

Validate the existing CLI offline round-trip suite, new CLI intake round-trips, React typecheck and targeted `intake`, `attention`, `delivery`, `hierarchy` and `temporal-plots` tests plus build. Add corresponding named E2E flows only when the authorized service journey exists; fixture-only UI passes are insufficient for live acceptance.

## 6. Integration validation and handoff

Use explicit build target/private scratch and registered jobs/CPUs from current effective policy. Proposed tests above must be registered before dispatching their commands; a missing named target is incomplete implementation, never a pass. Existing checkpoint commands include:

```sh
rtk proxy env CARGO_TARGET_DIR=/Users/bentaylor/Documents/ChatGPT/admiralty/target CARGO_BUILD_JOBS=2 cargo test -p admiralty-testkit --test plan_import --test scheduler_admission --test exact_candidate --test publication_gate --test integration_train --test upgrade_continuity --locked
rtk proxy env CARGO_TARGET_DIR=/Users/bentaylor/Documents/ChatGPT/admiralty/target CARGO_BUILD_JOBS=2 cargo clippy --workspace --all-targets --locked -- -D warnings
rtk proxy pnpm --filter @admiralty/dashboard typecheck
rtk proxy pnpm --filter @admiralty/dashboard test --run
rtk proxy pnpm --filter @admiralty/dashboard build
```

Confirm package script names against `apps/dashboard/package.json` before execution. Run targeted relevant checks per lane, then integrated strict checks on the exact combined source. Keep implementation moving while CI queues; never relabel earlier source's passing evidence as validation of later edits.

Required scenarios include: multi-paragraph request stays one request; intentional explicit batch yields correct independent tasks; duplicate completed/reopened work remains discoverable; lost record response creates one record; source revision/content collision preserves evidence; missing dependency stays unresolved; fake imported check text never executes; DST/restart selects one intended window; missing PATH/setup failure blocks launch; pressure is not idle or model failure; rebase invalidates affected checks; partial landing remains partial; merged source remains distinct from installed/running/released state; pending drain preserves active child and unique candidate work.

Every lane hands off base/head full OIDs, exact owned paths, frozen interface version, argv/cwd/toolchain/exits, output/artifact digests, negative evidence, limitations and next ready action. Save engineering receipts under `receipts/<parent>/<actual-commit>.json`, with the subpacket ID in scope; raw output stays in private/ignored artifacts. Update the parent's ledger note without declaring full acceptance. Integration independently reviews the combined tree and records the resulting source revision. Only separately authorized, evidenced executor/rollout gates activate native tasks or publication.
