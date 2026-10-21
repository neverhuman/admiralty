# Fleet operations and structured observations

Open **Fleet operations** or `#/operations` to read the cached producer diagnostics.
The page uses `/v1/legacy/detail/initial`, including the existing Git-health map.
It does not request the commit graph/history or trigger repository scans. An
installation without a producer feed reports unavailable diagnostics. Private
feeds require Session access; explicitly public feeds retain their existing policy.

## Reading the view

- **Needs attention** counts reports with proved actionable findings, including
  partial reports that also contain known problems.
- **Incomplete evidence** includes missing/invalid reports, unknown state,
  observation errors, shallow/truncated graphs and missing parents.
- **Source clock gaps** counts stale, future or unknown Git report clocks.
  Staleness means older than the greater of three collection intervals and
  180 seconds. A clock over 30 seconds ahead is future. Invalid/missing clocks
  remain unknown. This presentation policy does not change the Git evaluator.
- **Policy disabled** keeps unconfigured ancestry analysis separate from healthy
  reports. No integration branch is inferred.

These counts overlap. They are not a health percentage or a merge-safety score.
The fleet publication clock and each repository observation clock are shown
separately. Fresh publication does not refresh an old Git report.

Search and filters affect the table only. It renders 50 projects per page, ordered
with actionable and incomplete evidence first; **Open Git** retrieves the existing
project investigation view. On phones, project records stack with visible field
labels and retained table headers for assistive technology. Capacity shows
observed reachability, one-minute load, CPU count and available GiB.
Missing measurements/warnings remain unknown. At most
64 host summaries render, with the omitted count visible.

The page polls every 15 seconds after the preceding bounded request finishes.
Each request has a 12-second deadline. Hidden pages stop starting new polls;
becoming visible resumes them. Auth denial stops automatic retries and clears
private evidence. Changing the in-memory credential immediately scopes out the
old result. Transient failure retains the last snapshot with an error notice and
its unchanged clocks. There is no additional native fleet/SSE subscription for
this page.

## Agent brief

**Copy brief** copies an explicit, lossy decision summary; **Inspect agent JSON**
also works without clipboard permissions. It always summarizes the entire source
inventory, independently of table search, filter or page.

The version-1 packet includes original publication time, overlapping counts,
up to 20 exception records, report observation times/fingerprints, bounded reason
codes, and a count of omitted exceptions/codes. An identifier longer than 128 UTF-8
bytes is omitted from the exception list and included in the omitted count, rather
than silently shortened into an ambiguous identity. Oversized optional clock or
fingerprint strings are null. Transcripts, commit subjects, private paths, free-form
issue details and credential fields are not copied. Full evidence remains in Git
Flow. The byte count is a UTF-8 measurement, not a provider token count or a
compression/fidelity guarantee. The packet grants no task acceptance or action.

## Observer JSON lines

The observer defaults to compact JSON lines for successful collection/publication
and collection failures:

```sh
admiralty-observe --config /absolute/private/observe.json --once
admiralty-observe --config /absolute/private/observe.json --log-format text
```

Successful records have `schema_version:1`, `event_name:observer.snapshot_published`,
severity, original source and collector clocks, interval, project/report counts,
Git-state counts, a health digest/change marker, collection availability and an
evidence filename. They contain no project names, tails or free-form failure text.
Collection availability describes whether that adapter returned; it does not
establish complete token/cost coverage. Read the actual measurement coverage from
`status.json` before using a total.

The health digest depends on sorted project IDs, repository instance identities,
health fingerprints and states. It excludes timestamps/timings, so routine clock
churn does not create changed evidence. `health_changed` compares only with the
preceding successful JSON publication in this observer process; the first report
after restart is changed. This is not a durable notification/delivery receipt;
the existing Git-health captain-alert deduplication remains independent.

`collector.stages_ms` measures fleet, projects, Git collection/evaluation, usage
and history wall times. `duration_ms` is elapsed time before final source
scrubbing/serialization/publication. These stages do not sum to the whole cycle:
hierarchy/projection work and prior-snapshot reading are outside them. They are
not HTTP latency, CPU utilization or storage durability measurements.

Failures emit `observer.cycle_failed` with a fixed reason and `retrying`; a failed
cycle never emits a successful publication record. Startup/fatal errors and
`--validate` keep their existing diagnostic output. Select `--log-format text`
for existing prose-log consumers. JSON field names are intentionally readable.

## Python-free preview credentials

```sh
admiralty preview-init --state-directory /absolute/private/admiralty-preview
```

This Rust command prepares private parent directories and an owner-only
`dashboard-token`, or validates an existing one with the daemon's actual reader.
It returns paths and creation state, never the credential. On Unix it traverses
pinned no-follow directory descriptors, rejects unsafe ancestry/symlinks,
preserves existing permissions and tokens, and syncs newly written credentials.
Other platforms refuse this unqualified private initialization path.

It does not start a service, open a Store, change configuration, enroll nodes or
install execution grants. Invalid/partial existing credentials are refused; they
are never silently rotated. `pnpm live` now invokes this command instead of inline
Python. The shell launcher still builds from source; the distribution workflow
remains the explicit Rust stage/archive/private-install path and its documented
release gates.
