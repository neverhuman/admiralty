# Temporary legacy rollout observer

`admiralty rollout observe` joins the integration owner's rollout template with
captain-written lane reports. It is the Rust port (crate `admiralty-tools`,
module `rollout_observe`) of the retired `tools/rollout_observe.py`; flags,
JSON output, exit codes and safety bounds are unchanged
(`python3 tools/rollout_observe.py ARGS` → `admiralty rollout observe ARGS`). It refreshes the existing dashboard without new
model turns. It never launches agents, sends terminal input, changes tasks,
executes JSON content, verifies code or performs publication.

Use an existing private runtime directory owned by the observer's Unix user:

```sh
rtk proxy admiralty rollout observe \
  --source /home/ubuntu/admiralty-v2-build/rollout.json \
  --lanes /home/ubuntu/admiralty-v2-build/live-lanes.json \
  --output /home/ubuntu/xbabe2_kicker/captains-dashboard/rollout.json \
  --interval 15 --session-prefix adm_lane_
```

These paths are deployment examples, not product constants. `--once` performs a
single observation and exits. Omit `--session-prefix` to disable all process
commands. With a prefix, the only command is fixed argv `screen -ls`; only names
starting with that exact prefix are considered. No sessions are attached to,
launched, killed or typed into. Screen PID/name matching corroborates the Screen
session identity, not agent activity, task execution or candidate correctness.
Unavailable Screen probes remain unknown. Sessions outside the prefix remain
out of scope. This observer must run on the node hosting the named sessions;
it does not probe remote nodes or interpret reported PIDs as remote processes.

The source template is the integration owner's version-one dashboard manifest.
Every packet's status, claim, verification and integration provenance is retained
unchanged. Lane reports cannot advance packet status, establish trusted proof or
unlock dependencies. `updated_utc` and `producer` remain the template's values.
The independent observer timestamp appears under `observer.observed_utc`;
source/report times remain separately recorded. Repeated observation never makes
an old source fresh.

The captain may write either a bare lane array or this object, atomically:

```json
{
  "updated_utc": "2026-10-18T12:00:00Z",
  "lanes": [
    {
      "id": "contracts-worker",
      "packet_id": "P02",
      "owner": "worker-one",
      "node": "enrolled-node",
      "branch": "codex/contracts",
      "screen": "12345.adm_lane_contracts",
      "status": "running"
    }
  ]
}
```

`id` is the stable lane identity; `lane` or `screen` can supply it when `id` is
absent. `session`, `host`, `packet` and `state` are accepted aliases for `screen`,
`node`, `packet_id` and `status`. A row's `updated_utc`/`observed_utc` overrides the
wrapper time. Bare arrays can use the lane file's modification time, explicitly
recorded as such. Prefer explicit timezone-qualified timestamps.

Provider-internal agents can set `screen` and `pid` to `null` and identify their
`agent_session`. Their session liveness stays **not exposed**, never orphaned
merely because no OS Screen exists. The dashboard retains their reported
`candidate_committed` or `in_progress` state as a self-report, not verification.
Model/effort and CPU reservation appear in the lane status; `cpus_pinned: false`
is explicitly **NOT pinned**. Reservation alone never establishes enforcement.
Additional bounded worktree, tier and candidate-head metadata remains in runtime
JSON. No commands or paths from those fields are executed or followed.

The displayed lane status states pending/fresh/stale/orphaned/unknown/future,
its self-reported status and the independent Screen result. Fresh reports older
than 120 seconds become stale even if Screen remains present. Missing reports
retain template lanes as pending. An independently missing or PID-mismatched
Screen marks a reported lane orphaned. Conflicting duplicate lane IDs collapse
deterministically and remain pending; they never look healthy. Identical
duplicates collapse harmlessly. Source reports and template objects are not
rewritten.

Both input files must be owned regular JSON, at most one MiB; combined output is
also bounded to the dashboard's one MiB limit, 200 lanes and 2,000 packets.
Traversal, user-created symlinks, nonregular files, duplicate JSON keys and
unsafe output paths are rejected. Output must be a distinct `.json` file in an
existing owned directory. Publication uses a synced temporary file and atomic
replacement. Errors preserve the last output; continuous mode retries after its
interval. Missing lane reports produce explicit pending observations. Run only
one observer for a given runtime output; this utility is not a multiwriter
coordination system.

Validate offline:

```sh
rtk cargo test -p admiralty-tools rollout_observe
```

This is a transient bootstrap bridge. Retire its process and legacy manifest
publication when Rust's authenticated node observations, durable event stream
and runtime read projections own this deployment scope. Drain it before enabling
another manifest writer; its observations confer no scheduler or session custody.
