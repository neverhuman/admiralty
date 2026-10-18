# Offline CLI roundtrip proof

The `offline_roundtrip` integration test exercises the existing `admiralty` binary
through its public command line. It records a simulated task and a handoff message
in a private SQLite fixture, then opens new binary processes to read the snapshot,
compile task context, inspect the inbox, and acknowledge the message. It starts no
provider, task process, server, or external service.

Run the focused proof with the packet's private build settings:

```sh
CARGO_BUILD_JOBS=1 \
CARGO_INCREMENTAL=0 \
CARGO_PROFILE_DEV_DEBUG=line-tables-only \
CARGO_TARGET_DIR=/home/ubuntu/admiralty-v2-build/targets/adm_pool_adm_p19a \
rtk cargo test -p admiralty-cli --test offline_roundtrip -- --nocapture

CARGO_BUILD_JOBS=1 \
CARGO_INCREMENTAL=0 \
CARGO_PROFILE_DEV_DEBUG=line-tables-only \
CARGO_TARGET_DIR=/home/ubuntu/admiralty-v2-build/targets/adm_pool_adm_p19a \
rtk cargo clippy -p admiralty-cli --test offline_roundtrip -- -D warnings
```

The test launches `env!("CARGO_BIN_EXE_admiralty")`; it does not reimplement CLI
parsing or call crate internals. Each child has a 15-second timeout, a 64 KiB
captured stdout and stderr limit, and cleanup through its own `Child` handle. Test
files and the database live below a mode-0700 fixture directory under the assigned
private Cargo target. Contract and message JSON files use mode 0600. `TMPDIR` is
set to that fixture root for every child.

## Assertions

The fixture includes project, root, task and authority identities, owned and
forbidden paths, explicit non-goals, a negated objective, and an exact argv check.
The test checks that validation, durable submission, snapshot and compiled context
preserve those values and the contract digest. The submitted task remains `ready`
with no owner, attempt, or candidate. Submission reports `execution_started=false`.
The doctor response reports `runtime_enabled=false` and
`native_execution_qualified=false`.

The test queues a supported `Handoff` message scoped to the same project/task,
fetches it through the intended recipient's inbox, then records an ACK for its
exact recipient and context epoch. The ACK response identifies itself as
`application_receipt_only`. Wrong-recipient and stale-epoch ACK attempts must fail;
after each one the event cursor, projection, and integrity-incident count must be
unchanged. A fresh process reads the same task, message, acknowledgment, and
context digest after restart.

Invalid task submission, snapshot of a missing database, schema check of a missing
directory, and an unknown command must all exit nonzero. The invalid task, missing
snapshot, and unknown command must not create a database; schema check must not
create its input directory.

## Limits

This test proves the current offline CLI and durable store semantics only. The
submission API does not activate execution: there is no scheduler/runtime
activation in this command path, and the test intentionally asserts that absence.
The CLI uses its own wall clock when it records event observation times; it exposes
no injected clock option. Fixture IDs, producer identity, event sequence numbers,
contract content, and message epoch are explicit and stable. This is not evidence
of live-provider behavior, server authentication/SSE correctness, runtime
activation, execution acceptance, or publication authority.
