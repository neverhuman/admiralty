# P08Q controlled Codex transport conformance

`crates/admiralty-runtime/examples/codex_qualification.rs` is an explicit,
operator-controlled transport harness. It reuses the existing Codex launch
builder, `SessionHost`, fenced EOF, bounded journal decoder and final watermark.
It does not implement another Codex CLI or event parser and does not enable a
native route. Its receipts always retain:

```json
{"native_execution_qualified":false,"domain_admission_granted":false,"task_acceptance_granted":false}
```

One passing run establishes only controlled **safe exact-fixture transport
conformance** for the observed executable/model/effort/environment. It does not
qualify general coding quality, equivalent implementations, independent native
acceptance, domain admission, fleet enrollment, token escalation, disaster
recovery or all of P08. OpenCode fixture receipts remain a separate evidence set.

## Default and offline behavior

Build, tests, `--help`, and invocation without `--run` perform no paid inference.
An explicit private config without `--run` validates it and prints a small
read-only plan; it creates no scratch directory and starts no child. Unknown or
duplicate config fields and unsupported arguments fail closed.

```sh
rtk proxy env CARGO_TARGET_DIR="$PWD/target" cargo build -p admiralty-runtime --example codex_qualification
rtk proxy env CARGO_TARGET_DIR="$PWD/target" cargo test -p admiralty-runtime --example codex_qualification
rtk proxy env CARGO_TARGET_DIR="$PWD/target" cargo clippy -p admiralty-runtime --example codex_qualification -- -D warnings
rtk proxy target/debug/examples/codex_qualification --help
```

The offline tests use temporary private directories and owned shell/fake-provider
children, never an installed coding provider or credentials. Linux additionally
exercises the full harness against `P08Q_fake_codex.sh`: isolated Git init, help
observations, prepared launch, initial EOF, source mutation, NDJSON, final
watermark and protected verifier. That test matches its inherited CPU list and
assumes no particular fleet CPU assignment. A fake receipt cannot be cited as
native-provider evidence.

## Review the config before any real run

Copy `tests/qualification/codex/P08Q_config_template.json` to a private file
outside Git, then replace every placeholder. The template is intentionally
unrunnable. This packet's code-only stage authorizes no actual generation; a real
config, target, scope, limits and provider run must receive the integration
owner's review first.

The config must be an absolute canonical, effective-user-owned regular file with
mode 0600, one link and at most 16 KiB. No credential key, token, endpoint,
credential-file content or provider login belongs in this config. Unknown fields
are rejected, including secret-shaped additions. Required fields are:

| Field | Required meaning |
| --- | --- |
| `schema_version` | Exactly 1. |
| `registered_node_id` | Operator's actual registered-node assertion; not enrollment proof. |
| `run_id` | New unique safe ID, at most 64 ASCII identifier bytes. |
| `scratch_parent` | Existing dedicated canonical 0700 directory owned by this user, outside Git checkouts and distinct from the home directory. |
| `codex_program` | Reviewed absolute **canonical executable**; resolve the installed launcher symlink explicitly rather than using an alias. |
| `git_program`, `env_program` | Reviewed canonical executable paths for isolated Git initialization. |
| `registry_model_id`, `provider_model` | Explicit registry label and actual native Codex model. No defaults or fallback. |
| `effort` | Explicit supported `CodexEffort`. |
| `expected_cpu_affinity` | Exact already-owned inherited Linux CPU-list observation. This records an operator decision, not a runtime resource grant. |
| `cli_probe_timeout_secs` | 1–30 seconds per read-only version/help/helper child. |
| `provider_timeout_secs` | 1–300 seconds for the single Codex session. |
| `overall_timeout_secs` | At most 600, and at least provider time + four probe limits + five seconds. |
| `max_output_bytes` | Consumer stop threshold from 4096 through 262144 bytes. The host's separate hard capture cap is 8 MiB including framing. |
| `operator_transport_authorization` | Literal `controlled_exact_fixture_transport_only`, together with explicit `--run`. |

Paths reject aliases, symlink components, unsafe ownership/permissions and special
files. Executables must be regular, executable, non-writable by other users and
bounded in size. The harness records device/inode/size/timestamps plus the raw
executable `program_sha256` (`sha256:<hex>`). It streams at most 512 MiB through a
64 KiB buffer from a no-follow descriptor, compares opened/path identity before
and after reading, and verifies the digest again before paid launch and after
the provider exits or owned cleanup is attempted. A changed/unavailable digest
fails conformance. `program_sha256_after` and `program_unchanged_after` retain the
post-run evidence, including unknown hash observations.
These checks do not authenticate a publisher, interpreter/loader or delegated
vendor binary, establish provider credentials, or prevent a hostile exec race.
Prefer the reviewed actual native executable when registering the test target;
hashing a launcher authenticates only that launcher file's observed bytes.

The operator must separately establish that the selected CPU slot is released
and owned, the executable/node/model are the intended ones, and ambient Codex
authentication/configuration is appropriate. An idle Screen process still owns
its lane; idle is not released capacity. In the initial P08Q implementation pass,
read-only checks found existing CPU105 owners, so implementation stayed local and
no Luna/Screen worker was injected or started.

After the concrete config is reviewed, validation is read-only:

```sh
rtk proxy target/debug/examples/codex_qualification --config /absolute/private/p08q-config.json
```

A later explicitly authorized live run uses the built binary under the already
owned Linux affinity, for example:

```sh
rtk proxy taskset -c "$qualification_cpu" /absolute/build/target/debug/examples/codex_qualification --config /absolute/private/p08q-config.json --run
```

`qualification_cpu` must be the reviewed current assignment; this document does
not allocate it. Compilation uses the build lane's separate CPU/jobs policy.
The harness verifies the inherited CPU list before a live launch. Non-Linux live
runs are refused because this packet does not supply another affinity observer.

## One isolated attempt

The harness refuses an existing run directory. A new run owns:

```text
<scratch_parent>/<run_id>/
  intent.json             exact reviewed config and safe-fixture digest
  work/                   fresh isolated Git repository
  protected-oracle/       expected source and policy, outside worker write scope
  spool/                  existing SessionHost journals/manifests
  receipt.json            bounded final result, including failure stage/cleanup
```

Directories are 0700. State/receipt files are 0600; oracle files start 0400. Git
initialization runs through the owned host using `env -i`, no system/global
config, no template hooks, and the exact configured Git executable. No network
Git commands, commits, forge operations or production repository changes occur.
Each helper gets its own run reservation, initial EOF and bounded capture.

Before inference, read-only version/exec-help/root-help observations must produce
the installed dialect supported by `CodexCliObservation`. The harness creates a
valid exact `TaskContract`, observes the workspace with `WorkspaceSpec::validate`,
and prepares the existing adapter with the explicit model, effort and
`workspace-write` sandbox. No config or reference string is interpreted as a
domain grant. The admission reference strings describe a legacy operator's
controlled fixture decision only; all authority flags remain false.

The single task replaces only `src/lib.rs` with the canonical tiny Rust
`saturating_increment` implementation. Its prompt forbids commands, delegation,
extra files, Git changes, dependencies, network and oracle access/mutation.
These instructions and final-tree checks are not a fully qualified security or
tool policy boundary.

The local `SessionHost` remains private to the harness, which exposes no input
writer to another client. It closes initial stdin with
`close_input_if_epoch(run_id, 0)` and requires resulting epoch 1. No prompt or
newline is sent through stdin. Epoch/closure drift is a failure. Legacy direct
Automation can retain an epoch, so epoch alone is not a generic input-purity
attestation; the controlled harness additionally excludes other host writers.

The harness tails combined stdout/stderr from offset zero through the existing
decoder. Requested model/effort always remain separate from observed metadata.
Missing actual-model metadata remains null/unknown. Provider messages, claimed
PASS and turn completion never become verified task acceptance. Only a known
direct exit, clean both-stream capture EOF and a consumer cursor exactly at the
fixed final watermark permit coherent stream finalization.

## Protected safe verifier and receipts

The independent verifier is compiled into the operator-owned harness. Its
reviewed reference source is also retained outside the worker checkout. It
requires exact source bytes, checks the safe reference's boundary cases, and
compares the final tree against the baseline excluding only `src/lib.rs`.
Symlinks, hard links, special files, oversized files/trees, extra paths,
forbidden-path changes, changed oracle or changed launch intent fail. It never
compiles or executes arbitrary generated code. A different correct implementation
also fails: this is exact-fixture conformance, not semantic equivalence or a
general coding evaluator.

Config/content/fixture digests use shared `content_digest` over canonical
serialized values, including byte arrays. They differ from the executable's raw
streaming `program_sha256`; do not interchange their hash domains. Retain the exact example/fixture source commit
and externally hash artifacts when filing an integration receipt.

The final receipt records the config/program/affinity observations, exact launch
tuple/digest, requested model/effort, structured stream summary, probe exits,
safe-oracle result, artifact digest, failure stage and owned-cleanup outcomes.
It stores no credential values or raw provider message text. Private bounded
spool journals retain provider output for investigation and must not be copied
into public logs or Git. Do not introduce credentials through prompts or argv.
The harness does not inspect credential files or dump its environment.

On deadline, output exhaustion or parser/custody failure, it stops only its owned
direct child/group through the existing host and records whether capture/cleanup
completed or requires reconciliation. It never signals an unrelated or reaped
historical PID/group, adopts a previous session, retries a paid call, replaces a
run identity, or deletes evidence. A failure before final receipt creation can
leave the fresh intent and spool as the reconciliation record; filesystem errors
may prevent writing a final receipt.

Time bounds are checked before launch and during tailing, with existing bounded
owned cleanup. Filesystem sync and spawn remain operating-system calls and are
not a hard real-time SLA. Capture can reach the host's 8 MiB cap before the
consumer observes its smaller stop threshold. Ambient provider auth/environment,
same-user hostile processes, binary/path races, escaped descendants, fully
isolated network/resources, durable domain admission and accepted-work semantics
remain unqualified. One successful fixture must not change a production gate.
