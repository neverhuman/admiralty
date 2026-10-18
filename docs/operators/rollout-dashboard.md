# Rollout progress on the existing dashboard

The existing dashboard can display a project's implementation rollout alongside
its current captain cards. This is a read-only bridge: it reads one bounded JSON
manifest, computes counts, and shows pending packets, execution lanes, receipt
references, source freshness, and an optional integration PR. Project and node
names come from the manifest. Loading or rendering the manifest executes no
manifest-supplied commands. The collector can additionally join its actual local
Screen/process observations to declared rollout lanes using the fixed read-only
collector operations described below; it never sends terminal input.

No manifest means no panel. Existing dashboard installations continue to work.
This bridge does not replace the durable scheduler, claims, receipt verification,
or runtime authority planned for the Rust control plane.

## Configure the source

By default, the collector reads `rollout.json` from its `CDASH_RUNTIME` directory.
Alternatively, set `ADMIRALTY_ROLLOUT_MANIFEST` in the collector's environment to
the full path of a manifest produced by the rollout exporter. Keep this file and
all execution evidence outside Git. Prefer a private runtime directory with
permissions appropriate for the dashboard's existing exposure.

The configured file is the only file this loader opens. Capsule and receipt
paths are displayed as text. They are never followed, linked as local files,
executed, or used to read private task content.

Publish by writing a temporary file in the destination directory, flushing it,
and atomically replacing the manifest. Do not rewrite the live file in place.
The packet exporter uses generation directories for capsules and atomic
publication of the root manifest.

An absent default file hides the panel. An explicitly configured missing file
shows an error. Malformed JSON, unsupported schemas, duplicate IDs, unsupported
statuses, special files, and files larger than one MiB show a bounded error;
the other dashboard sections remain available. Errors never include file
contents or full private paths.

## Manifest version 1

Minimal source, using arbitrary project and node names:

```json
{
  "schema_version": 1,
  "project": {
    "id": "sample-build",
    "name": "Sample implementation",
    "repository_url": "https://example.com/team/repository"
  },
  "producer": "admiralty.rollout_packets/v1",
  "updated_utc": "2026-10-18T12:00:00Z",
  "stale_after_s": 300,
  "pr": {"url": null, "head": "candidate-commit"},
  "packets": [
    {
      "id": "P01",
      "title": "Foundation",
      "status": "in_progress",
      "lane": "foundation-worker",
      "depends_on": [],
      "capsule_path": "generations/example/capsules/P01.md",
      "claim": {"owner": "worker-a", "node": "build-node-a"},
      "verification": null,
      "integration": null,
      "evidence": []
    }
  ],
  "lanes": [
    {
      "id": "foundation-worker",
      "owner": "worker-a",
      "node": "build-node-a",
      "branch": "codex/foundation",
      "status": "running"
    }
  ]
}
```

`project.id`, `updated_utc` with timezone, and a `packets` list are required.
Packet IDs must be unique. Version 1 permits up to 2,000 packets, 200 lanes,
and 100 evidence references per packet. Producer strings, titles, lane fields,
and references are bounded before inclusion in the legacy status document.
Only HTTP and HTTPS links without embedded credentials become clickable.
Unknown fields are omitted from dashboard output.

Canonical packet statuses are `planned`, `ready`, `claimed`, `in_progress`,
`blocked`, `review`, `verified`, and `integrated`. The input alias `active`
normalizes to `in_progress`. Counts are recomputed from normalized packet rows;
the loader ignores source counters. Candidate receipts should produce `review`,
not `verified` or `integrated`.

`claimed` means a worker report; the collector never promotes a packet based on
its text, a process exit, a PR link, a capsule, or a candidate receipt. To display
verification recorded, `verification` must contain nonempty `receipt`,
`revision`, `verifier`, and `observed_utc` fields. To display integration
recorded, `integration` must additionally contain nonempty `receipt`, `revision`,
`target_ref`, and `observed_utc` fields. Receipt times require a timezone.

Missing verification provenance downgrades the display to claimed. Missing
integration provenance downgrades the display to verification recorded. An
explicit `candidate_revision` must match the verification revision; integration
and verification revisions must match each other. A verifier equal to
`claim.owner` cannot establish independent acceptance. Downgrades retain the
reported status and show a warning.

These are display guards. The legacy collector does not authenticate producers,
open receipt files, validate signatures, run tests, or check Git ancestry. Even
with complete provenance, the panel says **verification recorded** and
**integration recorded**, not independently accepted. Operational acceptance
must remain with the repository's authorized verifier and integration policy.

Freshness uses the manifest's `updated_utc`, never its file modification time.
Default stale threshold is 300 seconds; producers can choose an integer from
30 to 86,400 seconds. Source times more than 30 seconds ahead are marked
`future`. Re-export on meaningful changes or on a deterministic heartbeat;
no model turn is needed to refresh observational state. The panel also shows
producer, filename, load time, file modification time, and SHA-256 of the exact
bytes read. A file hash provides content identity, not producer authentication.

## Collector wiring and live worker observations

`cycle()` reads `collect_rollout(BASE)` before `extras.collect`. The pure
`observe_rollout_workers` helper joins the normalized project's lane screen
references to that same collector cycle's actual local inventory. Its additive
`extra_winfo` mapping is passed to extras, which preserves producer-owned IDs
and derives models and worker states from the existing observed processes.
No synthetic producer project is added. The existing producer projects,
CAPTAINS configuration, packet counts, claims, receipt evidence and source
freshness stay unchanged.

A lane may name a bounded exact Screen reference in `screen`, either a session
name or `PID.name`. Optional `screen_pid` and `reported_screen_pid` fields must
be null or integer PIDs from 1 through 2,147,483,647. All supplied pins, including
a PID embedded in `screen`, must match the actual inventory ID. Invalid pins
fail manifest normalization. A recycled name with a different pinned PID never
adopts the replacement process. Duplicate names require an explicit matching
PID; multiple lanes aliasing one actual session are excluded from the join.

The default captain reference is the existing `captain_<project.id>` naming
convention. A project may instead set `captain_screen`, `captain_screen_pid`
and `reported_captain_screen_pid`. No project or node name is hardcoded in the
join. A declared different node is not observed through local screen names.
A rollout project ID colliding with an existing producer project is excluded;
existing producer screen IDs and names cannot be adopted by a rollout lane.

The collector invokes its trusted `screen_info` callback only with verified
actual inventory IDs, at most once per unambiguous session. It shares process
probes across screen capture and extras during one cycle. Hardcopies follow
the existing worker-capture cadence. Captures use fixed argument vectors and
the collector's existing output cleaning; the manifest supplies no executable
program, command, filesystem path or terminal input. Future screen-change
timestamps cannot establish activity. Rollout sessions need a second distinct
captured frame before the first `screen_changed_utc` is recorded: screen
presence and the first capture alone cannot create a green activity dot.

`rollout.live_observation` separately records bounded captain/worker observations
with actual ID/PID, alive and agent-process flags, model/effort, cleaned tails,
reported packet assignment, and identity state. Unknown inventory, absent
screens, mismatched PIDs, aliases, collisions, supervisors without a detected
agent and capture failures remain explicit. A screen with no agent stays
finished rather than running; a detected agent without observed screen change
stays idle. Agent process presence or screen changes do not prove task
progress or acceptance. Extra observations do not refresh the original
manifest timestamp or its claims.

`dashboard/index.html` and the React legacy adapter consume the same source
document. Deployment requires the collector, rollout and extras modules from
one pinned candidate; loading these files never restarts services itself.

## Offline validation

From the repository root:

```sh
rtk proxy python3 -m unittest discover -s tests/legacy -p test_rollout_live_workers.py -v
rtk proxy python3 -m unittest discover -s tests/legacy -v
```

Tests cover missing and explicit sources, invalid JSON and schemas, duplicate
IDs, byte limits and special files, null paths, immutable manifest reads,
source age, clock skew, claimed/verified/integrated provenance, mismatched
revisions, self-verification, unsafe URLs, arbitrary node names, and actual
JavaScript rendering with hostile strings. JavaScript checks use Node when
installed and are explicitly skipped otherwise. No live collector import,
network, Screen access, provider credentials, or remote service is required.

Live roster tests use fake inventories and read-only callbacks. Collector and
extras functions are AST-loaded without their module-level filesystem side
effects. Tests cover PID bounds/mismatches, aliases, unknown and absent sessions,
project/producer collisions, configured captains, sanitized bounded tails,
first-frame activity suppression, per-cycle process caching, manifest-before-
extras wiring and preservation of the existing producer project counts. No
live Screen, provider, remote process or service is touched by these tests.
