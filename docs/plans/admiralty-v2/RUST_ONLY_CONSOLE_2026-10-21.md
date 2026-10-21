# Rust-only repository, single-binary console and chart-room redesign (21 October 2026)

Study and implementation notes for the request:

> We want this to be 100% Rust/Vite/TypeScript/React. Also, we want a clean
> deployment. Look for feature upgrades, performance upgrades, improved logging
> (prepared for agent consumption, context-smart and compressed). Consider git
> health metrics and all of the live data: what else should we be tracking? How
> can we improve layout and design, how can we produce a major upgrade?

Three open pull requests were already answering parts of this request when this
work started: **#168** (fleet operations view, compact observer logs, Rust
preview start-up), **#169** (Rust telemetry, agent brief, systemd deployment,
Fleet health) and **#171** (agent-ready telemetry with log digests, Git hygiene
metrics, health-gated rollout, Fleet health; it has since absorbed parts of
#168 and #169). All three stop at 38 of 42 Python files and none attempts a
redesign. The owner chose to **complement** them rather than add a fourth
competing implementation, so this change:

- takes the repository to **zero Python** and replaces every shell script that
  carried logic;
- makes deployment **one binary** that embeds the console;
- delivers a **design-system redesign** of the console;
- adds the one live signal the owner asked for that nobody had wired:
  **jankurai scores per repository**;
- leaves logging, `/readyz`, `/metrics`, Git hygiene metrics and systemd units to
  #171 (see "Logging" and "What to track next" for how they fit together).

## 1. Language inventory: before and after

| | `main` (2f5b931) | This change |
| --- | --- | --- |
| Python files | 42 (11,802 lines; 22 of them under `tests/`) | **0** |
| Shell scripts | 25 (`scripts/check/*.sh`, `scripts/dev/*.sh`, `tools/*.sh`, `tools/bin/*`, `captains/super_captain_tick.sh`, `dashboard/run.sh`, plus the test double) | **2**, neither with logic: the test double `tests/qualification/codex/P08Q_fake_codex.sh` (a fake provider executable for qualification tests) and `tools/security-lane.sh`, now a one-line `exec cargo xtask security` kept because Jankurai looks for that path |
| Hand-written JavaScript | `scripts/contracts/generate.mjs`, `scripts/demos/*.mjs`, `packages/contracts/runtime.test.mjs`, `dashboard/index.html` (57 KB page with inline JS) | **0**; scripts are TypeScript run directly by Node 24, the static page is retired |
| Generated JavaScript | `packages/contracts/generated/validators*.mjs` | unchanged (generator output, embedded byte-for-byte by the Rust release stager) |

The `python_policy` test now enforces the end state: the repository must contain
no Python at all, no new shell script, and the two remaining scripts must keep
their reviewed content (see `tests/integration/python_policy.rs`).

### Where each script went

See the per-area tables in the pull request description. In short:

| Area | New home | Command |
| --- | --- | --- |
| Super-captain audit, ticker, native queue health (`tools/sc_audit.py`, `tools/native_queue_health.py`, `captains/super_captain_tick.sh`) | `crates/admiralty-audit` | `admiralty sc-audit`, `admiralty sc-tick`, `admiralty queue-health` |
| Screen and fleet tooling (`tools/bin/*`, `tools/*.sh`, `dashboard/globalres.py`) | `crates/admiralty-fleetops` | `admiralty fleet snap|send|classify|status|kick|peek|paste|deliver|probe-models|codex-luna|resources|sweep` |
| Release and smoke qualification (`scripts/release/*.py`, `scripts/check/*-smoke.py`, `tests/qualification/*.py`) | `crates/admiralty-qualify` | `admiralty release source-package|qualify-native|evidence`, `admiralty-canary` |
| Rollout, history and OpenCode tools (`tools/rollout_*.py`, `tools/export_history.py`, `tools/history_export_cron.sh`, `tools/probe_opencode.py`) | `crates/admiralty-tools` | `admiralty rollout …`, `admiralty history export|publish`, `admiralty opencode probe` |
| Legacy collector stack (`dashboard/*.py`, `tests/legacy/*`, `tests/fixtures/legacy/verify.py`) | already replaced by `admiralty-observe`; parity tests added there | — |
| Local CI, supply-chain lane, demo and live launchers | `crates/admiralty-xtask` | `cargo xtask check|security|demo|live|dist` (`pnpm check|demo|live` still work) |
| Contract generator, demo capture/encode/verify | TypeScript under `scripts/` | `pnpm contracts:generate`, `pnpm demo:*` |

Every Python test case was ported to a Rust test with the same assertions and a
`/// Ported from …` comment, or is listed with the reason it no longer applies.
Porting found real defects in the Rust observer that the Python tests had been
guarding; they are fixed (section 6).

## 2. Clean deployment: one binary

- `crates/admiralty-server/build.rs` embeds a built `apps/dashboard/dist` when
  `ADMIRALTY_EMBED_DASHBOARD` names it. Ordinary builds embed nothing.
- `admiralty serve` (same flags as `admiraltyd`) and `admiralty observe` (same
  flags as `admiralty-observe`) make the CLI a complete deployment; the
  standalone executables remain for existing units.
- `cargo xtask dist` produces `bin/admiralty` (+ compatibility executables),
  `SHA256SUMS` and `BUILD.json` with the embedded dashboard's tree digest.
- Embedded assets are served from memory with build-time gzip, strong ETags and
  `304` revalidation; hashed assets stay `immutable`.
- `/legacy/` redirects to the React Fleet detail view when no retained page is
  configured, so the 57 KB hand-written page could be deleted without breaking
  links.

How this composes with #171: its hardened systemd units, `release
activate|rollback` and `/readyz`-gated rollout operate on whatever binary is in
the version directory. With an embedded console, "activate" switches the UI
together with the server, which removes the per-version asset caveat #171 had to
document. Recommended order: land this change, then point #171's units at
`admiralty serve`/`admiralty observe`.

See [single-binary deployment](../../operators/single-binary.md).

## 3. Design: the chart room

Details are in [console design](../../architecture/console-design.md). The
summary:

- **One shell** for every view (previously three: a 244 px sidebar, a 62 px rail
  and a 64 px rail with different top bars). A labelled 88 px rail grouped as
  Observe / Operate / Configure; canvases collapse it to icons with their
  original geometry.
- **Tokens** for colour, type, spacing and radius, in night-watch and day-chart
  themes taken from nautical charts. Magenta is reserved for what the operator
  can act on; status colours are always paired with words or shapes.
- **Atkinson Hyperlegible** (self-hosted, OFL) chosen for unambiguous glyphs in
  hashes and counts; tabular numerals everywhere.
- **Sentence-case labels** replacing all-caps monospace eyebrows; copy rewritten
  where tests did not pin the original.
- "Legacy detailed view" is now **Fleet detail**: it is the production default
  view, and nothing about it is legacy any more.
- A useful empty state when a server has no producer feed.
- The browser journeys' axe audits (zero violations and zero incompletes
  required) pass on every surface they audit, in both themes and at mobile
  width, except one live-views case that fails identically on `main` in the
  same environment (see the pull request).

Roadmap for the next design steps (not in this change):

1. **Bridge** home view: one screen answering "what needs me now" across
   projects (attention queue, stalled lanes, red CI, idle GPUs, overdue ETAs),
   fed by `admiralty sc-audit` findings plus #171's Fleet health ranking.
2. A per-project **timeline** strip (commits, merges, CI runs, task transitions,
   agent sessions) under each Fleet detail card.
3. A command palette (#169/#171 implement one; keep a single implementation).
4. Persisted theme and density per browser (#169/#171 implement theme
   persistence; it needs the e2e harness to reset storage between tests).
5. Split `CommandJourneys.tsx` (4,303 lines) along the seams listed in the
   survey: types, digest, parsers, API and replay, intake domain, derivations,
   one file per journey.

## 4. Performance

Done here:

- Entry chunk **521 KB → 348 KB** (gzip 122.6 KB → 106.7 KB): the generated
  contract validators load with the first native fleet request instead of with
  the shell.
- Embedded console: no filesystem reads per asset request, precompressed bodies,
  ETag/304 for `index.html`.
- Observer: `admiralty observe` handles SIGTERM between cycles, so a systemd
  stop never interrupts a publication.

Recommended next (owned by the integrator or already in #169/#171):

- Server snapshot projection cache and ETags for `/v1/legacy*` (#169).
- Concurrent Git scanning in the observer (#169).
- Store hot paths: per-event reducer clone, snapshot deep clone on the writer
  thread, `events_after` N+1 (documented in #169).
- The `/v1/legacy` reader still caps sources at 8 MiB while the observer writes
  up to 32 MiB (#169 fixes this; it should land regardless of the rest).

## 5. Logging for agents

#171 provides the right foundation: one `ALOG/1` line format across services,
per-target filters, burst coalescing (`x=N`), redaction and a token-budgeted
digest (`admiralty logs digest`, `GET /v1/logs/digest`). Recommendations so the
new Rust tools in this change fit into it rather than inventing another format:

1. Use the `admiralty-telemetry` writer in `sc-audit`, `fleet kick|deliver`,
   `history publish` and `xtask check`. Each already produces one structured
   record per action (audit findings, deliveries, stage results); emitting them
   as `ALOG/1` events makes them digestible with the same budget.
2. Make `sc-audit` the first-class agent input: its `latest.json` already ranks
   must-act findings. Expose `admiralty sc-audit --format alog --budget-tokens N`
   so the super-captain reads 300 tokens instead of `latest.md`.
3. Keep identifiers stable (`code`, `scope`, `fingerprint`) so digests can
   deduplicate across cycles, and keep source times so stale facts stay visible.

## 6. Defects found while porting

The parity work exercised the Rust observer with the regression cases the
Python suite had protected. Fixed here, each with a test:

- Rollout manifests: a non-list `lanes`, a non-integer `stale_after_s` and a
  non-object manifest combined with a lanes file were accepted (or panicked).
- Rollout links: `javascript:` URLs and URLs with embedded credentials were
  published; now only `http(s)` links without user info pass.
- Rollout lanes: two lanes resolving to one screen both adopted it; a screen
  owned by another project could be adopted.
- Lane source age was lost for merged manifests.
- Major-release receipts: `echo PASS` counted as an executed check; a receipt
  verified before its commit existed was accepted; `exit_code : 1` headers were
  ignored.

Follow-ups reported by the parity work (not changed, to keep this change
reviewable): receipts with no stated `command` are still accepted; a cargo
summary with `M failed` > 0 is still accepted when no Python test covered it;
any `ok. 0 passed` summary rejects a whole log; `kick --dry-run` still writes
state. Full lists are in the pull request.

## 7. What else to track

Live data that exists today: project ledgers and stages, Git graph and health,
hosts (load, memory, disk), screens and models, token usage and yield, history.
#171 adds Git hygiene metrics, PSI, disk growth and phase timings. This change
adds jankurai scores. Still missing, in priority order:

| Signal | Why it matters | Source |
| --- | --- | --- |
| **Jankurai score per repository and trend** (added) | The team's coding standard; regressions should be visible the cycle they land | `.jankurai/repo-score.json` (observer reads it; never runs the audit) |
| **Must-act audit findings over time** | `sc-audit` already computes stalls, idle screens, overdue ETAs, red main CI; trending red/amber counts shows whether supervision is working | `sc_audit/audit.jsonl` |
| **Cost per accepted task** | Token burn only matters relative to accepted outcomes | token yield + acceptance receipts |
| **PR lead time and review latency** | Where work waits: on agents, CI or the owner | GitHub API (open → first review → merge) |
| **CI health per head** | Pass rate, queued-without-runner minutes, retry-to-green rate | Actions runs; `sc-audit` already detects queued-stuck checks |
| **Flaky test register** | Pass-after-fail on the same SHA with identical commands is flakiness, not progress | receipts (`pass_after_fail_same_cmd` exists in the audit) |
| **Revert and fix-up rate** | Quality of merged work | first-parent history (#171 counts reverts/fix-ups per window) |
| **Agent session lifecycle** | Start, crash, restart, quota stop, idle time per lane | screens + provider logs |
| **Provider quota headroom** | Time to exhaustion per provider and model | quota history |
| **Native queue depth and age** | Persisted pending messages are not consumption | `admiralty queue-health` |
| **GPU utilisation timeline** | P1 GPU work idles silently today except in the audit | `nvidia-smi` samples (audit already samples) |
| **Build cache and target sizes** | Disk-full incidents come from `target/` growth | `du` of configured worktrees |
| **Owner interventions** | How often the fleet needs the owner, by cause | coordination logs (`STALL_PLAN_CHANGE`, escalations) |

Git health metrics worth adding beyond #171's set: PR age distribution, merge
queue wait, protected-branch force-push detection, signed-commit ratio,
agent attribution per commit (`Agent-Session` trailers), change size
distribution, and test-to-code ratio of merged changes.

## 8. Validation summary

See the pull request for exact commands, counts, platforms and the browser
results, including the two pre-existing local failures that also fail on `main`
in the same environment.
