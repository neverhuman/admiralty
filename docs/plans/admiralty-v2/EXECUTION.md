# Admiralty execution ledger

Planning baseline: `1e8ed17eff27ed3ab80dc6860fc8eda301406aee`. Implementation started on 18 October 2026. PR #9 carries the integration branch. A tested read-only rollout panel is running on the existing dashboard as an immutable PR canary; only its exact collector child was restarted. Existing coding projects retain their lanes and ownership. Native v2 execution remains disabled pending qualification.

Read [MASTER_PLAN.md](MASTER_PLAN.md) and [WORK_PACKETS.md](WORK_PACKETS.md). Integrator assigns one current base/interface version and exact write scope per packet. Update this table from evidence; do not infer completion from agent messages.

| Packet | State | Owner | Integrated commit or candidate | Evidence and next action |
| --- | --- | --- | --- | --- |
| P00 | review | local evidence agent | candidate, not integrated | 13 legacy tests and 37-source hash verifier pass; integration receipt pending |
| P01 | in_progress | local integrator + remote fixture lane | candidate, not integrated | Six crates, pinned tools, CI and fake-process lane; native qualification pending |
| P02 | in_progress | local integrator | candidate, not integrated | Typed contracts/reducer/schema/TS pipeline with core scope tests; complete policy model pending |
| P03 | in_progress | local store agent | candidate, not integrated | Single-owner WAL writer, replay/collision/ACK/backup tests; outbox and full recovery pending |
| P04 | planned | unassigned | — | Implement artifacts/pins/assets |
| P05 | in_progress | local coordination team | candidate, not integrated | Durable scoped messages/ACK; full subscriptions/question lifecycle pending |
| P06 | in_progress | local context team | candidate, not integrated | Constraint-preserving capsules and epoch checks; full governor/retrieval pending |
| P07 | claimed | remote captain session lane | candidate, not integrated | Bounded headless prototype dispatched; remains unqualified |
| P08 | planned | unassigned | — | Qualify first provider |
| P09 | planned | unassigned | — | Implement runner/resource boundaries |
| P10 | planned | unassigned | — | Implement complete economics |
| P11 | in_progress | local API team | candidate, not integrated | Read-only authenticated API/SSE, generation/precision/security tests; commands pending |
| P12 | claimed | remote captain UI lane | candidate, not integrated | React/Vite build dispatched in isolated scope; legacy progress bridge already live |
| P13 | planned | unassigned | — | Implement workplan import/lint |
| P14 | planned | unassigned | — | Implement scheduler/recovery |
| P15 | planned | unassigned | — | Implement trusted exact verifier |
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
| P30 | planned | unassigned | — | Complete packaging/brand/docs |
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
