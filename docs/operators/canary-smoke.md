# Local canary smoke harness

`scripts/check/canary-smoke.py` checks the read-only local HTTP boundary using
only explicitly named `admiraltyd` and `admiralty` binaries. It does not build
or discover binaries, consult `PATH`, contact a configured model endpoint, or
attach to an existing Admiralty service.

## Synthetic self-tests

Run the Python standard-library tests without binaries or network services:

```sh
python3 scripts/check/canary-smoke.py --self-test
```

These fixtures exercise path and argument validation, child-reported listener
parsing, protected-port refusal, auth and simulated-mode assertions, bounded
startup/output/HTTP handling, and direct-child cleanup. The fixtures are
synthetic and are not real fleet qualification.

## Explicit local binary smoke

Choose a private scratch directory owned by the current user with mode `0700`
or stricter, and pass absolute paths to both regular executable files:

```sh
python3 scripts/check/canary-smoke.py \
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
children are terminated through their owned `Popen` handles with bounded wait
and kill fallback; no numeric PID, process-group, service or foreign-process
signal is used. The output JSON records per-check state, elapsed time and
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
