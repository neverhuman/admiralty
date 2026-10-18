# Recorded status diagnostics

Run `admiralty explain --file recorded-status.json` to explain a supplied status
record. It prints a recorded-only notice to stderr and a structured JSON
diagnostic to stdout. The command only reads that file; it does not open a store,
refresh sources, read a clock, contact providers or launch a process. No model
turn is needed. Parse or admission failures return a nonzero exit status.

## Input

The file must be a regular UTF-8 JSON file of at most 1 MiB. The reader bounds
allocation using file metadata, then reads at most 1 MiB plus one byte to detect
growth. On Unix, opening uses nonblocking and no-follow flags before checking the
opened file type, so FIFOs, sockets, devices and final-component symlinks are
refused. Other platforms use a symlink/type check and bounded read; equivalent
race protection on those platforms has not been qualified. The pure typed API
also streams a size check before hashing or cloning an input record.

This synthetic example records a provider tool wait with unknown evidence:

```json
{
  "schema_version": 1,
  "status": {
    "contract_revision": 1,
    "contract_digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "authority_ref": "grant:recorded-task",
    "attempt_id": "attempt-one",
    "owner": "worker-one",
    "producer_id": "recorded-producer",
    "producer_incarnation": "boot-one",
    "source_sequence": "9007199254740993",
    "project_id": "recorded-project",
    "root_id": "root-one",
    "task_id": "task-one",
    "source_revision": "rev-1",
    "source_tick": "10",
    "state": "tool_wait",
    "ownership": "known",
    "provider_liveness": "live",
    "evidence_presence": "unknown",
    "evidence_provenance": "unknown",
    "native_execution_qualified": false,
    "candidate_head": null,
    "receipt_digest": null
  },
  "policy": {
    "explicit_now": "10",
    "previous_source": null,
    "stale_after": "5"
  }
}
```

Ticks and source sequences are canonical decimal `u64` **strings**; JSON numbers,
leading zeroes and overflow are rejected. Source sequences start at one. A
positive `stale_after` uses the same logical tick unit as the supplied producer
clock. `explicit_now: null` means the evaluation clock is unknown; the command
never substitutes wall time. `source_tick: null` preserves unknown source time.
Only compare ticks from a caller-established clock domain.

Project, root, task, attempt and owner use validated core identifiers. Contract
revision must be positive; contract and receipt digests use `sha256:` followed by
64 lowercase hexadecimal digits. A candidate head is a complete, non-null,
lowercase Git object ID of 40 or 64 hexadecimal digits. A digest or object ID is
an identity claim, not proof that the object exists or was verified. Producer,
incarnation and opaque revision identifiers are 1–128 ASCII letters, digits,
underscores, periods or hyphens; `.` and `..` are refused. Authority references
are bounded to 512 bytes without whitespace or control characters. These
references must contain identifiers, not credentials or free-form source text.
Unknown fields, duplicate JSON fields and unsupported schema versions fail
closed. The input has no command or arbitrary explanation field.

## Meaning and provenance

`explain_status` returns fixed reason codes, a short explanation, exact scope and
source references, plus a deterministic ordered set of advisory actions.
Unknown, future or stale source time requests a refreshed snapshot. If known
ownership lacks an exact owner or attempt, the result requests reconciliation.
Unknown provider liveness and missing evidence request evidence. A provider tool
wait does not establish idle work, a stall or a dead process. Waiting is suggested
only for a current recorded source, a bound owner/attempt and recorded live
provider. Planned work suggests plan review; blocked work suggests inspection of
the recorded blocker. Candidate review requires current source time, a candidate
head and recorded evidence presence. No suggested action signals or restarts a
process.

To compare two records, copy the earlier output's `source` object into the next
input's `policy.previous_source`. It pins producer/incarnation, sequence, opaque
revision, optional tick, scope digest and observation digest. A changed task,
contract, authority, owner or attempt is a scope mismatch. A changed producer or
incarnation is explicit. Sequence/tick regression or different observations with
the same source sequence suppresses operational advice and requests review and a
fresh snapshot. This comparison only checks the supplied records; it does not
authenticate their origin or retain history between CLI runs. Replaying the same
status and policy produces the same output.

`verified_recorded`, `integrated_recorded`, `evidence_provenance: "independent"`
and `native_execution_qualified: true` remain assertions from the input. The
output calls out unverified provenance or qualification claims and requests
manual review. The qualification assertion is preserved as
`reported_native_execution_qualified`. Output trust fields are sealed by the
implementation and always contain:

```json
{
  "execution_granted": false,
  "acceptance_granted": false,
  "recorded_only": true,
  "read_only": true
}
```

The CLI and pure explanation component are available. Automatic live adapters,
authenticated source provenance, native ownership reconciliation, provider
qualification, trusted verifier execution and acceptance remain outside this
component. A successful diagnostic is not evidence that any of those paths ran.
