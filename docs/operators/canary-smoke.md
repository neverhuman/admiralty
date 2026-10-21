# Local canary smoke harness

`admiralty-canary` (the binary of the `admiralty-qualify` crate, library module
`admiralty_qualify::canary`) checks the read-only local HTTP boundary using only
explicitly named `admiraltyd` and `admiralty` binaries. It does not build or
discover binaries, consult `PATH`, contact a configured model endpoint, or attach
to an existing Admiralty service. It is the Rust port of the retired
`scripts/check/canary-smoke.py`; the flags, checks, bounds and JSON receipt are
unchanged:

| Retired invocation | Replacement |
| --- | --- |
| `python3 scripts/check/canary-smoke.py --admiraltyd-binary … --admiralty-binary … --scratch-root …` | `admiralty-canary --admiraltyd-binary … --admiralty-binary … --scratch-root …` |
| `python3 scripts/check/canary-smoke.py --self-test` | `cargo test -p admiralty-qualify canary` |
| `python3 scripts/check/excursions-smoke.py --binary … --scratch …` | `admiralty-canary excursions --binary … --scratch …` |

Build it with `cargo build -p admiralty-qualify` (the executable is
`target/debug/admiralty-canary`) or run it through
`cargo run -p admiralty-qualify --bin admiralty-canary -- …`.

## Synthetic self-tests

Run the synthetic tests without Admiralty binaries or network services:

```sh
cargo test -p admiralty-qualify canary
```

These fixtures (all 23 cases of the retired Python `HarnessTests`) exercise path and argument validation, child-reported listener
parsing, protected-port refusal, auth and simulated-mode assertions, bounded
startup/output/HTTP handling, and direct-child cleanup. The fixtures are
synthetic and are not real fleet qualification.

## Explicit local binary smoke

Choose a private scratch directory owned by the current user with mode `0700`
or stricter, and pass absolute paths to both regular executable files:

```sh
admiralty-canary \
  --admiraltyd-binary /absolute/path/to/admiraltyd \
  --admiralty-binary /absolute/path/to/admiralty \
  --scratch-root /absolute/path/to/private/scratch
```

The harness rejects symlink components and parent traversal in binary and
scratch paths. It creates separate private real-server and demo temporary
scopes, writes a generated bearer token and a single synthetic model policy
with mode `0600`, and uses loopback port `0`. It parses only the server child's
bounded stderr listener announcement and rejects non-loopback, malformed,
future-format, and protected `18890`/`18891` listener identities. Requests use
fixed local API paths and bounded bodies/deadlines; responses and child output
are never stored without byte limits. Child diagnostics have the generated
token redacted.

The real server checks health and confirms `native_execution_qualified=false`,
denies unauthenticated fleet/model reads, then checks the authenticated fleet,
initialized model policy and model-selection proposal. Selection is not a
provider launch. The separate CLI demo is checked for `simulated=true`, absent
model and legacy policy routes, and no CLI flags for attaching either policy.
The CLI `doctor` output must also leave native execution unqualified. Temporary
children are terminated through their owned, still-unreaped child handles
(SIGTERM, bounded wait, SIGKILL fallback, then reaping); no process group,
service or foreign process is signalled. Every child gets a null stdin. The output JSON records per-check state, elapsed time and
binary SHA-256 digests, and explicitly leaves acceptance and fleet rollout
closure false.

## Qualification boundary

A passing local smoke is only one read-only API boundary check for those exact
binary bytes. It does not qualify a provider, managed session, mutation path,
production model credentials, legacy migration, process isolation, retention,
recovery, fleet enrollment, project canary, rollback or release. Synthetic
self-tests are labeled synthetic. If explicit qualified binaries are not
available, leave the real smoke unexecuted and report that dependency as
unknown; do not substitute a build, a guessed path, a running service, or a
protected legacy port. Fleet rollout still requires the complete P31 inventory,
backup, shadow, observed-session, bounded new-task canary, staged expansion and
rollback evidence from the deployment owner.

The HTTP deadline covers connect, request transmission, response headers and the
complete response body. A monotonic watchdog shuts down only this request's own
connection. Shutdown can surface as EOF rather than a timeout exception, so the
reader checks an independent expiry marker and monotonic time after blocking I/O,
including EOF, before reporting success. A body shorter than its declared content
length is rejected. Tests retain the original slow-drip timing limits and include
deterministic EOF/expiry coverage plus complete and truncated local responses.
These checks do not contact inference or production services. Tests
resolve their own temporary-root aliases before passing strict no-symlink paths;
production input validation still refuses symlink components.

The response parser follows CPython `http.client` framing (status line,
`email`-style headers, `Content-Length`, chunked and close-delimited bodies);
`crates/admiralty-qualify/tests/python_parity.rs` replays recorded responses,
listener lines, JSON bounds and redaction cases against outcomes captured from
the retired Python harness.

## Isolated excursions smoke

`admiralty-canary excursions --binary <admiraltyd> --scratch <new-dir>` exercises
an isolated real incident journal, including a restart. The scratch directory
must not exist; it is created with mode `0700` under protected, owner-controlled
ancestors and keeps `token`, `config.json`, `incidents.db`, `server.log` and
`result.json`. Only the supplied binary is started, on an ephemeral loopback
port; there is no deployment. Requests use a 10-second whole-request deadline
and a 2 MiB response bound. Success prints the result JSON and exits 0; any
failed assertion is reported on stderr with exit 1.
