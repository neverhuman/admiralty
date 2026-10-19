# Architecture entry points

The current native service combines a Rust HTTP server, a single durable SQLite
Store writer, Rust observation/CLI/runtime components and a React/Vite dashboard.
Public contracts are generated from versioned schemas. SQLite is the current
durable authority; a generic audit profile mentioning PostgreSQL does not change
the architecture or establish a migration requirement.

Start with the [master plan](plans/admiralty-v2/MASTER_PLAN.md) and
[execution ledger](plans/admiralty-v2/EXECUTION.md). Then use:

- [Durable admission](architecture/durable-admission.md),
  [policy fencing](architecture/admission-fence.md) and
  [inference slots](architecture/inference-slots.md).
- [Provider supervision](architecture/provider-supervision.md),
  [session custody](architecture/session-daemon.md) and
  [provider observations](architecture/provider-observations.md).
- [Workplans](architecture/durable-workplans.md),
  [pending inbox](architecture/pending-inbox.md) and
  [native coordination](operators/native-coordination.md).
- [Native lifecycle](architecture/native-lifecycle.md),
  [artifact custody](architecture/artifact-lifecycle.md) and
  [recovery](operators/recovery.md).

Implemented components, deployed observation, actual role communication and
general managed coding execution have separate qualification boundaries. The
[release status](operators/release-status.md) records those limits.
