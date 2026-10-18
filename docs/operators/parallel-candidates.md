# Bounded parallel candidates and synthesis

`admiralty-runtime::strategies` proposes an optional best-of workflow. It is pure:
no agent starts, capacity lease, directory creation, shared write, publication or
acceptance occurs. Single-candidate execution remains available independently.
The operator chooses this strategy only when extra parallel work is justified.

The example in `configs/examples/best-of.json` uses two candidates and the same
configured own-model ID for synthesis. Model names are data. Candidate count
supports 2–4; total attempt budget is at most eight. The default budget is exactly
three attempts: two variants and one synthesis. `max_concurrent` reserves all
variant slots plus one finishing slot for the later synthesis worker; variants
must finish before synthesis starts, so that reserve is not permission to run a
judge concurrently with unresolved variants. Three reserved permits are needed
for the default two-plus-one workflow, even though only two variants run at once.

`plan_variants` requires the validated original `TaskContract`, its computed
SHA-256 digest, full common baseline Git SHA, protected oracle digest, scoped run
identity, model registry and explicit capacity snapshot. A capacity snapshot names
its registry revision, own revision and logical validity window. Missing/null
free-slot observations are unknown, never assumed free. Stale, unknown or
insufficient capacity returns a visible waiting reason. Declared free capacity
cannot exceed the model's configured concurrency. Cost remains unknown.

Every work unit carries the unchanged original contract: project/root/task,
revision, authority, write/deny scope, acceptance and checks. SHA-256 unit IDs bind
run identity, all original evidence, config, role and ordinal. Identical replay
produces identical IDs for deduplication; distinct variants receive distinct
directory keys. The caller must create separate owned private checkouts from the
same baseline. A shared checkout, writable Git metadata or origin publication is
not granted by these proposals. Atomic admission, real resource permits and
custody remain with the runtime/integration owner; recheck and reserve capacity
at admission rather than treating an old pure proposal as authority.

`plan_synthesis` accepts one report for each exact variant. Missing or unresolved
workers, including unknown process termination, block synthesis. Duplicate,
foreign or stale contract/baseline/oracle bindings fail validation. Inputs are
immutable commit/tree/artifact identities and diagnostic SHA-256 references;
whole agent transcripts are not required. Delivery order does not alter the
synthesis proposal. Failed variants may contribute diagnostic receipts alongside
usable candidates. If every variant fails, the result is a diagnostic proposal
and explicit manual/bounded-repair route, with `accepted: false`; it does not
invent a successful combined candidate.

The synthesis worker receives its own private directory and the original
contract/oracle binding. It may compare candidates and produce a combined
candidate. Its selection, source tests or final message establish no acceptance.
The final combined candidate **must receive fresh independent verification against
the original protected oracle and contract**, then the existing exact-head
integration policy. All work units have `may_publish: false` and
`requires_fresh_original_verification: true`. No API here returns trusted acceptance.

An optional `synthesis_fallback` may explicitly name another registered model,
such as the operator's Luna profile, and qualified failure categories. The
default example disables fallback. Enabling it also requires an adequate total
attempt budget, observed capacity and the registry's actual escalation decision.
Setup errors and blocked input never authorize spending more tokens. A caller
must supply actual synthesis attempts by tier; do not invent counts to exhaust a
cheap tier. If the registry still calls for a same-tier retry or has no permitted
route to the configured fallback, the strategy waits for an explicit routing
decision. There is no silent model substitution and no automatic retry loop.

Each variant and synthesis is its own bounded work unit. The strategy-wide
`max_total_attempts` limits their combined task-root consumption; the registry's
per-task attempt policy additionally governs retries of each unit. Record all
variant, failed and synthesis usage under the original task root, with coverage
and unknown costs intact. A self-hosted model is not assumed to cost zero.

Offline checks after the integration owner registers `pub mod strategies`:

```sh
rtk cargo test -p admiralty-runtime strategies
rtk cargo clippy -p admiralty-runtime --all-targets -- -D warnings
```

Tests cover scope/oracle/baseline binding, deterministic distinct unit IDs,
capacity and attempt limits, unresolved-worker gating, all-failed diagnostic
proposals, immutable synthesis inputs and explicit qualified registry fallback.
Live multi-candidate orchestration remains a separate qualified capability.
