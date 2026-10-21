# data/history: running history of the overnight captains build (xbabe2)

A sanitized, git-friendly export of the captains dashboard's history. It's here so external agents can study the cost index over time, Luna failures and escalations, in-progress and done work per project, agents by tier, quota burn, host load and disk, CI state, hygiene, and token yield.

- **Producer:** `admiralty history export` (Rust port of the retired `tools/export_history.py`; same files, columns, row order and bytes), which only reads the live runtime files.
- **Schedule:** hourly, via `admiralty history publish` (cron on xbabe2, minute 23; replaces the retired `tools/history_export_cron.sh`).
- **Time zone:** every timestamp is **UTC** (`...Z`). Integer `ts` fields are Unix epoch seconds. Jepson's local time is America/Denver (MDT = UTC-6).

## Layout

| Path | Rows | Source (on xbabe2, read-only) |
|---|---|---|
| `timeseries/history_YYYY-MM-DD.jsonl` | one dashboard snapshot each ~2 min (15 min for data older than 6 h) | `captains-dashboard/history.jsonl` |
| `timeseries/history.csv` | the same rows, flattened | same |
| `ledger/model_usage_YYYY-MM-DD.jsonl`, `ledger/model_usage.csv` | one line per model-attempt event, normalized | `captains-20261017/model_usage.jsonl` |
| `quota/codex_quota.csv` | Codex plan usage samples | `captains-dashboard/.quota_history.json` |
| `resources/disk_used.csv` | disk used bytes per host:mount, every ~2 min | `captains-dashboard/.res_history.json` |
| `resources/host_snapshots.csv` | one row per host per export (load, RAM, swap, disk, GPU) | `captains-dashboard/global_resources.json` |
| `tokens/codex_sessions.csv` | one row per Codex/Claude session (cumulative tokens) | `captains-dashboard/tokens.db` table `sessions` |
| `tokens/claude_hourly.csv` | Claude messages aggregated per hour × project × model | `tokens.db` table `claude_msgs` |
| `token_yield/token_yield_YYYY-MM-DD.jsonl` | one token-yield snapshot per export | `captains-dashboard/token_yield.json` |
| `snapshots/projects_YYYY-MM-DD.jsonl` | one compact project and hygiene snapshot per export | `status.json` + `super_captain_status.json` |
| `rates_index.json` | pointer, with sha256, to the rate files in `captains/rates/` | `captains/rates/*_rates_*.json` |

Several sources are rolling windows: history is downsampled after 6 h, disk history keeps 6 h, quota history keeps 12 h. The exporter **union-merges** each run with what's already committed, so this folder keeps the full record even after the live files roll over. Re-running is idempotent.

## Field dictionary

### timeseries (`history_*.jsonl`; CSV column names in brackets)

| Field | Unit / meaning |
|---|---|
| `ts` [`ts`], `ts_utc` [`ts_utc`] | snapshot time (epoch s / ISO UTC) |
| `cost_index` | **Codex cost index vs all-Luna.** Basis `tokens (last 15 min ...)`: Σ credits of the real Codex tokens in the last 15 min (each model priced at its published rate) ÷ the credits the SAME tokens would cost on gpt-6-luna. 1.0 = everything on gpt-6-luna; gpt-6.1-sol alone would be 20. Basis `agent-count weighted`: used when no recent token events; the average published output rate of live Codex agents ÷ 12.5. Codex only (Claude and agy aren't included). Rates come from `captains/rates/codex_rates_*.json` since 2026-10-18 ~04:58Z (same values as before). |
| `cost_basis` | which of the two methods above was used |
| `tiers.{luna,sol,astra,claude,agy,other}` [`tier_*`] | number of live agents in each group (Codex by model family; all Claude screens; all agy screens) |
| `quota_pct` | Codex plan usage, % of the current window (real value reported by Codex) |
| `credits_h` | estimated Codex credits per hour over the last 60 min (published rates × real tokens) |
| `luna_fail_total` | cumulative count of ledger events where model ∋ `luna` or tier ∈ {T0, T1} and outcome ∈ {fail, loop, timeout, escalated}. **A "Luna failure"** is a task that failed its success criteria on Luna when the captain checked, or was escalated to a stronger model. |
| `luna_fp_rate` | Luna first-pass rate, %: among tasks whose first *judged* ledger event (pass/fail/loop/timeout/escalated) was on Luna, the share where that event is `pass`. See the caveat about values before ~05:00Z. |
| `proj.<p>.done` / `.total` [`<p>_done`, `<p>_total`] | workplan tasks done / total from the project's ledger |
| `proj.<p>.ip` [`<p>_in_progress`] | in progress = ledger in-progress/review/assigned states, plus tasks with an active claim or a LANE line (in_progress, lane_complete or integrated) in the coordination log. JopeDime ASSIGNMENT files and LANE lines count only while their named screen is running. |
| `proj.<p>.st.<stage>` [`<p>_stage_<stage>`] | per-task **delivery stage** counts (added 2026-10-18; absent/blank before). Every task is in exactly one stage, so the seven add up to `total`. `done` = acceptance-verified on main (headline number). `on_main` = merged to main with main CI green but no acceptance receipt yet. `in_pr` = in the open integration PR. `implemented` = lane finished (LANE `lane_complete`/`done`) but not yet in the PR. `in_progress`, `blocked`, `not_started` = the rest. Sources: JopeDime `LIVE_STATUS.json` (`verified_tasks` → done, `programme_progress.verified_task_ids` → on_main, `lane_complete_task_ids` on the open PR's branch → in_pr); DOPE packet `tasks.json` + LANE lines; Joil `TASKS.json`/progress + COORD LANE lines; Jarambit `docs/WORKPLAN.md` + LANE lines. LANE branches are checked with `git merge-base --is-ancestor` against `origin/main` (→ on_main) and the PR head (→ in_pr). Evidence only ever promotes a task, except that a ledger-blocked task stays `blocked`. **Joil:** its ledger lives on the PR #6 branch (origin/main has no V5 plan), so ledger done/review tasks count as `in_pr` and done-on-main is 0 until Jepson merges PR #6. |
| `proj.<p>.ci` / `.pr` | CI rollup state (success/failure/pending) of the project's integration PR, and its number |
| `hyg.<p>` [`<p>_hygiene`] | super_captain hygiene score 0-100 (stale merged worktrees, uncommitted work > 45 min, lanes behind, temp growth) |
| `hosts.<h>.load` [`<h>_load_pct`] | 1-min load as % of CPUs |
| `hosts.<h>.disk` [`<h>_disk_root_pct`] | `/` used % |
| `ty.tok` / `out` / `cr` [`ty_tokens`, `ty_output`, `ty_credits`] | all Codex session tokens on xbabe2 in the yield window / output tokens / estimated credits |
| `ty.int` / `main` | tokens attributed to commits that landed on the integration branch / on main |
| `ty.yi` / `ym` / `cov` | yield to integration % / yield to main % / attribution coverage % |
| `ty.p.<p>` = `[tokens, integration_tokens, main_tokens, credits_per_done_todo]` [`ty_<p>_*`] | the same, per project |

### ledger (`model_usage_*.jsonl`, canonical schema)

`ts` (UTC), `project`, `task`, `screen`, `host`, `tier` (T0-T4 for Codex; captains may use C0-C3 for Claude), `model`, `effort`, `outcome` (`started`, `pass`, `fail`, `escalated`, `deescalated`; legacy `loop`/`timeout`), `escalated_to`, `reason` (short captain note, max 400 chars), `tokens` (int or null), and `source_format`:
- `canonical`: written in the unified schema.
- `legacy_s9`: an earlier draft of playbook §9, mapped here as `utc`→`ts`, `to_tier`→`escalated_to`, `note`→`reason`.

Optional fields when present: `class`, `attempt`, `minutes`. Exact duplicate lines are collapsed.

### quota / resources / tokens / token_yield / snapshots
- `codex_quota.csv`: `used_pct` (% of the plan window), `credits_balance` (as reported by Codex).
- `disk_used.csv`: `used_bytes` per `host` and `mount`.
- `host_snapshots.csv`: `load1`, `nproc`, `mem_total_gb`, `mem_avail_gb`, `swap_used_gb`, `disk_root_pct`, `disk_root_avail_bytes`, `gpus`, `gpu_util_avg`. xbabe0 is never probed.
- `codex_sessions.csv`: `session` (rollout or session id), `tool`, `model`, `effort`, `project` (from the session cwd), `host`, `start_utc`, `end_utc`, token counts `input`, `cached`, `output`, `reasoning`, and `credits_est` (published Codex rates). Values are cumulative per session; later exports overwrite earlier values for the same session.
- `claude_hourly.csv`: `messages`, `input`, `cached`, `output` per hour, project and model.
- `token_yield_*.jsonl`: the full `total`, `by_tier` and `projects` blocks of `token_yield.json`, plus `method`, `hosts_covered` and `not_covered`.
- `projects_*.jsonl`: per project `done`, `total`, `counts`, `stages` (same seven keys as above), `stage_meta` (`pr_number`, `pr_open`, `main_sha`, `main_ci_green`, `accepted_in_pr`, `blocked_code_stage` = how far the code of blocked tasks has got), `blocked`, `workers`, `pr_number`, `pr_head` (12 chars), `ci_state`, `needs_jepson` (count only) and `hygiene` (score, grade and counters).

## Known caveats
- **Token yield is an estimate.** It covers xbabe2 only (xbabe1/xbabe3 sessions aren't counted). It attributes session tokens to commits authored in the session window in the session's repo. Attribution coverage was **~57-59%** overnight.
- **Credits are estimates** from published Codex rates. OpenAI publishes no exact per-model Pro-quota ratio. `quota_pct` and `credits_balance` are the only real quota numbers.
- **Ledger format unification at 2026-10-18 ~05:00Z:** before that, two schemas coexisted (see `source_format`). The collector also counted a task's first event, including `started`, when computing `luna_fp_rate`. **`luna_fp_rate` values before ~05:00Z are therefore artificially low**, around 11% instead of ~90%+.
- Model routing changed at 2026-10-18 ~03:43Z, when the default moved from gpt-6.1-sol xhigh to gpt-6-luna. The cost index drops around then by design.
- `history` starts at 2026-10-18T04:17Z, the earliest row the dashboard kept. The ledger starts at 02:50Z (back-filled by captains).
- The `project` value in the token files comes from the session working directory. `other` means outside the four projects.

## Sanitization
- No raw prompts, screen hardcopies, message ids, credentials or tokens are exported.
- Home paths are rewritten to `~/`.
- Every run ends with a secret-pattern scan (GitHub, OpenAI, AWS, Slack and Google keys, private keys, JWTs, password/secret assignments, absolute home paths). Any hit aborts the run and nothing is committed. `gitleaks` isn't installed on xbabe2; the built-in scan plus an `rg` pass were used for the initial snapshot.

## Updates, and the data-commit exception
- `admiralty history publish` runs **hourly at minute 23** from cron on xbabe2, in its own clone (`~/.admiralty-history`), never in the live checkout. Crontab line: `23 * * * * admiralty history publish` (use the installed binary's absolute path). `ADMIRALTY_HISTORY_CLONE`, `ADMIRALTY_HISTORY_LOG` and `ADMIRALTY_HISTORY_LOCK` override the clone, log and lock paths.
- It commits **only files under `data/history/`**. If anything else is staged, it refuses.
- **Exception to the PR rule:** these data-only commits are pushed **directly to `main` as jepsontaylor**, with no PR or review. All code (including the exporter itself) still goes through the normal PR flow: neverhumanbot approves the exact head, then `gh pr merge --rebase --match-head-commit`.
- Log: `~/xbabe2_kicker/captains-dashboard/history_export.log` (not in git).
- To stop it, remove the `admiralty history publish` line from `crontab -e`.

The automated `admiralty history publish` publisher is a Linux cron utility.
It requires Linux, Git and `ionice` and refuses unsupported hosts or missing
tools before acquiring a lock or modifying a checkout. It takes a non-blocking
`flock(2)` on the lock file itself; a contended lock is a logged, successful
no-op. The exporter runs in-process at `nice` 15 and idle I/O class; no Python
is needed. To export or scan by hand: `admiralty history export [--out DIR]
[--dash DIR] [--capd DIR] [--check]` (`--repo DIR`, default the current
directory, locates `captains/rates/`).
