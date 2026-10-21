# Native Rust observations

`admiralty-observe` reads configured project ledgers, Screen/process observations,
resource measurements and usage logs, then publishes a dashboard snapshot and
temporal history. It runs no model turns and uses no Python subprocess. The
observer does not launch agents, change tasks, enforce resource reservations,
verify candidates, merge code or grant native execution/acceptance authority.
The full execution controller has separate qualification and activation gates.

The React dashboard and the original detailed view can consume this producer
through the existing Rust server. Compatibility JSON keys such as `captain` and
`super_captain` retain their schema names; the UX calls these roles **Commodore**
and **Admiral**. Observation does not rename existing Screen sessions.

## Build and prepare an isolated candidate

Build an exact reviewed source revision in its owned build area:

```sh
rtk cargo build -p admiralty-observe --release --locked
```

Record the source revision, dirty-tree state, executed checks and binary hash.
Install the built binary into a new immutable release directory such as
`/srv/admiralty/releases/REVISION/bin/admiralty-observe`. Do not rebuild or replace
a binary in place while it is running. Treat `REVISION` as a deployment-selected
directory name, not a literal release identity.

Start from [the generic example](../../configs/examples/observe.json). Replace
paths, SSH aliases and project/session identities with operator-owned values.
Use an absolute configuration path. The configuration file must be an owned,
regular, single-link file with mode `0600`, inside a protected directory; use
mode `0700` for the private parent and output directory. Ancestors must be owned
by the service account or root, without writable unprotected directories or
symlinks. Use canonical paths on platforms where a familiar path is a symlink.
Do not put credentials in this JSON or make private snapshots world-readable.

The output directory's parent must already exist. Normal startup can create the
last output-directory component with mode `0700`; it does not recursively create
parents. Use a new candidate output directory, separate from the active feed.

```sh
rtk proxy /srv/admiralty/releases/REVISION/bin/admiralty-observe \
  --config /srv/admiralty/observe/config.json --validate
rtk proxy /srv/admiralty/releases/REVISION/bin/admiralty-observe \
  --config /srv/admiralty/observe/config.json --once
```

`--validate` parses the private configuration and checks its structure. It does
not connect to nodes, qualify source files, inspect output permissions or prove
that a cycle can publish. `--once` performs a real observation cycle and exits;
it can make configured read-only SSH/GitHub requests and writes only the owned
output area. Inspect its source timestamps, errors and unknowns before cutover.

For continuous operation, omit both flags:

```sh
rtk proxy /srv/admiralty/releases/REVISION/bin/admiralty-observe \
  --config /srv/admiralty/observe/config.json
```

There is no `daemon` subcommand. The continuous process stays in the foreground
for an operator-owned supervisor; [systemd user units](deployment-systemd.md)
are provided in `packaging/systemd/`. SIGTERM (and Ctrl-C) let the cycle in
progress finish and publish atomically before exit.

Each cycle writes one compact log event (`logfmt` by default, `--log-format
json` for NDJSON). Identical consecutive outcomes collapse into a counted
`repeat=N` line every `--log-rollup-seconds` (default 600; `0` writes every
cycle). See [agent brief and compact logs](agent-brief.md).

Fleet/project collection and Git scans run concurrently, and the previous
`status.json` is read once per cycle. `collector.stages_ms` publishes the
`fleet`, `projects`, `git`, `git_health`, `usage` and `history` durations; the
first two lanes overlap, so their sum can exceed `collector.duration_ms`.

Disk growth (`growth_gb_per_h`), `hours_to_full` and `rate_window_min` are
estimated from the observer's own used-byte samples, kept for six hours in the
private `disk-rates-v1.json`. The rate uses the oldest sample between 20 minutes
and 3 hours old, so these fields stay `null` for the first 20 minutes and after
gaps. Fewer than 24 (12) projected hours to full raise a disk to amber (red),
matching the retired Python estimate. Projections are linear advice, not
reservations or capacity guarantees.
Its advisory `collector.lock` prevents a second native observer from writing the
same output directory. It does not coordinate with old collectors. Keep a single
writer for every active feed and use separate output directories for candidates.

## Configuration

The loader rejects unknown top-level, project and node fields. All fields ending
in `_dir`, `_file`, `_source`, the rollout manifest, the repository and program
paths should be explicit absolute paths; do not use shell expansion or commands.
Optional paths can be omitted or `null`. Tracker paths are adapter-specific:
file adapters resolve relative paths from `repository`, while `major_release`
requires a repository-relative Git object path.

| Field | Meaning |
| --- | --- |
| `schema_version` | `1`. |
| `node_id` | Stable identity of the observer's own node. |
| `output_dir` | Private owned snapshot/history output directory. |
| `interval_seconds` | Requested cycle interval, 5–300 seconds; default 15. A slow cycle does not create fabricated intermediate samples. |
| `nodes` | Up to 32 `{id, ssh, os}` records. Use `ssh: null` only for the local `node_id`; all other records need explicit SSH aliases/targets. `os` is `linux` or `macos`. |
| `projects` | Up to 64 project records described below. IDs must be unique. |
| `admiral_screen` | Actual Screen name or full `PID.name`; no session is created. |
| `owner_name` | Optional owner display name, up to 100 characters without control characters. Defaults to `Owner`; it does not identify an authenticated principal or grant authority. |
| `coordination_dir` | Optional read-only coordination source containing compatibility-named `super_captain_status.json`, `super_captain_LOG.md`, `super_captain_DECISIONS.md` and, where present, `model_usage.jsonl`. |
| `rollout_manifest` | Optional version-one rollout manifest. Reported stages and provenance remain claims/observations. |
| `rollout_lanes_file` | Optional separate lane-report input; reported lane state cannot advance packet acceptance. |
| `codex_sessions`, `claude_projects` | Optional configured JSONL usage-log roots. Raw prompts and log bodies are not copied into metrics. |
| `rates_dir` | Optional directory of `codex_rates_*.json` rate files; the lexically latest matching file supplies `{models:[{model,input,cached_input,output}]}` rates per million tokens. Use the same credit unit throughout. Missing rates keep costs unknown. |
| `history_source` | Optional existing `history.jsonl` file, or directory containing it, imported without changing original sample timestamps. |
| `gh_program` | Optional absolute path to the trusted `gh` executable. Together with tracker `github_repo`, enables bounded read-only PR queries using already configured credentials. `null` disables those queries. |

A project record has `id`, `name`, absolute `repository`, a `tracker` object,
optional `manager_screen`, `worker_prefixes` and optional `coordination_log`.
Prefixes classify actual observed sessions; they do not launch workers. Avoid
overlapping project prefixes. Exact manager names or full Screen IDs are safer
than relying on coincidental worker-name matches.

Implemented tracker kinds are:

| `tracker.kind` | Required inputs and interpretation |
| --- | --- |
| `task_array` | `path` to a task array or `{tasks:[...]}` with unique `id` and `status`. Optional `progress_path` supplies ledger metadata/leases and `events_path` supplies JSONL events. Ledger “done” is a source claim, not independently checked acceptance. |
| `packet_progress` | `path` to `{packets:[{id,tasks:[...]}]}` and `progress_dir` containing `PACKET_ID.json` task states. Optional `validator_path` supplies the audit text. Missing progress stays unknown. |
| `markdown_table` | `path` to a Markdown task table. Optional `waves_dir` supplies existing wave results. Textual status remains a source observation. |
| `major_release` | Repository-relative Git `path` and optional local `ref` (default `origin/main`); optional `verify_dir`. Joins tracker claims with local Git/PR/live-worker evidence and bounded hashed receipt/log checks. It does not fetch Git refs or grant runtime acceptance. |
| `snapshot` | `path` to an existing JSON project object; optional `project_key` selects a key from `projects` or the top-level object. Retain its source provenance. This adapter imports an existing observation; it is not native measurement of its contents. |
| `rollout` | Classifies configured rollout manager/workers without creating a second project task ledger; the configured rollout manifest supplies the packet view. |

Adapters also accept optional `supplemental_path` and `pr_snapshot_path` for
existing structured observations. Configure only sources you intend to expose.
No tracker value is executed as shell code. Source read failures produce an
explicit unavailable/unknown project record, not a successful empty ledger.

## What the output establishes

The main feed is `output_dir/status.json`, atomically replaced by the Rust
producer. It contains project records, hierarchy, resource observations, usage,
history and collector metadata including `implementation: "rust"`, observation
time, duration, `model_turns: 0` and `acceptance_authority: false`. Other owned
files include `history.jsonl`, `collector.lock`, the private model-observation
cache, `usage-state-v1.json` and quota-history state when available. Brief owned Screen capture files
are private scratch data and are removed after their bounded capture batch.

`generated_utc` establishes publication time. It does not refresh a ledger's
source timestamp, a manifest's `updated_utc`, a receipt's revision or a previous
model observation. Rollout freshness uses the manifest's original timestamp and
threshold; future-dated sources remain visibly distinct. Check per-source age,
coverage flags, usage scan truncation and error fields as well as publication
age. An alive producer can still be reading old or unavailable inputs.

Resource probes are fixed, bounded, read-only local/SSH commands for configured
nodes. Failed SSH, unsupported measurements and incomplete Screen inventories
stay unknown. Linux process ancestry can corroborate an agent process; process
presence is not proof of useful work, task ownership or completion. The current
macOS path supplies resource/Screen inventory but leaves agent ancestry/model
identity unknown. Remote Screen names alone do not establish their model.

Model badges come from observed terminal footers. Requested CLI models, default
configuration and assignment labels do not establish actual identity. Local
footer captures prioritize Admiral and project managers and rotate through
workers with bounded concurrent captures. A private cache retains an original
`model_observed_utc` for up to 300 seconds only while the exact Screen ID, actual
agent PID, boot/process start identity and family still match. Changed identities
or expired observations become unknown. A footer remains terminal evidence,
not authenticated provider identity. Missing task/activity evidence stays
unknown; a static Screen is not acceptance.

History imports retain original timestamps and null measurements. New samples
are added at intervals of at least 110 seconds; restarts/repeated polls do not
fabricate samples. Retention is bounded to 4,096 rows/8 MiB. Archive the original
history separately before migration so this bounded display window does not
become the only copy. Missing token rates, cached-input counts or coverage do
not become zero cost. Cost index, token attribution and yield estimates retain
their accounting notes and cannot certify completed work.

Usage scanning persists bounded byte offsets, cumulative-counter baselines and
numeric usage events in the private `usage-state-v1.json`; it does not persist
raw prompts or complete log bodies there. Preserve that state with the owned
output when restarting the same qualified producer. On an initial scan of a
large/truncated log, after replacement/rotation, or after a coverage gap, full
window totals and costs remain null until a retained baseline covers the whole
window and all configured sources are completely enumerated. The 15-minute,
60-minute and 48-hour windows can therefore qualify at different times. Observed
`lower_bound_*` values remain separate and do not establish full totals. Inspect
`coverage_complete`, `usage_coverage_complete`, scan/source-inventory flags and
`state_persisted`; merely waiting for one interval does not establish coverage.

## Cutover and rollback

The deployment owner performs the service changes after qualification. Keep
existing project managers, task workers, repositories, worktrees, credentials,
other projects and shared fleet services untouched during observer replacement.

1. Inventory the exact existing writers, supervisor definitions, PIDs/start
   identities, listeners and dependencies. Archive active configuration and raw
   snapshot/history inputs with hashes and timestamps. Preserve failed evidence.
2. Run the candidate immutable binary against a separate private output area.
   Point `history_source` at an archived or read-only original history. Keep
   original source timestamps; do not rewrite a stale manifest to make it green.
3. Compare real candidate project/source identities, counts, role/model evidence,
   resource values, plot history and unknown/error states against the original
   inputs. Verify at least one new real history sample after its minimum sample
   interval. Record exact source/binary/config hashes and observed outcomes.
4. Validate the Rust server's candidate feed configuration. Its compatibility
   options are `--legacy-status-file` pointing to the new Rust `status.json` and
   paired `--legacy-index-file` pointing to the retained static detailed view.
   The option's historical name does not imply a Python producer. Keep existing
   authentication, assets, profile data and native configuration intact. See
   [native server startup](native-startup.md).
5. Review old launcher exit traps, restart policies, timers and cron before
   stopping anything; a stale-PID trap must not kill an unrelated process. Drain
   only the obsolete observation/HTTP writers in the approved deployment scope.
   Replace their owned supervisor entry with an absolute immutable Rust binary
   and explicit `--config` path; do not leave two publishers on the active feed.
6. Switch the server feed to the qualified candidate output and observe real
   refreshes in both React and the detailed view, including temporal plots.
   Confirm process executable paths and listeners are the intended Rust release;
   static HTML assets can remain without the old HTTP/collector processes.
   Reconcile history jobs individually so no recurring legacy publisher revives.
7. Record the old/new release identities, archived inputs, retired exact process
   identities, remaining dependencies and validation in a deployment receipt.
   If qualification fails, keep the active feed unchanged and preserve evidence.

For rollback, restore the previously recorded server-feed and supervisor
configuration through the deployment owner. Keep the candidate output and
evidence; never overwrite the original history or production checkout. A prior
collector must be deliberately restored as a single writer, not accidentally
revived by a leftover restart loop. Switching dashboard producers does not
activate the execution controller or prove all work packets accepted.
