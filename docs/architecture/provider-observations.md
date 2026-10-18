# Offline provider observations

Claude Code, AGY/Gemini, Grok and generic-process prototypes are compiled by the
workspace and tested in ordinary CI. They are not selectable native execution
providers: the configured provider enum and server launch routing are unchanged.
Compilation, lexical launch validation and parsed help are not qualification.

## Shared JSONL decoder

`admiralty_runtime::providers::observation::JsonlDecoder` accepts byte chunks,
returns provisional JSON objects from `push`, and requires `finish` on an
explicitly complete stream. `decode_complete` performs both operations for an
offline recording. It enforces:

- 256 KiB per line, 8 MiB per stream and 8,192 nonblank object events;
- recursive duplicate-key rejection, including nested arrays and objects;
- strict JSON/object roots, UTF-8 and newline framing, with CRLF supported;
- sticky parse, size and partial-frame failures, and no input after finish.

The decoder supplies no process identity, session ownership, journal offsets,
capture watermark, trusted EOF or authority. Returned values remain observations.
Provider wrappers validate their own supported event shapes and session semantics.

## Claude fixture dialect

The parser recognizes captured fixture dialect `2.1.302`, requires one leading
parent init and one final parent result, and checks session identity wherever
supplied. A result cannot overwrite a previous terminal event. Unsupported event
semantics remain unknown; a provider-reported successful turn is not task acceptance.

Malformed supplied usage never falls back to earlier message counters. Only the
parent terminal record supplies the reported turn total; assistant and opaque
child usage remain separately scoped observations. Missing categories remain
unknown. Nested inclusion, billing settlement and managed child semantics remain
unqualified. Native counters are exact offline `u64` values; future public API
integration must use the domain's precision-safe counter representation.

`ClaudeLaunch` is an unadmitted, shell-free argv proposal. It explicitly requests
print mode, omits the permission flag for the default choice, restricts explicit
choices to a captured subset, and distinguishes fresh UUIDs from resume selectors.
Option-like requested models/selectors are rejected. JSONL input permits normal
multiline capsules while enforcing the session host's 4,096-byte encoded limit,
including escaping and the final LF. Larger capsules need retrieval or separately
qualified chunking; they are never truncated automatically.

No proposal proves immutable task/configuration binding, executable identity,
filesystem custody, resource permits, credentials, continuity or launch admission.
The capability and observation APIs keep native qualification and acceptance false.

## AGY, Grok and generic processes

AGY and Grok expose bounded incremental observation wrappers over the shared
decoder. Syntax and semantic failures stay sticky. AGY requires a bounded event
type; Grok checks the JSON-RPC object/method envelope. Unknown typed payloads may
be retained as raw observations and never become Admiralty coordination messages.
Help-derived advertised flags remain separate from unsupported authentication,
usage, continuation and permission semantics.

Launch proposals accept the complete existing `LaunchSpec`, fill an initially
empty argv and enforce the shared aggregate argv and serialized request bounds.
Absolute lexical paths and option-like requested values are checked; filesystem
ownership, binary behavior and admission remain unqualified.

Generic-process executable names and model argv are explicitly hints and requested
configuration. Screen names and transcript quotations cannot establish a provider.
Observed provider and model remain unknown without a future trusted observation
boundary. Process output/lifecycle observation supplies no billing or permission
authority.

## Checks and remaining activation gates

```sh
rtk cargo test -p admiralty-runtime --locked providers::observation::tests
rtk cargo test -p admiralty-testkit --test provider_claude --locked
rtk cargo test -p admiralty-testkit --test provider_matrix --locked
rtk cargo clippy --workspace --all-targets --locked -- -D warnings
```

The provider targets run 22 Claude and 12 matrix tests without inference. Four
shared-decoder tests cover chunk boundaries, ambiguous JSON, EOF and limits.
Original Claude recordings are retained; new counterexample files are explicitly
synthetic and tracked in Git despite the repository's general JSONL ignore rule.

P21/P22 remain partial. Activation still needs immutable prepared-launch binding,
trusted executable/configuration/resource observations, journal/watermark
integration and bounded real transport/continuity/usage qualification. Such work
must preserve existing Gemini manager observations and ordinary legacy sessions.
