# Rollout progress on the existing dashboard

The existing dashboard can display a project's implementation rollout alongside
its current captain cards. This is a read-only bridge: it reads one bounded JSON
manifest, computes counts, and shows pending packets, execution lanes, receipt
references, source freshness, and an optional integration PR. Project and node
names come from the manifest. No captain, Screen session, provider, or repository
command runs as a result of loading or rendering this panel.

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

## Collector wiring

`dashboard/rollout.py` exposes `collect_rollout(runtime_dir)`. The integration
owner adds the following to `dashboard/collector.py` immediately before the
final `wj(HIST, HISTORY)` and `wj(OUT, out)` calls in `cycle()`:

```python
from rollout import collect_rollout

rollout_state = collect_rollout(BASE)
if rollout_state is not None:
    out["rollout"] = rollout_state
```

The import can live at module scope. `dashboard/index.html` handles the optional
`rollout` key and hides its section when absent. Current status polling and
static snapshots continue to use the same source document. Deployment uses the
existing operator procedure for the collector; loading this module does not
restart or change services itself.

## Offline validation

From the repository root:

```sh
rtk proxy python3 -m unittest discover -s tests/legacy -p test_rollout.py -v
```

Tests cover missing and explicit sources, invalid JSON and schemas, duplicate
IDs, byte limits and special files, null paths, immutable manifest reads,
source age, clock skew, claimed/verified/integrated provenance, mismatched
revisions, self-verification, unsafe URLs, arbitrary node names, and actual
JavaScript rendering with hostile strings. JavaScript checks use Node when
installed and are explicitly skipped otherwise. No live collector import,
network, Screen access, provider credentials, or remote service is required.
