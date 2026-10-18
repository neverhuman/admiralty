# Admiralty execution ledger

Planning baseline: `1e8ed17eff27ed3ab80dc6860fc8eda301406aee`. Implementation started on 18 October 2026. PR #9 carries the integration branch. A tested read-only rollout panel is running on the existing dashboard as an immutable PR canary; only its exact collector child was restarted. Existing coding projects retain their lanes and ownership. Native v2 execution remains disabled pending qualification.

Read [MASTER_PLAN.md](MASTER_PLAN.md) and [WORK_PACKETS.md](WORK_PACKETS.md). Integrator assigns one current base/interface version and exact write scope per packet. Update this table from evidence; do not infer completion from agent messages.

| Packet | State | Owner | Integrated commit or candidate | Evidence and next action |
| --- | --- | --- | --- | --- |
| P00 | review | local evidence agent | candidate, not integrated | 13 legacy tests and 37-source hash verifier pass; integration receipt pending |
| P01 | in_progress | local integrator + remote fixture lane | candidate, not integrated | Six crates, pinned tools, CI and fake-process lane; native qualification pending |
| P02 | in_progress | local integrator | candidate, not integrated | Typed contracts/reducer/schema/TS pipeline with core scope tests; complete policy model pending |
| P03 | in_progress | local store agent | PR prototype | Single-owner WAL writer, replay/collision/ACK/backup; fenced durable outbox initialized on startup, atomic domain/effect admission still pending |
| P04 | in_progress | remote artifact lane | scoped candidate pending | P04A bounded file publication; transactional pins/assets remain pending |
| P05 | in_progress | local coordination team | question candidate `9655f63`, imported `14d4810` | Durable scoped messages/ACK plus independently reviewed question reducer; durable question service/subscriptions still pending |
| P06 | in_progress | local context team | PR prototype | Constraint-preserving capsules, epochs and bounded attention governor; durable governor/outbox activation and retrieval pending |
| P07 | in_progress | remote session lane + local reviewer | candidate `2d9cc760`, imported as `5a0f639` | Pipe/journal prototype under boundedness review; restart adoption and independent custody unqualified |
| P08 | planned | unassigned | — | Qualify first provider |
| P09 | in_progress | remote workspace lane + local reviewer | candidate `f0d15a82`, imported `d637bfe` | Read-only custody registration; real runner/resource isolation pending |
| P10 | in_progress | remote usage lane | scoped candidate pending | P10A segmented cumulative usage accumulator; prices/reservations remain pending |
| P11 | in_progress | local API team | candidate, not integrated | Read-only authenticated API/SSE, generation/precision/security tests; commands pending |
| P12 | in_progress | remote UI lane + local refinement | candidate `ae57d6d`, imported as `6a62db2` | React/Vite shell, single SSE stream, runtime guards and branding; read-only parity/qualification still pending |
| P13 | in_progress | remote workplan lane + local reviewer | candidate `d54ea23a`, imported `d0275c5` | Bounded dependency preview and CLI entry point; durable plan submission/revision service pending |
| P14 | in_progress | remote scheduler lane | scoped candidate pending | Pure scheduling proposal capsule dispatched; recovery and real launch not implemented |
| P15 | in_progress | remote verifier lane | scoped candidate pending | Exact receipt validation capsule dispatched; trusted execution/verifier custody pending |
| P16 | planned | unassigned | — | Qualify Jankurai/staged audit |
| P17 | planned | unassigned | — | Implement peer coordination/tools |
| P18 | planned | unassigned | — | Implement conflict/train flow |
| P19 | planned | unassigned | — | Implement exclusive publisher |
| P20 | planned | unassigned | — | Implement safe retirement |
| P21 | planned | unassigned | — | Qualify second provider |
| P22 | planned | unassigned | — | Qualify remaining current pathways |
| P23 | planned | unassigned | — | Implement node enrollment/replay |
| P24 | planned | unassigned | — | Qualify fleet placement/partitions |
| P25 | planned | unassigned | — | Qualify upgrades/restore |
| P26 | planned | unassigned | — | Build economic/overhead explorer |
| P27 | planned | unassigned | — | Implement excursion/proposal loop |
| P28 | planned | unassigned | — | Implement independent trials |
| P29 | planned | unassigned | — | Finish operational UX |
| P30 | in_progress | local integrator | source preview | Source launchers, logo, docs and separate live/demo ports; release bundles/installers pending |
| P31 | planned | unassigned | — | Qualify and roll out by gates |

## Receipt template

Save at `receipts/<packet>/<actual-commit>.json`; do not create an example file with invented commit hashes. Large raw output belongs in artifacts, not this table.

```json
{
  "packet": "P00",
  "state": "review",
  "owner": "assigned principal",
  "base_sha": "computed full OID",
  "head_sha": "computed full OID",
  "contract_version": "actual version",
  "changed_paths": [],
  "criteria": [{"name": "criterion", "result": "pass|fail|not_run", "evidence": []}],
  "validation": [{"argv": [], "cwd": "actual path", "exit_code": 0, "artifact": "computed reference"}],
  "environment": {},
  "decisions": [],
  "limitations": [],
  "blockers": [],
  "next_action": "bounded action"
}
```

The receipt is engineering evidence, not product acceptance. Integrator verifies actual resulting tree/tests and records the integrated SHA. Reconcile planning estimates after P08/P15 establish real provider/verifier throughput; preserve earlier estimates as history rather than silently changing a committed delivery promise.

## Live build visibility

The existing legacy dashboard has a generic rollout summary and expanded agent
table. Rust and React now serve real producer observations on loopback18891;
the original dashboard stays available on18890 and under `/legacy/`. React assets
are pinned to `8474dec9`, the running Rust binary to `7277293f`, and the legacy
worker projection to `bd33d23a`. These are unmerged PR canaries. The default page
is the detailed real fleet; synthetic demo now defaults to18892.

A bounded observer joins captain reports into runtime JSON every15 seconds
without model turns. It independently checks owned `adm_` Screen identities and
bounded candidate-path/HEAD metadata; producer report age is preserved. The
collector consumes that projection on its next cycle. Three actual GNU Screens
run Luna with enforced CPU pairs104–109 in isolated worktrees. Startup trust
prompts were independently observed and cleared for this checkout before edits
began. The first three candidates are imported for independent review; a second
three-packet batch is building scheduler, verifier and config components. Process
presence, file edits and passing candidate tests never complete parent packets.

Current native read path: max 16 SSE bodies, max 8 concurrent reads; a coalesced
writer notification wakes subscribers, which then re-read durable cursor pages.
These bounds and offline tests do not constitute fleet-scale performance or
paid-provider qualification. Existing project scheduling/authority remains in
the legacy system until the planned qualification gates pass.

Prototype integration evidence at `109563722a4c03941b18e79d180a31c9225369fb`: 95 Rust tests and strict workspace Clippy pass. Partial receipts under `receipts/P02`, `P03`, `P05`, `P06`, `P07` and `P11` name the tested code commit and remaining acceptance gaps.


## Owner steering: detailed UI, own models and more parallel workers

React now preserves detailed legacy project cards and side panels and treats
Admiralty as an ordinary project tab. A configured original-page fallback stays
available. Private model configuration supports own-model tiers below Luna,
provider-native identity, caps and explicit escalation, with setup/input failures
waiting for repair. One corrected Ornith/OpenCode coding task passed8 independent
methods and80 seeded cases in30.039s; its first attempt is retained as a checkout
setup/permission failure. This qualifies that observed helper tuple, not the
entire native managed execution/recovery path or all model task classes.

The remote captain has moved the next workplan, question-lifecycle and workspace
packets to actual GNU Screen+taskset lanes in its reserved CPU scope. The prior
artifact-file and segmented-usage candidates have been imported for review.
Additional independent local workers implement a captain attention governor and
an outbox component while the UI/model/provider integration is deployed. Parent
packets remain partial; no full-fleet cutover or merge gate is bypassed.


## Dense console restoration and expanded execution

At code `617e9127`,184 Rust tests, strict workspace Clippy and formatting pass.
Partial component receipts name the tested commit and keep parent acceptance
false. UI code `8474dec9` restores the black fleet console, bright model badges,
compact hierarchy, model-mix and usage tables, plus all13 expandable temporal
metrics.65 React tests, typecheck and production build pass; this exact asset
revision is deployed on18891 and verified against real producer history.
The user explicitly requested aggressive Admiralty-first Luna parallelism; an
expanded pool is being admitted within the existing worker CPU reservation.
Record actual starts in the private lane manifest before describing workers as
running. Strong integration review and existing CI remain in effect.
