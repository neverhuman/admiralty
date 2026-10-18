# Admiralty execution ledger

Planning baseline: `1e8ed17eff27ed3ab80dc6860fc8eda301406aee`. Implementation has not started. Existing dashboard/fleet services have not been changed by this planning task.

Read [MASTER_PLAN.md](MASTER_PLAN.md) and [WORK_PACKETS.md](WORK_PACKETS.md). Integrator assigns one current base/interface version and exact write scope per packet. Update this table from evidence; do not infer completion from agent messages.

| Packet | State | Owner | Integrated commit or candidate | Evidence and next action |
| --- | --- | --- | --- | --- |
| P00 | planned | unassigned | — | Pin fixtures and actual deployment inventory |
| P01 | planned | unassigned | — | Bootstrap workspace/testkit |
| P02 | planned | unassigned | — | Freeze shared domain/protocol |
| P03 | planned | unassigned | — | Implement store/replay/outbox |
| P04 | planned | unassigned | — | Implement artifacts/pins/assets |
| P05 | planned | unassigned | — | Implement durable coordination |
| P06 | planned | unassigned | — | Implement context/fidelity/governor |
| P07 | planned | unassigned | — | Implement independent session host |
| P08 | planned | unassigned | — | Qualify first provider |
| P09 | planned | unassigned | — | Implement runner/resource boundaries |
| P10 | planned | unassigned | — | Implement complete economics |
| P11 | planned | unassigned | — | Implement API/stream/auth/CLI |
| P12 | planned | unassigned | — | Build dashboard parity |
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
