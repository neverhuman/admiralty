# Local session daemon prototype

`admiralty-session` can now run an explicitly enabled, local Unix-socket daemon.
It retains one `SessionHost` independently of request clients, so disconnecting
a dashboard, CLI or coordinator does not destroy that host or replace its child.
This prototype uses pipes and a durable bounded output journal. Global native
execution qualification remains **false**. Starting the daemon enrolls no node,
opens no network listener and launches no program until an explicit request.

## Start explicitly

Create two owned private directories at absolute paths with no symlink
components. For example, after selecting a suitable existing parent:

```sh
session_state="$HOME/.local/state/admiralty-session"
install -d -m 0700 "$session_state" "$session_state/runs"
admiralty-session serve \
  --socket "$session_state/session.sock" \
  --spool-root "$session_state/runs"
```

There are no defaults for either path. The socket's parent and spool root must
already exist, belong to the effective Unix user and have mode 0700. Ancestors
must be directories owned by that user or root without group/world write;
root-owned sticky temporary directories are allowed above the private leaf.
Every symlink component is refused. The socket has mode 0600. A held, private,
no-follow `.daemon.lock` prevents concurrent daemon ownership of one spool root.
Directory identities and permissions are checked again during operation.

An existing socket path, regular file, symlink or other object is always refused.
The daemon never unlinks a socket at startup, shutdown or error. Before reusing a
path, an operator must establish that its previous daemon and sessions are no
longer active, then explicitly reconcile that path. Startup errors exit 78.
`admiralty-session --help` has no effects. Other platforms do not start this
Unix-only daemon.

The boundary is the same Unix user and private filesystem permissions. Any
process authorized to connect can request arbitrary explicit local executables
under that user's authority. This is not a sandbox for hostile programs, a
multiuser authentication service, or a qualified ACL/hostile-same-user boundary.
Run it only with the authority intended for its clients. Do not put credentials
in argv or request records. Programs inherit the daemon environment through the
existing host; credential isolation and provider enrollment are separate work.

## Wire protocol, version 1

Each connection carries exactly one request and one response, then closes.
Frames consist of a four-byte unsigned big-endian byte length followed by UTF-8
JSON. Empty frames and request lengths above 262,144 bytes fail before allocating
the body. Unknown/duplicate fields and unknown command variants are rejected.
Eight connections may be active; saturation returns a bounded `busy` response.
Reads and writes share a two-second absolute I/O deadline, including partial
frames. This bounds socket I/O; filesystem sync and process spawn remain OS calls
and are not a qualified hard real-time deadline.

Requests are:

```json
{"command":"launch","spec":{"run_id":"task-attempt-one","program":"/bin/cat","args":[],"cwd":"/absolute/workspace","spool_dir":"/absolute/private/runs/task-attempt-one"}}
```

```json
{"command":"status","run_id":"task-attempt-one"}
```

```json
{"command":"tail","run_id":"task-attempt-one","from":"0","max_bytes":8192}
```

```json
{"command":"input","run_id":"task-attempt-one","expected_epoch":"0","bytes":[104,105]}
```

```json
{"command":"close_input","run_id":"task-attempt-one","expected_epoch":"1"}
```

`program` must be absolute. The host invokes that exact executable and argv; it
does not construct a shell command. Explicitly requesting a shell executable
still invokes that shell with the supplied arguments. The working directory
must be absolute and exist. The spool path must equal the configured spool root
joined to the safe run ID, with no alternate nesting or root. Run IDs are 1–128
ASCII letters, digits, underscores or hyphens. The shared `LaunchSpec` envelope
allows at most 128 argv entries, each at most 64 KiB and 128 KiB combined, without
NUL bytes. Its serialized JSON must fit 240 KiB and the complete launch request
must fit 256 KiB, including JSON escaping. The direct host, daemon and prepared
Codex builder use the same validator before reserving or launching a run. This
supports a 64 KiB serialized Codex contract/capsule without dropping wire bounds.
Program, cwd and spool paths are absolute and at most 4096 bytes.
The host retains at most 32 lifetime sessions;
exited runs are retained for observation and count against this limit.

Input is 1–4096 bytes. The epoch must be an exact canonical decimal `u64` string,
including for values above JavaScript's integer precision. Under one input lock,
the daemon checks the current epoch, advances it once and uses the existing
prepared/outcome journal before acknowledging delivery. Both stale and future
epochs fail with `epoch_mismatch` and the current epoch. A timeout or ambiguous
write becomes `input_outcome_unknown` and blocks further delivery. It never means
that input was not written or that a retry is authorized.

`close_input` delivers explicit pipe EOF with no implicit bytes or newline. It
compares and advances the epoch under the same lock and journals preparation and
closure. Repeating the successful close with its original expected epoch, or the
resulting current epoch, is idempotent. Any other epoch fails. Input after a
known closure returns `input_closed`; an ambiguous closure remains unknown and
cannot be retried. The direct `SessionHost::close_input(run_id)` helper provides
idempotent EOF to an explicitly owned run without a network request. Providers
can instead use `close_input_if_epoch(run_id, expected_epoch)` for the same
comparison as the daemon. Managed Codex argv-prompt launches must close with
initial expected epoch `"0"`, receiving epoch `"1"`; an intervening stdin write
must reconcile rather than accepting the new epoch and silently changing the
prepared prompt. Launch and EOF remain separate requests. EOF does not
signal or terminate the process.

The direct host also retains legacy `InputKind::Automation`, which can deliver
bytes without advancing an epoch. A fenced direct close therefore checks epoch
freshness but does not by itself prove that no stdin bytes preceded it. Direct
owners must exclude other writers or reconcile protected input-journal evidence.
All daemon wire Input requests use CompareAndAdvance; this direct legacy
limitation must not be mistaken for a daemon input attestation.

Every response has `protocol_version: 1`, a typed `result`, and sealed fields:

```json
{"native_execution_qualified":false,"execution_granted":false,"acceptance_granted":false}
```

These flags describe qualification/authority; an acknowledged launch still means
the local host actually launched the requested process. They never promote
process success into verified task acceptance. A successful launch returns
`result.kind: "launched"` and its run ID. Status returns the owned direct PID,
recorded start identity, running/exited observation, exit result, input epoch,
EOF/unknown-input state, acknowledged output offset and capture failure flag.
`capture_state` is `capturing`, `complete` or `failed` and
`final_output_offset` is a canonical decimal string or null. Only both stdout
and stderr reaching recorded EOF after their durable records establishes a fixed
producer final watermark. A child exit or temporarily empty tail never does.
Reader failure/exhaustion remains failed; a descendant-held pipe remains
capturing even after direct-child exit. Status does not wait or join a reader.
The final watermark describes producer capture, not consumer progress: clients
must tail every byte through that exact watermark before finalizing a parser.
Use the Codex adapter's `finish_at_watermark`/`finish_from_status` rather than
converting `exit_success` or `capture_failed: false` into complete capture.
Status uses try-locks and `Child::try_wait`; it never waits for child completion,
joins readers or holds an idle tool as a stalled process. Contention returns
`busy`. On Linux the host records `/proc` start time; other platforms currently
return a null start identity. An exited PID is historical and grants no signal.

Tail responses have `result.kind: "tail"`, byte-array chunks with stream tags,
decimal-string offsets and a decimal-string `next_offset`. Pages are bounded to
8192 raw bytes and 128 chunks. If the chunk limit is reached, continue exactly at
`next_offset`; no acknowledged bytes are skipped. A cursor ahead of the recorded
output is refused. Responses are capped at 128 KiB before serialization. Existing
host limits remain: 8 MiB of journal storage including framing, 8192 records,
4096-byte input payloads, and a 1 MiB input journal. Torn/corrupt journal records
and capture exhaustion remain visible failures, never clean completion.

Errors use fixed codes without echoing raw argv, payloads or filesystem errors:
`invalid_frame`, `invalid_request`, `unknown_run`, `reconciliation_required`,
`duplicate_run`, `busy`, `epoch_mismatch`, `input_closed`,
`input_outcome_unknown`, or `io_failure`. Launch acknowledgement loss is not a
reason to submit a new run ID. Reconnect and inspect the original ID; a duplicate
launch is refused even when its program or argv differ. A lost input receipt
requires epoch/status reconciliation rather than assuming no delivery.

## Continuity and limits

While this daemon and its host remain alive, client disconnect/reconnect retains
the same actual child object, PID/start-identity observation and journal offsets.
Output capture does not need a viewer, coordinator loop or model turn. Input
serialization is per owned run. No process termination command is exposed.

The host durably reserves each run ID before spawn. After a daemon restart,
existing reservations or spool paths produce `reconciliation_required` for
status and prevent another launch with that ID. The daemon does not adopt a
manifest's PID, restore a child, replay input or remove reservations. A crash or
signal may leave the old child alive; it must remain ambiguous until a separate
qualified owner reconciles it. The orderly host destructor only cleans up its
currently owned child handles using the existing identity-safe semantics.

On Unix, owned-group TERM/KILL uses the native
`rustix::process::kill_process_group` API rather than platform-dependent
`/bin/kill` argument parsing. The raw group ID must exceed one and fit `pid_t`;
zero/current-group and PID-one broadcast semantics are refused. The host sends
both signals while the owned direct child remains unreaped, then polls/reaps it.
Once that child has already been reaped, its historical numeric group never
authorizes another signal. This fixes Linux cleanup of pipe-holding descendants
inside the owned group; escaped descendants remain an unsupported boundary.

Daemon restart adoption, continuity after host destruction, PTY behavior,
terminal attachment, descendant/process-group escape containment, trusted
provider supervision, service-manager installation and native execution
qualification remain unsupported. Do not call this production supervision.

## Validation

The Unix-socket tests use isolated temporary private directories and a fake
`/bin/cat` child. They cover client disconnect/reconnect with the same child and
continuous byte offsets, epoch replay/future rejection, explicit EOF without a
newline, immutable duplicate-run journals, bounded frames and connection slots,
unsafe socket/root refusal, large escaped prepared Codex prompts against a fake
executable, overflow rejection, descendant-held output pipes and fixed final
watermarks, and ambiguous restart behavior. The direct host test
also holds the input lock to prove status returns `busy` and verifies idempotent
EOF. No coding provider, credential or production service is exercised.

A Unix regression creates an owned TERM-ignoring child and a TERM-ignoring
background descendant that holds the capture pipes. Group KILL must close both
pipes and produce a complete fixed watermark within the existing deadlines;
killing only the direct child cannot satisfy it. Other regressions reject
broadcast/out-of-range IDs before effects and ensure already-reaped children
never authorize group signals. The same tests run on the Linux build gate.
