# Fleet health and compact agent diagnostics

Open **Fleet health** (`#/health`) to scan the existing producer feed. The page
uses the same access boundary as Fleet detail, including explicitly public
loopback observations. Review findings and coverage are independent: incomplete
ancestry can coexist with known conflicts. Dirty worker checkouts can be normal
work in progress. A healthy report is an observation, not merge approval.

The producer and each Git report retain separate clocks. More than 180 seconds
old is stale; future and invalid clocks have separate labels. The browser
recomputes age from the original timestamp, including after an HTTP failure.
Requests are serialized with a 12-second deadline and a 15-second interval while
visible. Leaving the page stops polling; changing session access hides data
from the previous session immediately. A denied/revoked bearer clears retained
health data and stops the visible-page retry timer until access or Retry changes.

## Agent brief

`GET /v1/legacy/brief` returns at most **16,384 UTF-8 JSON bytes**, before optional
gzip. It uses the existing authentication/public-read policy, read semaphore,
blocking file-I/O worker and `no-store` response policy. It runs no Git command,
inference, Store mutation or provider request.

For an existing local source file:

```sh
admiralty brief --status /absolute/producer/status.json --max-bytes 4096
```

The budget accepts 2,048–16,384 bytes, including all JSON metadata. No extra
newline is emitted. `--now 2026-10-21T12:00:00Z` fixes the evaluation clock for a
reproducible offline comparison; it does not refresh observations.

The version-1 operational projection contains:

- `source_sha256`: identity of the original feed bytes, including its clock.
  This is not a semantic-change digest or a publisher/approval attestation.
- Source clock, staleness threshold, the source's collector duration and fixed
  usage/history diagnostic codes. Collector duration is not measured HTTP p95.
- `totals` over the union of project, Git and Git-health keys. Git state counts
  partition that scope. `git_review_projects` separately counts reports with
  known attention findings, even when overall coverage is unknown. Checkout
  totals cover parsed reports; missing reports are not zero measurements.
- Nullable decimal-string `total`, `done` and `blocked` counters, preserving
  unsigned 64-bit precision and the producer's meaning, not native acceptance.
- Project Git state/clock, observed branch/divergence counts, dirty/conflicted/
  unknown checkout counts and grouped fixed issue codes.
- At most 64 project rows. Conflicts come first, then attention, unknown/stale
  coverage and other rows, with deterministic ID ordering. `omitted_projects`
  declares every excluded row. Aggregates are computed before row selection.

Inputs are bounded to 8 MiB and 2,000 project keys. Malformed/oversized sources
fail; invalid Git reports become unknown. Unsafe/long IDs become SHA-256 labels.
Imported paths, commands, screen tails, commit messages and free-text errors are
excluded. This allowlisted projection is not a general-purpose secret scrubber.
Read full evidence whenever omissions or unknowns affect the decision. The brief
does not join protected native tasks/incidents into a public producer response.

## Structured observer logs

```sh
admiralty-observe --config /private/observe.json --log-format json
```

Each successful cycle emits one compact JSON line on stdout: version, fixed
`observation.published` event, source observation time, collector duration,
published bytes, project/report coverage, Git state counts, changed-alert count
and usage/history error booleans. Failures use fixed codes on stderr without
imported error prose. `--validate` also emits JSON. Text remains the default.

Archive with ordinary stream/journal compression. Give agents the brief and
retrieve detail for the specific decision. Byte reduction is not measured
provider-token savings. Existing persisted Git-health fingerprints still dedupe
unchanged alerts; logs remain available independently for diagnosis.

## Native live startup

`pnpm live` builds the dashboard, then invokes `admiralty live`. For an already
built/installed bundle:

```sh
/absolute/release/bin/admiralty live \
  --state-dir /private/admiralty-preview \
  --status /absolute/producer/status.json \
  --index /absolute/release/legacy/index.html \
  --assets /absolute/release/dashboard \
  --listen 127.0.0.1:18891
```

This runtime launch needs neither Python, Node, pnpm nor Cargo. Source builds
and retained compatibility qualification have separate tool requirements.
Optional `--models /private/models.json` preserves model configuration without
enabling general managed execution.

The command requires loopback and existing producer/index/build files. Rust
creates owner-only state and refuses public directories, symlink ancestors,
invalid/public tokens and multiply linked credentials. It never changes existing
permissions or rotates credentials. OS-random token bytes are synced before
serving; only the file path is printed. Interrupted creation can leave a partial
token that is refused pending explicit local reconciliation.

This is the existing read-only producer preview, retaining `/legacy/` and
session-gated native reads. Protected native startup, provider admission and
production service ownership retain their own configuration and qualification.
Source changes and isolated tests do not change a primary service.
