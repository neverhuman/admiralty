# Codex structured adapter — P08C

`crates/admiralty-runtime/src/providers/codex.rs` prepares a shell-free
`SessionHost::LaunchSpec` and decodes bounded `codex exec --json` output. It does
not launch a process, grant a lease, select a fallback, accept a task, run checks,
publish a branch, or qualify native fleet execution. Its qualification and
acceptance flags are private, serialization-only, and always false.

## Observed transport

On 2026-10-18 the registered build node's installed
`/home/ubuntu/.npm-global/bin/codex` reported `codex-cli 0.163.0`. Version, root
help, exec help, and the final flag combination followed by `--help` were checked
read-only through the registered SSH connection, pinned to the requested CPU
pair. No generation, login, credential access, configuration write, or production
repository change was performed. The node name, executable, worktree, spool
root, provider model and registry ID are input data, not fleet constants.

The [official noninteractive documentation](https://learn.chatgpt.com/docs/non-interactive-mode)
describes JSONL stdout, per-turn usage, and piped stdin being appended even when
an argv prompt exists. The adapter also follows the
[0.163.0 exec event definitions](https://github.com/openai/codex/blob/rust-v0.163.0/codex-rs/exec/src/exec_events.rs):
file changes can be completion-only; web searches omit status; command reports
can be declined; MCP and native collaboration calls are distinct item kinds.
Unknown revisions/dialects require explicit qualification rather than an
automatic flag or parser fallback.

## Preparing an exact launch

The caller supplies:

- An immutable, valid, non-simulated `TaskContract` and `CodexAdmission`. Project,
  root, task, revision, authority reference and exact contract digest must agree.
  Attempt ID, owner and incarnation are bound in `SupervisionScope`.
- An already selected `ModelSpec` with provider `codex_cli`, explicit native
  `provider_model`, enabled state, applicable node, registry revision and exact
  model digest. The positive ordinal must fit both attempt budgets. No endpoint
  or credential-file reference is silently applied: this adapter rejects them.
- `CodexExecutionOptions { effort, sandbox }`, with an explicit reasoning effort
  and either `read-only` or `workspace-write`. There is no dangerous/full-access
  variant, approval bypass, model fallback or default execution option.
- A trusted node collector's `CodexCliObservation::from_help` value: registered
  node, absolute executable, supported version, required help flags and a known
  recorded observation tick. The fingerprint covers the path, node, version,
  adapter version, tick and help digest. A caller cannot deserialize a forged
  observation directly into this type. Help remains a caller-supplied report,
  not an executable identity or remote authentication proof.
- Lexically normalized absolute worktree and spool-root paths, without traversal
  or shared ancestry. Existence, real-path aliases, symlinks, mount identity and
  race-free custody must already be established outside this pure builder.
- Bounded immutable lease, resource permit, custody and auth-context references;
  an admission tick, expiry tick, and explicit known current tick. Current time
  must be at least admission and strictly before expiry. CLI observation cannot
  be in the future or over 300 recorded ticks older than admission. Tick zero is
  valid. All ticks must originate in the same recorded clock/epoch; this module
  does not infer wall time or authenticate a clock source.
- A nonempty task capsule. Contract and capsule share a 64 KiB serialized prompt
  limit. The entire immutable contract accompanies the task worker, while the
  captain can retain its compact digest/receipt rather than the whole transcript.

The prepared result also passes the shared host/daemon launch envelope: 64 KiB
per argument, 128 KiB combined argv, 240 KiB serialized `LaunchSpec`, and 256 KiB
complete launch request including escaping. This validation occurs before
returning a prepared success, not after submitting an incompatible wire request.

Call `prepare_launch(contract, admission, model, options, cli, capsule,
explicit_now)`. The returned `PreparedCodexLaunch` exposes its read-only tuple,
tuple digest, and `LaunchSpec`. `into_launch_spec` consumes it when the caller is
ready to submit to a separately admitted session operation. Keep the tuple and
digest in the durable launch-intent receipt before consuming the value.

The program is the observed absolute executable. Its argv is exactly:

```text
--no-daemon --ask-for-approval never exec
--json --color never --ignore-user-config --strict-config --ephemeral
--model <native-provider-model> --sandbox <admitted-sandbox>
--config model_reasoning_effort="<selected-effort>"
--cd <exact-worktree> -- <single-serialized-contract-and-capsule-argument>
```

Each displayed argument is a separate `String`; no shell concatenation or
interpolation occurs. The prompt follows `--`, so prompt text cannot supply CLI
options. Repository rules remain enabled. User configuration is ignored, while
Codex authentication still uses its externally established `CODEX_HOME` context.
Prompts are argv data in this transport and can be visible in process inspection;
do not use this channel to distribute credential contents.

The run ID hashes project/root/task/attempt/incarnation. Changing a model or
working directory within that identity does not invent a new run ID. The spool
directory is `<admitted-spool-root>/<run-id>`. The tuple additionally binds node,
registry revision/model digest, native model, effort, sandbox, worktree/spool,
CLI fingerprint, prompt/argv digests and the admission references/deadline.
`SessionHost` reserves run IDs and refuses existing or ambiguous spool ownership;
changing spool roots must not be used to evade the caller's durable attempt CAS.

## Session and journal integration

1. Recheck the exact task attempt, owner/incarnation, input epoch, policy denies,
   active lease, resource permit, registry revision, directory/binary custody,
   node/auth context and clock immediately before an effect. A reference string
   is not proof that those grants exist. Atomically journal/reserve the launch
   intent in the owner before calling the host; uncertain outcomes reconcile.
2. Launch once through the separately qualified session owner. This module has no
   convenience `launch()` and never chooses to start Codex itself.
3. Explicitly call `SessionHost::close_input_if_epoch(run_id, 0)` once after launch,
   or send daemon CloseInput with initial expected epoch `"0"`. This
   drops the owned pipe; it sends no prompt, newline or signal. A successful
   repeated close is idempotent; `InputOutcomeUnknown` requires reconciliation,
   never a retry or another launch. Require resulting epoch `"1"`; if intervening
   input changed the epoch, reconcile instead of using the new epoch and
   silently binding altered stdin to the prepared argv prompt. The session
   daemon provides same-user, owned-run, epoch-checked journaled CloseInput. It
   does not authenticate a task principal/incarnation or validate a domain grant.
   The unfenced `close_input(run_id)` helper remains available for explicitly
   owned generic sessions; managed Codex clients use the fenced form. Direct-host
   legacy `InputKind::Automation` can write without advancing an epoch, so epoch
   zero alone is not proof of untouched stdin for arbitrary direct callers. A
   direct owner must exclude other writers or reconcile its protected input
   journal; every daemon wire Input instead uses CompareAndAdvance. Neither
   capture finalization method attests prompt purity or input authority.
4. Construct `CodexNdjsonStream::from_prepared(&prepared)` before consuming the
   prepared launch, or retain it alongside the journal tailer. Feed **that exact
   run's** combined stdout/stderr `OutputChunk`s from offset zero in order. Each
   observation carries launch-tuple digest, decimal event ordinal, journal byte
   range, normalized event digest and thread/item identity. `OutputChunk` itself
   has no run ID; attaching foreign chunks is a caller attribution error.
5. Persist observations or compact receipts plus raw spool references. Map byte
   activity, recognized provider events, distinct-command digest, reported tool
   lifecycle and independently checked artifacts to their separate supervision
   clocks. The adapter never advances a verified-milestone clock.
6. On a decoding error, receipt the valid prefix available through
   `observations()` by its ordinal, plus the error and raw journal reference. The
   bounded prefix survives a malformed later line in the same chunk, so journal
   fragmentation cannot discard previously valid facts. Repeated delivery of a
   retained observation must deduplicate using launch tuple and ordinal.
7. Require host/daemon `capture_state: complete` and its fixed
   `final_output_offset`, then drain exactly through that canonical decimal
   watermark. Child exit, a momentarily empty tail and `capture_failed: false`
   are insufficient. A held descendant pipe remains `capturing` with no final
   watermark. Use `finish_from_status(&SessionStatus)` for direct host evidence,
   or `finish_at_watermark(exit_code, CaptureState, final_offset)` after validating
   the daemon status/cursor fields. Both verify producer EOF and exact consumed
   byte offset; unread trailing output leaves capture incomplete. The manual
   `finish(exit_code, capture_complete)` API is for offline/direct-host assertions
   with separately established complete capture, not an exit-to-EOF shortcut.
   Finalization consumes the decoder and fixes one termination observation. The
   summary preserves any parser error and its input chunk byte range. A
   missing exit/capture proof, partial record, open tool, unknown protocol or
   parser failure remains unknown. A nonzero child exit is process failure even
   after a provider completion marker.

`CodexNdjsonStream::new` is available for unbound/offline decoding. Its output
has `launch_tuple_digest: null`; it is not an attributed execution receipt.
Replaying the immutable journal into a fresh parser is deterministic and has no
effects. A gap/overlap/duplicate input range fails instead of silently replaying
provider events. Parsing again from offset zero requires a fresh instance.

## Reports, bounds and uncertainty

The decoder accepts fragmented UTF-8/JSON across chunks and separates stderr
diagnostics from stdout JSONL. Bounds are 8 MiB combined raw bytes, 256 KiB per
line, 8,192 retained compact events, 2,048 item identities, and bounded
cumulative-source tables.
Bounds apply before growing an output line or traversing oversized input.
Duplicate object keys, invalid UTF-8/JSON, impossible item/turn order, changed
immutable command/MCP/collaboration identity under one item ID, counter
overflow, negative/fractional counters and conflicting model/effort metadata
produce sticky failures. Unterminated final JSON is never silently completed.
Unknown event/item/status forms are explicit and prevent a clean termination
classification. Raw JSON remains in the session spool; normalized event digests
are not substitutes for retaining raw journal custody.

`turn.completed` usage is a provider-reported **per-turn** observation. Input,
cached input, cache-write input, output and reasoning output are independent
optional decimal fields. Missing fields stay unknown. Cached input cannot
exceed input and is never added to input again. This adapter computes no price,
credits, burn rate, cumulative grand total or verified accounting receipt.

For a separately identified cumulative source, `observe_cumulative` requires an
explicit thread/session, segment, source-event ID, selected model and counter.
It delegates to `providers::usage::UsageAccumulator`: opening balances remain
left-censored, missing values stay unknown, event replays deduplicate, decreases
are rejected, and resets need a new explicit segment/baseline. Exec turn totals
are never automatically relabeled as cumulative counters or added to them.

The pinned exec event definitions do not guarantee actual model/effort metadata.
Selection is always retained as **requested**. Absence remains unknown rather
than being copied from argv. Explicit root-level model/effort extension fields,
when present, are retained as provider reports and must equal selection; they
remain unqualified metadata until an actual transport fixture establishes their
origin. Agent text cannot provide this metadata. MCP/collaboration items report
provider activity only; native subagents receive no Admiralty enrollment, role,
lease, mailbox or acceptance authority through those events.

A quiet running tool remains open; repeated output does not prove progress.
Command hashes let the supervisor distinguish repeated command text without
replaying it. Tool exits, claimed `PASS`, provider turn completion and child exit
zero never verify or accept a task. Acceptance remains the independent oracle
and scoped verification path.

## Qualification limits and next hook

This component has fake-process/parser proof plus read-only installed CLI flag
compatibility. It has **no paid generation or native fleet transport proof**.
`native_execution_qualified` stays false. `SessionHost` currently inherits its
environment; the builder cannot attest a controlled auth/environment, protected
project configuration/hooks, network policy, binary bytes, directory custody,
cross-restart process adoption or escaped descendants. `--ephemeral` limits Codex
session persistence; it does not prove zero external files/effects. Unsupported
model/effort combinations must surface setup/provider failure, never trigger a
silent cheaper model or automatic replay. `ProcessFailed` is not a classification
of a valid failed task attempt: auth/setup errors, blocked input, transport failure
and task failure require separate evidence. The governor must not consume an
attempt or escalate solely from this summary.

The next integration must supply domain admission around the daemon's same-user,
journaled launch/EOF/tail operations, immutable task-attempt state, exact permit
and custody checks, environment/configuration policy and independent verifier.
It must also record an actual, explicitly authorized sandboxed transport fixture
for the chosen model and effort before enabling an automatic native route.
Ambiguous launch/input/capture outcomes remain reconciliation states. This
packet changes no execution-qualified gate and adds no default live route.

Validation:

```sh
rtk proxy env CARGO_TARGET_DIR=/Users/bentaylor/Documents/ChatGPT/admiralty/target cargo test -p admiralty-runtime providers::codex --lib
rtk proxy env CARGO_TARGET_DIR=/Users/bentaylor/Documents/ChatGPT/admiralty/target cargo clippy -p admiralty-runtime --all-targets -- -D warnings
```

Tests cover exact argv/task/model bindings, literal shell metacharacters,
fake-process EOF and journal decoding, fragmented input, stderr separation,
counter gaps/resets, duplicate keys, model/effort mismatches, known tool shapes,
immutable tool identities, retained-prefix replay, clock zero/staleness, tool liveness,
noisy repeated commands, truncated/missing terminal output, process failure,
bounded inputs, and refusal to treat a claimed PASS as acceptance.
