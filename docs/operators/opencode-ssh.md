# OpenCode over registered SSH nodes

`admiralty-runtime::providers::opencode_ssh` prepares a reviewable SSH argv and
parses bounded OpenCode JSON events. Node aliases, model references, executable
paths and remote checkout paths are configuration. The module performs no SSH,
reads no credential/configuration files, starts no providers and grants no task
acceptance. Native runtime execution remains unqualified.

An `OpenCodeSshProfile` names a registered `node_id`, safe `ssh_alias`, absolute
local `ssh_program`, absolute `remote_program`, optional `remote_config` file
reference, existing `remote_worktree`, and explicit `provider/model` reference.
The directory must exist **on the execution node**; a Mac operator checkout path
does not establish a corresponding directory on that node. The caller must
independently establish checkout custody and node authority.

The generated command uses noninteractive SSH, batch authentication and bounded
connection/liveness options. Each remote argv component is shell quoted because
SSH's remote command crosses a shell boundary. Dynamic host-option injection and
path traversal are rejected. The remote OpenCode command is:

```text
env OPENCODE_DISABLE_AUTOUPDATE=true [OPENCODE_CONFIG=/protected/config-reference]
  /registered/path/opencode run --model provider/model --agent build
  --dir /registered/private/checkout --format json TASK_CAPSULE
```

No API key belongs in a task capsule, profile, command argument, fixture or
receipt. Existing protected provider configuration owns authentication. This
module does not read or copy those credentials. Configuration references are
not guarantees that an installed provider/model tuple is available or authorized.

## Observed installation and qualification

On 18 October 2026 the owner's existing registered node had OpenCode **1.19.3**
and an installed coding helper selecting
`ornith/Ornith-1.5-35B-A3B`. Read-only probes verified `run --help` supports
`--model`, `--agent`, `--dir`, and `--format json`; helper `--json` maps to that
format. Session export supports `export --sanitize SESSION_ID`. These are
observed flags for that installation, not a promise about arbitrary CLI versions.

An initial bounded real coding task ran through that installed helper. The
owner created a unique private, unborn Git checkout and pinned an external
read-only oracle before model execution. The task concerned interval gaps for
resource coordination; the oracle had eight unittest methods including an
80-case seeded reference comparison. No existing project or service was changed,
and no package installation, reboot or throughput load test occurred.

The helper exited 0 after 97.565 seconds; independent verification exited 1.
The implementation remained the original `NotImplementedError` stub. All oracle
methods therefore failed; its before/after SHA-256 matched the owner's pinned
bytes, and candidate write scope remained intact. OpenCode made five tool calls,
encountered an unborn-HEAD Git lookup failure, then stopped after an external
read requested permission and was rejected. This is a blocked checkout/tool
workflow, not successful coding or proof that the model cannot solve the task.
The original failed receipt and raw outputs remain intact.

The owner then explicitly authorized **one setup-corrected attempt** on that same
private fixture. The owner added a baseline Git commit, visible read-only tests
and a complete README contract inside the checkout. The model was told to read
only that checkout and change only `gaps.py`. It completed in **30.039 seconds**;
the helper and independent verifier both exited 0. All eight oracle methods and
their 80 seeded reference cases passed. A separate owner recheck also passed.
The external oracle, visible tests, README and baseline Git HEAD retained their
recorded hashes; the only tracked modification was `gaps.py`. Actual session
export again confirmed the selected Ornith provider/model. This qualifies one
trusted-local coding loop for this installed tuple, not native fleet execution.
No Luna escalation was needed, and the setup failure was not counted as a model
correctness failure.

Independent export of each exact session confirmed provider `ornith` and model
`Ornith-1.5-35B-A3B`. Native step reports included input/output/cache values,
with reported reasoning zero and reported cost zero. Their aggregation, billing
and cache economics remain unqualified. Zero reported cost is not evidence of
zero hardware cost. Raw JSON events and verifier output stay in the owner's
private qualification directory; sanitized metadata belongs in fixtures.

Routing records the original setup-blocked attempt separately from the corrected
task's independent pass. Resolve missing checkout/permission contracts before
retrying. Any higher-tier attempt needs a
recorded reason and its own bounded task budget; do not silently substitute a
different model or equate helper exit 0 with acceptance.

## JSON observations and limits

`OpenCodeJsonStream` accepts fragmented NDJSON, caps individual events at 256 KiB,
the complete stream at 8 MiB and event count at 8,192. Malformed, oversized or
truncated data poisons the parser; preserve raw output and reconcile. Unknown
event types remain explicit observations. Tool completion and step finish never
produce trusted acceptance.

Counters are precision-safe decimal strings. Missing, negative, fractional or
otherwise unsupported counters remain unknown. Step usage is labeled
`unqualified_provider_step_report`; the adapter does not sum possibly overlapping
reports. Inference model identity is unknown when JSON events omit it.
`model_identities_from_export` reads only an explicitly supplied export of this
task's own session, and `ensure_selected_model` rejects observed fallback/mixed
identity. An empty identity set yields unknown, not a match.

## Explicit one-task probe helper

`admiralty opencode probe` (Rust port, crate `admiralty-tools`, of the retired
`tools/probe_opencode.py`; `python3 tools/probe_opencode.py ARGS` →
`admiralty opencode probe ARGS`) is a temporary qualification tool, not a scheduler. It
requires explicit `--qualify` and a new local output file in an existing owned
directory. Choose an already authorized node and remote helper:

```sh
rtk proxy admiralty opencode probe \
  --node registered-node --remote-parent /registered/qualification-root \
  --remote-helper /registered/bin/coding-helper \
  --opencode /registered/bin/opencode --model provider/model \
  --output /owned/receipts/one-task.json --qualify
```

Each ordinary invocation creates only `admiralty-qualification-<unique-id>`
beneath the chosen remote parent, with a fixture-only baseline commit and all
required visible task inputs inside its candidate checkout. It runs one helper invocation with a 120-second remote
deadline and an 8-MiB process file-size limit. The output contains source/oracle
digests, independent verification result, exact observed model metadata, native
step reports, elapsed time and artifact references. It does not claim process
continuity, isolated credential protection, concurrent throughput, fleet
scheduling or restart adoption. Do not rerun it as a monitoring heartbeat.

The qualification workload is part of the task's meaning, so it is unchanged:
the owner oracle (a Python `unittest` program for the Python `gaps.py` task),
the task prompt and the remote harness are byte-identical string constants in
`crates/admiralty-tools/src/opencode_probe/workload.rs`. They are data sent over
SSH and interpreted by the node's own `/usr/bin/python3`; no Python file exists
in this repository for them. Their SHA-256 digests are pinned by tests, so
`oracle_owner_expected_sha256` stays comparable with earlier receipts. The local
side runs one fixed `ssh -T -o BatchMode=yes -o ConnectTimeout=8 <node> ...`
with a 180-second deadline (`--ssh-binary` overrides the default
`/usr/bin/ssh`), never reads credential files and writes the receipt with
exclusive creation. Exit codes: 0 qualified, 1 failed qualification or local
error, 2 refused arguments or unresolved SSH outcome.

An explicitly authorized setup correction can use
`--correct-fixture /registered/qualification-root/admiralty-qualification-<id>`.
It preserves the original attempt and creates a new `attempt-1` artifact directory;
an existing `attempt-1` refuses another automatic rerun. This flag is not
permission for an unbounded repair loop.

The independent oracle is read-only and outside the candidate checkout, with independent
local digest comparison. Both still share an OS account: this is trusted-local
qualification, not enforcement against an adversarial worker. A changed oracle,
unexpected candidate file, unknown remote outcome or model mismatch blocks
acceptance. Preserve failures and all previous receipts.

Offline checks after integration registers the module:

```sh
rtk cargo test -p admiralty-runtime providers::opencode_ssh
rtk cargo clippy -p admiralty-runtime --all-targets -- -D warnings
```

The fixtures are synthetic except the explicitly labeled qualification metadata.
No tests invoke a provider, SSH, API request or forge operation.
