# Deterministic offline test fixtures

`admiralty-testkit` exposes four reusable fixture modules. They run without
credentials, external services, process signals, sleeps or wall-clock reads.
The named `deterministic_faults` target is registered in the ordinary workspace
tests and uses the real mailbox, JSONL decoder and usage accumulator.

| Module | Supported behavior | Bound or limitation |
| --- | --- | --- |
| `clock` | Explicit millisecond clock, fixed deadlines, checked advances | Backward time and overflow reject without mutation; it does not replace runtime clocks. |
| `faults` | Per-point, one-based occurrence schedule | At most 256 rules, 64-byte point names, 60-second symbolic delays; unknown points reject. No dynamic points, event history or fault execution. |
| `forge` | Immutable intents, full-head CAS, cached rejection/success and reply-loss probes | At most 256 refs and 1024 requests, 128-byte identifiers; history exhaustion rejects without eviction. No Git or network. |
| `fake_provider` | Seeded JSONL and chunk boundaries, malformed output, quiet/crash/input/noisy observations, cumulative reset readings | Synthetic data and exit-code descriptions; these helpers do not start a process or qualify a real provider. |

The clock and plan are independent values. A harness explicitly interprets
`DelayMs`, `Drop`, `Duplicate`, `Fail` or `Crash`; returning that value executes
nothing. A clock deadline is due at equality. Clones replay independently.

The forge simulator distinguishes the reply from the simulated server fact.
Losing a reply produces `OutcomeUnknown`. A probe or exact retry can return the
immutable recorded outcome; it never reapplies a request. Reusing an ID with
different scope or heads rejects. Its before-mutation loss means the server
admitted an intent and recorded `NotApplied`, not that transport lost a request
before admission. Retry faults are not reinjected into cached replies. Neither
observed CAS nor reconciliation grants publication, execution or task acceptance.

`Split` cuts one valid JSONL event inside the UTF-8 bytes for `é` and withholds
its final LF. Its concatenated bytes match `Success`. The shared decoder test
requires no event before that LF, rejects truncated captures and keeps failures
sticky. Provisional decoded frames alone are not capture completion.

`CumulativeReset` emits eight readings with six unique usage event IDs. The
outer source sequence advances on every emitted frame; repeated inner event IDs
represent duplicate semantic readings, not exact transport replay. Counters and
baselines are decimal strings or null. The unknown opening segment remains
left-censored even after later deltas; the separate reset/model segment reaches
a known total of five. Summing known deltas does not establish a full-session
total or measured context savings. This is a synthetic fixture format, not a
provider wire protocol.

Run from the repository root with a writable owned Cargo target and canonical
temporary directory outside the checkout:

```sh
rtk cargo test -p admiralty-testkit --lib --test deterministic_faults --locked
rtk cargo test --workspace --locked
```

These tests advance P01's offline fixture infrastructure. They do not close
P00–P31 acceptance, native admission, real forge behavior, runtime crash
recovery, provider qualification, installation or fleet rollout gates.
