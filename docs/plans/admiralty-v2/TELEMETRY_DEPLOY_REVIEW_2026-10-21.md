# Upgrade review, 21 October 2026

Scope: a close review of `main` at `5916726` for four goals: a 100% Rust and
Vite/TypeScript/React product, a clean deployment, compact logs that agents can
read cheaply, and better live data, Git health and layout. This file records
what the accompanying PR changed, what it measured, and what should come next,
ranked by value per effort. Each recommendation names its data source, so it can
be scoped as a packet without re-reading the code.

## 1. What changed in this PR

| Area | Change | Guide |
| --- | --- | --- |
| Logging | New `admiralty-telemetry` crate (no dependencies beyond `serde_json`): `ALOG/1` one-line events, `ADMIRALTY_LOG` per-target filter, 60 s burst coalescing, secret redaction, and a token-budgeted digest (`admiralty logs digest`, `GET /v1/logs/digest`) | [deployment: observe](../../operators/deployment-systemd.md#4-observe) |
| Service | `/readyz`, `/metrics` (Prometheus text), `X-Request-Id`, per-route latency histograms, one `http.summary` line a minute, constant-time token check | same |
| Git health | Descriptive hygiene metrics (unmerged, merged, diverged branches; lag; dirty, conflicted, locked or prunable checkouts; cleanup candidates; integration activity over 24 h and 7 d, merges, reverts, fixups). Fingerprints are unchanged, so alerts do not re-fire | [Git health](../../operators/git-health.md) |
| Observer | Disk growth and time-to-full, PSI pressure, D-state processes, top memory consumers, per-phase cycle timings, change events, and Git and host series in history | [native observer](../../operators/native-observer.md) |
| Python | Four Python files (363 lines) and the `dashboard/run.sh` launcher retired; `scripts/dev/live.sh` no longer embeds Python. Rust now provides the release qualifier, the excursions smoke test, preview-state creation and the dashboard server | [legacy inventory](../../operators/legacy-inventory.md) |
| Deployment | Hardened systemd units, `admiralty release activate` and `rollback` (atomic symlink switch with a 0600 journal), `admiralty ready`, and a rollout script that rolls back automatically when readiness fails | [deployment with systemd](../../operators/deployment-systemd.md) |
| Dashboard | Fleet health page, grouped navigation, command palette, persisted theme, two new temporal plots, memoized derived data | [live console](../../operators/live-console.md#fleet-health) |

## 2. Live data now collected

Observer, per cycle: project ledgers and task stages, agents and models, Git
graph, refs, checkouts and Git health (with metrics), PR and CI evidence, token
usage and yield, and for each host: load, memory and swap, disks (with growth),
GPUs, PSI, blocked processes and top processes. History keeps bounded samples,
now including Git hygiene (`git: st, un, bh, dt, cc, c7`) and host memory and
disk-growth fields.

Service: request counts and latency by route template and status class,
live event streams, read permits, log volume by level, uptime, readiness and
store latency.

## 3. What else to track (ranked)

Each row says where the data comes from and where it should surface.

| # | Signal | Why it matters | Source | Surface |
| --- | --- | --- | --- | --- |
| 1 | **jankurai score per project and commit** | The team standard is enforced in pre-commit; a trend shows drift before review does | Score file written by the pre-commit hook beside each commit, read by the observer like CI evidence | Git hygiene column, temporal plot, attention finding on regression |
| 2 | **PR flow**: open-PR age, time to first review, time to `neverhumanbot` approval, time to merge, head-mismatch rejections | Merge latency is the fleet's real throughput limit | GitHub REST (`pulls`, `reviews`, `events`), already reachable from the observer's PR evidence path | Git hygiene table; finding when PR age exceeds a policy |
| 3 | **CI health**: pass rate per job, retry-to-green (flaky) on the same SHA, queue wait, p50/p95 duration, local `pnpm check` stage times | Flaky or slow checks cost more agent time than any single bug | Actions runs API; `scripts/check/all.sh` stage logs (time each `run`) | Service panel and temporal plot |
| 4 | **Cost per outcome**: credits per merged PR, per accepted task, per accepted line changed; retries and escalations per task | Token burn only means something per delivered result | Existing token-usage samples joined with ledger acceptance and merges | Token panel; replaces `credits_per_done_todo` where acceptance is known |
| 5 | **Dirty and diverged durations** (first-seen time per dirty checkout and per lagging lane) | "Dirty now" is noise; "dirty for 3 days" is a stuck agent | Observer keeps first-seen timestamps keyed by checkout and branch | Git hygiene `since` column; finding past a threshold |
| 6 | **Agent idle and stuck time**: last output age, open task with no commits | Separates waiting on review from stuck | Screen/process observations and ledger timestamps | Hierarchy badges; attention finding |
| 7 | **Store health**: SQLite file and WAL size, checkpoint time, write latency p95 | The store is the only stateful dependency | `admiraltyd` gauges next to the `/readyz` probe | `/metrics`, Service panel |
| 8 | **Build identity**: add the source commit to the existing `admiralty_build_info{component,version}` gauge, and the observer version to `status.json` | Proves which bundle is serving after `rollout.sh` | Compile-time constants | Service panel header |
| 9 | **Host reliability**: inode use, OOM kills (`/proc/vmstat oom_kill`), failed systemd units, clock offset, journal size, NVMe wear | Common silent failure modes on build hosts | Extra lines in the Linux probe, using the same parser pattern as PSI | Host cards and warnings |
| 10 | **Supply chain**: `cargo deny` and `pnpm audit` advisories, outdated pins | Cheap to collect, expensive to ignore | Local CI stage output | Attention finding |
| 11 | **Python burn-down**: remaining files and lines (38 and 11,439 today) | Keeps the 100% Rust goal measurable | `tests/fixtures/python-baseline.txt` | Temporal plot |

## 4. Remaining Python, in suggested port order

1. `scripts/check/canary-smoke.py` (1,124 lines): a Rust integration test
   beside `excursions_binary.rs`, using the same real-binary harness.
2. `scripts/release/package.py` (609): move into `admiralty release` next to
   `qualify`, `activate` and `rollback`.
3. `dashboard/collector.py`, `extras.py`, `rollout.py`, `token_yield.py` and
   `globalres.py`: compare each with the Rust observer and port what is missing.
   `globalres.py` already has Rust parity in `resources.rs`; delete it once
   `COMMON.md` consumers switch to the Rust path.
4. `tools/sc_audit.py` (1,828) and `tools/rollout_*.py`: port as CLI
   subcommands; their tests move with them.
5. `tests/legacy/*`: retire with the code they test.

## 5. Layout and design, next steps

- **Make Fleet health the home view** once it has run in production for a week;
  the detailed view stays one keystroke away (Ctrl K).
- **Mark events on time series**: draw deploys (from the activation journal),
  incidents and Git health changes on the temporal plots, so cause and effect
  share one axis.
- **A project page** (`#/project/:id`): ledger, Git hygiene, PRs, CI, agents and
  cost for one project, linked from every project name.
- **Replace Unicode navigation glyphs** with one small SVG icon set; glyph
  rendering varies by platform font.
- **Split the main bundle** (527 kB, 125 kB gzip): `LegacyDetailed` and
  `TemporalPlots` load eagerly on every route; Fleet health already loads
  lazily (23 kB).
- **Notify on new critical findings** (title badge or desktop notification,
  off by default).

## 6. Rollout notes for this PR

- Deploy `admiraltyd` and `admiralty-observe` together. `GitHealthReport` gained
  an optional `metrics` field; older readers ignore it, but the dashboard's new
  columns need the new observer.
- The observer no longer prints `Rust observation published` on stdout; use
  the `obs.cycle` event (`ADMIRALTY_LOG=info`) instead.
- No services were restarted and nothing was deployed while this was prepared.
