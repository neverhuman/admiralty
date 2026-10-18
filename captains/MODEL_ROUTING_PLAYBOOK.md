# MODEL ROUTING + LOGGING PLAYBOOK (all captains)

Issued: Oct 17 2026, ~9:45 PM MDT, by Grok Bot for Jepson. Applies to captain_joil, captain_dope, captain_jopedime, captain_jarambit and every worker they own.

**This replaces the Oct 1 default of "every Codex screen on gpt-6.1-sol xhigh".** From now on, start each task on the cheapest model that can pass its success criteria, and move up only on objective failure. Throughput rules from Addendum 1 and 2 stay in force: Jepson wants MANY lanes, all code work in flight, and the current Codex quota used up. The point is to buy more lanes with the same quota: **prefer more Luna lanes over fewer Sol lanes.**

Safety and merge rules are unchanged: neverhumanbot approves the EXACT head, then `gh pr merge <n> --rebase --match-head-commit <sha>`. No squash, no `--admin`, no `--merge`, no force-push. Joil PR #6 is merged only by Jepson. Done still needs real evidence.

---

## 0. Why (published Codex credit rates, credits per 1M tokens, Standard speed)

| Model (Codex id) | Uncached in | Cached in | Output | Output vs gpt-6-luna | Verdict |
|---|---:|---:|---:|---:|---|
| gpt-6-astra | 250 | 25 | 1250 | 100x | Last resort only (Tier 4) |
| gpt-5.6-sol | 100 | 10 | 500 | 40x | DO NOT USE (dominated) |
| gpt-5.6-terra | 50 | 5 | 300 | 24x | DO NOT USE (dominated) |
| **gpt-6.1-sol** | 50 | 2.5 | 250 | 20x | Strong tier (Tiers 2-3) |
| gpt-6-sol | 50 | 5 | 250 | 20x | DO NOT USE (6.1-sol is the same price with cheaper cache) |
| gpt-5.6-luna | 5 | 0.5 | 30 | 2.4x | DO NOT USE (dominated by gpt-6-luna) |
| **gpt-6-luna** | 2.5 | 0.25 | 12.5 | 1x | DEFAULT (Tiers 0-1) |

- gpt-6-luna is **20x cheaper than gpt-6.1-sol** per output token. One Sol lane costs about as much as 20 Luna lanes.
- **Always Standard speed.** Never Fast (2.5x quota) or Ultrafast (8x quota). Don't set `service_tier`/fast mode anywhere.
- **Reasoning effort burns quota too.** Never use `max` or `ultra`, even though the picker offers them. `xhigh` is only for Tier 3.
- OpenAI doesn't publish exact per-model Pro-quota ratios. These credit rates are the proxy, and they're good enough to route by.

## 1. The model ladder

| Tier | Codex model + effort | Use for | Launch flags |
|---|---|---|---|
| **T0** | `gpt-6-luna`, `medium` | Default for every simple task | `-m gpt-6-luna -c model_reasoning_effort=medium` |
| **T1** | `gpt-6-luna`, `high` | Simple task that failed twice at T0 | `-m gpt-6-luna -c model_reasoning_effort=high` |
| **T2** | `gpt-6.1-sol`, `high` | Medium tasks; simple tasks that failed at T1; start of most hard tasks | `-m gpt-6.1-sol -c model_reasoning_effort=high` |
| **T3** | `gpt-6.1-sol`, `xhigh` | Hard tier; 2nd Sol attempt after T2 failed | `-m gpt-6.1-sol -c model_reasoning_effort=xhigh` |
| **T4** | `gpt-6-astra`, `high` | LAST RESORT: T3 failed twice, or a defect only Astra can plausibly solve | `-m gpt-6-astra -c model_reasoning_effort=high` |

- **T4 requires a written justification.** Before launching it, add a line to the lane's CHANGELOG and the coordination log: `ASTRA JUSTIFICATION <task>: <why T3 is insufficient, attempts so far, evidence path>`. Drop back to T0/T2 when the task is done.
- **Claude workers: see §11 (CLAUDE CODE ROUTING), which supersedes this bullet.** Claude workers follow the same principle. Verified aliases on xbabe2 (`claude --help`, Claude Code 2.1.302) are `sonnet`, `opus` and `fable`, set with `--model`, and `--effort low|medium|high|xhigh|max`. The global default in `~/.claude/settings.json` is `"model": "opus"`, `"effortLevel": "xhigh"`. Any `claude` call without `--model` therefore runs **opus xhigh, the expensive end.**
  - Simple Claude work (reviews of small diffs, docs, receipts, planning notes): `--model sonnet --effort medium`.
  - Escalation: `--model sonnet --effort high`, then `--model opus --effort high`, then `opus xhigh`.
  - `fable`: don't use without the same written justification as Astra.
  - Pass `--model` and `--effort` explicitly in every runner/launcher (worker_runner.py, claude-resume-stage*.py, launch_*.sh). Do NOT edit `~/.claude/settings.json`, because other non-captain screens share it.
- **agy (Gemini 4 Argon) is the captain and planner, not a coder.** Use agy only to plan, decompose, classify, route ToDos to cheaper workers, review evidence, and orchestrate integration and merges.
  - No implementation turns in agy. Keep agy turns short.
  - The old agy worker screens (dope_gemini, jopedime_agy, agy_jarambit_screen) should get routing and review work, or sit idle. Don't give them coding packets.
  - If agy finds itself writing code, it should stop, write a packet, and hand it to a Luna lane.

## 2. Task classification rubric (assign BEFORE dispatch)

**SIMPLE: start at T0 (gpt-6-luna medium)**
- Docs, READMEs, comments, changelog/receipt writing, ledger/COMMS updates
- Config, YAML/TOML/JSON edits, CI workflow tweaks, dependency bumps with known fixes (e.g. `zip` advisory bump)
- Renames, moves, mechanical refactors inside one module, file splits to satisfy a line limit
- Tests for EXISTING behaviour (characterization tests, fixture plumbing)
- Small bugfix where a failing test already pins the bug
- Lint, fmt, clippy and CI-red fixes; boilerplate; CLI flag wiring; serialization structs
- Data plumbing: loaders, schema/ledger JSON generation, report generation, scripts
- Running a validation command and collecting evidence

**MEDIUM: start at T0 if the spec is very precise, otherwise T2 (gpt-6.1-sol high)**
- Multi-file feature with a clear written spec and acceptance tests
- New module following an existing pattern
- Moderate test-suite expansion that needs some design

**HARD: may start at T2, or T3 (gpt-6.1-sol xhigh) when the risk is high**
- Novel algorithms, numerics, statistical correctness, kernels, SIMD/GPU, performance-critical code
- Concurrency, locking, async, distributed state, custody/lease logic
- Security-sensitive code (auth, sandboxing, permissions, secrets handling)
- Cross-module refactors and API redesigns
- Science-critical correctness (pre-registered metrics, oracles, baselines, CT/paper numbers)
- Integration, super-merge and conflict resolution across many lanes
- **Anything that has already failed twice at any tier**

Record the class and starting tier in the packet header and in the LANE line (see §5).

## 3. Success criteria and objective escalation

**Every packet must state its success criteria up front, before the worker starts:**
1. Named commands that must pass, with exit 0, e.g. `cargo test -p crate --test x`, `cargo clippy -- -D warnings`, `pytest tests/test_y.py`, `make validate`
2. Required CI checks on the exact head
3. The acceptance criteria copied verbatim from WORKPLAN or the ledger (don't paraphrase, don't loosen)
4. Evidence artifacts the worker must produce: receipt path, results JSON, hashes it actually computed, log tails
5. Owned paths (one writer per worktree) and an attempt budget

**Escalate ONE tier when any of these is true:**
- The success commands still fail after **2 full attempts** at the current tier
- The model is looping: the same diff or failing approach repeats, or it keeps reverting its own changes
- A reviewer (captain, Claude reviewer, CI or neverhumanbot) finds a **correctness defect**, not just style
- The attempt budget is exceeded: **45 min wall clock at T0/T1**, 90 min at T2/T3
- The worker says it lacks the capability or understanding

**Before escalating:** the current worker (or the captain) updates the handoff note (§4) with the failing command, the output tail and a hypothesis. Then either relaunch the lane with the higher tier, or switch the model inside the live session (see §6).

**De-escalate:** when a task completes, the lane's NEXT simple task goes back to T0. Never leave a screen parked on Sol or Astra after its hard task ends. An idle screen should sit on gpt-6-luna medium.

**Never escalate to get around a gate.** Raising the tier never justifies loosening a criterion, relaxing a threshold, or skipping evidence.

## 4. Logging so a stronger model can take over in minutes

Use the project's existing convention where there is one (Joil COORD.md and the .private/v5-workers lane dirs; DOPE COMMS.md and .agent; JopeDime COMMS.md and .agent; Jarambit COMMS.md and .agent/xfinal-*). Otherwise create:

**a) Per-lane `CHANGELOG.md`** (append-only, in the lane worktree's `.agent/` dir or the lane dir; never rewrite old entries). One entry per attempt or meaningful step:
```
## 2026-10-18 03:55 UTC / 21:55 MDT | TASK <ID> | T0 gpt-6-luna medium | lane <screen>
- worktree/branch: <path> @ <branch>
- commits: <sha> <subject> (or "uncommitted: <files>")
- commands: `<cmd>` -> exit 0 PASS | `<cmd>` -> exit 101 FAIL (tail in handoff)
- unverified / known issues: <list or "none">
- next step: <one line>
```

**b) Per-task handoff note** at `.agent/handoffs/<TASK-ID>.md` (or the project equivalent), created at dispatch and updated on EVERY escalation and at end of shift:
- Goal and success criteria (copied from the packet)
- Current state: what's done, what's left, branch and SHA
- Last failing command plus a 30-80 line output tail
- Current hypothesis, approaches already tried, and why they failed
- Files touched and key code locations (`path:line`)
- Exact next command to run

**c) Model-usage ledger**, one line per attempt in the coordination log or `.agent/model_usage.jsonl`:
```
MODEL <UTC> | task <ID> | class simple|medium|hard | tier T0..T4 | model <id> <effort> | screen <name> | outcome pass|fail|loop|timeout|escalated | minutes <n>
```
Captains use this to track the escalation rate per class. If most simple tasks pass at T0, keep routing more work to Luna.

**e) REQUIRED: shared machine-readable ledger for the dashboard** (added 10:07 PM MDT for Jepson; a dashboard parses this exact path and format)
Append ONE JSON line per attempt event to `/home/ubuntu/xbabe2_kicker/captains-20261017/model_usage.jsonl`. Every captain writes to this one file, so you must use flock:
```
{"ts":"2026-10-18T04:10:00Z","project":"dope|joil|jopedime|jarambit","task":"<TASK-ID>","screen":"<screen name>","host":"xbabe2|xbabe3","tier":"T0|T1|T2|T3|T4","model":"gpt-6-luna","effort":"medium","outcome":"started|pass|fail|escalated|deescalated","escalated_to":null,"reason":"<short>","tokens":null}
```
- `started`: an attempt is launched or a task is dispatched on a tier.
- `pass`: you (the captain) checked the success criteria and they passed.
- `fail`: you checked and they failed. **A Luna `fail` is exactly what Jepson wants to track.**
- `escalated`: moved up a tier. Set `escalated_to` to the new model (e.g. `"gpt-6.1-sol"`) and give the reason (e.g. `"2 attempts: cargo test -p x exit 101"`).
- `deescalated`: dropped back to a cheaper tier for the next task.
- `tokens`: an integer if known (Codex prints "tokens used" in exec output and in the TUI), otherwise `null`. Use `ts` in UTC ISO 8601 with Z. Keep it strict JSON on one line: double quotes, no trailing commas.

Append safely like this:
```
L='{"ts":"'$(date -u +%Y-%m-%dT%H:%M:%SZ)'","project":"dope","task":"S02","screen":"dope_lane_s02","host":"xbabe2","tier":"T0","model":"gpt-6-luna","effort":"medium","outcome":"started","escalated_to":null,"reason":"simple packet","tokens":null}'
flock -w 30 /home/ubuntu/xbabe2_kicker/captains-20261017/model_usage.jsonl.lock sh -c 'printf "%s\n" "$1" >> /home/ubuntu/xbabe2_kicker/captains-20261017/model_usage.jsonl' _ "$L"
```
(Python works too: `fcntl.flock` on the `.lock` file, then append `json.dumps(obj)+"\n"`.) Back-fill one `started` line for every lane currently running, with its present tier and model. The `MODEL ...` text lines in your coordination log stay as well.

**d) Escalated models read first.** Every escalation packet starts with: "Read `.agent/handoffs/<ID>.md` and the last 30 lines of the lane CHANGELOG before touching code."

## 5. LANE line format (extends Addendum 2)

```
LANE <TASK-ID> | screen <name> | model <id> <effort> (T<n>, <class>) | worktree <path> | branch <branch> | cpus <a,b> | state in_progress|lane_complete|integrated|done | <UTC time>
```

## 6. How to switch models (verified on xbabe2, Oct 17 2026)

Binaries: Codex is `/home/ubuntu/.npm-global/bin/codex` (codex-cli 0.162.1), Claude is `/home/ubuntu/.local/bin/claude` (2.1.302), agy is `/home/ubuntu/.local/bin/agy`.
Codex model ids from `~/.codex/models_cache.json`: `gpt-6-luna`, `gpt-6.1-sol`, `gpt-6-astra`, plus `gpt-6-sol`, `gpt-5.6-sol`, `gpt-5.6-terra` and `gpt-5.6-luna`, which are all dominated (don't use). Efforts: low, medium, high, xhigh, max, ultra (Luna has no ultra). Never use max or ultra.

**Global default, `~/.codex/config.toml`:** `model = "gpt-6.1-sol"`, `model_reasoning_effort = "xhigh"`, `plan_mode_reasoning_effort = "xhigh"`. **Do NOT edit it.** Other non-captain screens on this box depend on it. Always pass `-m` and `-c model_reasoning_effort=...` per screen or per exec instead.

**New interactive Codex lane (screen):**
```
screen -dmS <proj>_lane_<id> bash -lc 'cd <worktree> && exec nice -n 10 taskset -c <cpus> codex --dangerously-bypass-approvals-and-sandbox -m gpt-6-luna -c model_reasoning_effort=medium -C <worktree>'
```
(The bare value works: per `codex --help`, a `-c` value that doesn't parse as TOML is used as a raw string. This is the same as the current dope_lane_* launcher with `-m gpt-6.1-sol ... "xhigh"` swapped for `-m gpt-6-luna ... "medium"`.)

**Headless Codex:** `codex -a never -m gpt-6-luna -c model_reasoning_effort="medium" ... exec -o <last-message.txt> -`
- Joil: `lane_supervisor.py` reads `model` and `reasoning_effort` from `lease.json` → `codex_lane_pool.lanes.<codex_laneN>`. All lanes currently say gpt-6.1-sol xhigh. Change those two fields per lane, or better, add a per-stage tier override argument to the supervisor. Record either change as a captain lease note in COORD.md. Caps, affinity and RSS rules stay unchanged. Also: `codex-resume-stage-*.py` passes no `-m`, so it inherits the gpt-6.1-sol xhigh global default. Pass `-m`/effort explicitly for the next stage.

**Switching a LIVE interactive Codex session:** when the session is idle at its prompt, type `/model`, press Enter, then pick the model and effort in the picker. That takes a few keystrokes; verify with hardcopy that the footer shows e.g. `GPT-6-Luna medium`. Do it between tasks, never mid-turn. If the picker is awkward to drive via screen stuff, let the current task finish, then relaunch the lane with the new `-m` flags (same worktree, new screen name or the same name after the old one exits cleanly). **Never kill a mid-task worker just to switch.** Switch at the next task boundary.

**Claude:**
- New session: `claude --model sonnet --effort medium ...`
- Headless runners: add `'--model','sonnet','--effort','medium'` to the cmd list (e.g. Jarambit `worker_runner.py`, Joil `claude-resume-stage-*.py`, JopeDime `launch_jope_review_claude.sh`)
- Live interactive session: `/model sonnet` (and `/effort medium` if available, or relaunch with `--effort`)

**agy:** stays on Gemini 4 Argon High for captain duty. Keep its turns short and delegate.

## 7. Captain duties every cycle (add to your 30-min loop)

1. Reclassify every queued and in-progress ToDo (simple/medium/hard) and assign a tier.
2. Point new simple packets at T0 Luna lanes. Re-point idle Sol screens to Luna (via `/model` at a task boundary or a relaunch).
3. **Use the quota savings to launch MORE lanes** within your CPU slice (CPU_SLICES.md) and the Addendum 2 caps. Jepson still wants every code task in flight and the current Codex quota used up.
4. Check the model-usage ledger and escalate stuck lanes promptly. Don't let a Luna lane grind past its budget.
5. Make sure every lane has a CHANGELOG plus a handoff note. Reopen "done" claims that lack them.
6. In your 30-min status line, report the tier mix, e.g. `TIERS T0:9 T1:2 T2:3 T3:1 T4:0 | claude sonnet:2 opus:1`, plus any Astra/fable justifications.
7. If Codex quota runs out: post `NEEDS JEPSON: Codex quota exhausted` and fall back per Addendum 1.

## 8. Immediate actions (now)

a) Post `MODEL ROUTING ACK` in your coordination log, and start appending to the shared `model_usage.jsonl` (§4e).
b) Switch every worker screen you own to the right tier: Luna for simple and in-progress-simple work, at the next task boundary (or now if idle). Fix every launcher/runner script so new launches pass explicit `-m`/`--model` and effort flags.
c) Add the CHANGELOG and handoff logging to every lane (back-fill a one-entry CHANGELOG for lanes already running).
d) Post a per-screen model table in the coordination log: `screen | task | class | tier | model effort (before -> after) | switched now / at boundary`.

## 9. Central machine-readable model-usage ledger (Jepson, Oct 17 2026, 10:08 PM MDT) — REQUIRED

The §4c `MODEL ...` text lines stay valid, but the dashboard now charts Luna failures, first-pass rate and the escalation funnel from ONE JSONL file. **Append one line per attempt AND per escalation** to:

`/home/ubuntu/xbabe2_kicker/captains-20261017/model_usage.jsonl`

```
{"utc":"2026-10-18T04:20:00Z","project":"dope","task":"C03","class":"simple","tier":"T0","model":"gpt-6-luna","effort":"medium","screen":"dope_lane_c03","host":"xbabe2","attempt":1,"outcome":"pass","minutes":22,"to_tier":null,"session":"<codex session id or null>","note":"cargo test -p x exit 0"}
```
- `outcome`: `pass` (success criteria verified by the captain), `fail`, `loop`, `timeout`, or `escalated` (write this when you move the task up; set `to_tier`, e.g. "T2").
- One JSON object per line, no pretty-printing. Append with `flock /home/ubuntu/xbabe2_kicker/captains-20261017/model_usage.lock -c 'printf "%s\n" "$LINE" >> .../model_usage.jsonl'`. Never rewrite the file.
- Back-fill one line for each attempt already judged tonight if you can do it truthfully; otherwise start now.

## 10. Commit trailers for token-yield tracking (Jepson, 10:08 PM MDT) — REQUIRED going forward
Every commit made by an agent lane carries trailers so tokens can be matched exactly to code that lands on main:
```
Agent-Session: <codex/claude session id, e.g. the rollout file's UUID, or "unknown">
Agent-Model: <model> <effort>   (e.g. gpt-6-luna medium)
Agent-Task: <TASK-ID>
```
e.g. `git commit -m "C03: ..." --trailer "Agent-Session: 01a14d16-..." --trailer "Agent-Model: gpt-6-luna medium" --trailer "Agent-Task: C03"`. Codex: the session id is in `~/.codex/sessions/.../rollout-*-<uuid>.jsonl` (the worker can find its own via `ls -t ~/.codex/sessions/*/*/*/ | head`) or "unknown". Keep trailers when rebasing (rebase preserves them). Don't add them to the integration PR history retroactively.

---

## 11. CLAUDE CODE ROUTING (Claude Max plan, claude.ai login). Added 10:29 PM MDT for Jepson

This section replaces the short Claude bullets in §1. Everything else carries over to Claude unchanged: escalation criteria and budgets (§3), logging and handoffs (§4), the shared `model_usage.jsonl` (§4e, logging model as e.g. `claude-haiku-5-5`), and the de-escalation rule.

**Published API list prices (USD per 1M tokens: input / cached input / output, standard speed):**

| Model | In | Cached | Out | Out vs Haiku 5.5 (<=100K) | Verdict |
|---|---:|---:|---:|---:|---|
| Fable 5.1 (`fable`) | 10 | 0.25 | 50 | 100x | C3, last resort |
| Fable 5 | 10 | 1 | 50 | 100x | BANNED (5.1 has a cheaper cache) |
| Opus 5 / 4.8 / 4.7 / 4.6 / 4.5 | 5 | 0.5 | 25 | 50x | BANNED (cost more than Opus 5.5) |
| **Opus 5.5 (`opus`)** | 4 | 0.2 | 20 | 40x | C2 |
| Sonnet 4.6 / 4.5 (deprecated, retire 2026-11-30) | 3 | 0.3 | 15 | 30x | BANNED |
| **Sonnet 5.5 (`sonnet`)** | 2 | 0.1 | 10 | 20x | C1 |
| Sonnet 5 | 2 | 0.2 | 10 | 20x | BANNED (5.5 is the same price with a cheaper cache) |
| Haiku 4.5 | 1 | 0.1 | 5 | 10x | BANNED (even Haiku 5.5 over 100K is cheaper) |
| Haiku 5.5, prompt >100K | 0.5 | 0.05 | 2.5 | 5x | C0 (avoid going over 100K) |
| **Haiku 5.5 (`haiku`), prompt <=100K** | 0.1 | 0.01 | 0.5 | **1x** | **C0 DEFAULT** |

- **Aliases** (verified 10:30 PM MDT): `haiku`, `sonnet`, `opus` and `fable` resolve to Haiku 5.5, Sonnet 5.5, Opus 5.5 and Fable 5.1. On xbabe2, `claude --print --model haiku --effort medium "Reply with exactly: OK"` returned `OK`, reported model `claude-haiku-5-5`, and cost $0.0026.
- **Never use Fast mode.** Never use `--effort max`. `xhigh` is only for C2 or C3 on a 2nd attempt.
- **Tokenizer:** 4.7+ models produce ~30% more tokens for the same text than 4.6 and older. That's already priced into the banned list. Don't "save" by picking an old model.

**Max-plan quota facts** (Anthropic publishes no per-token quota ratios, so list prices are the proxy):
- Rolling **5-hour session window**, a **weekly all-models limit**, and a **SEPARATE weekly Sonnet-only limit**. Sonnet work draws on its own pool, so routing medium work to Sonnet takes load off the shared weekly limit.
- **Fable 5/5.1 may use at most 50% of the weekly limit and burns it faster.** Keep Fable to a small share and justify every use.
- The limits are shared with Jepson's own claude.ai and Desktop use. Don't drain them to zero.

**Claude ladder:**

| Tier | Alias (model) | Effort | Use for | Flags |
|---|---|---|---|---|
| **C0** | `haiku` (Haiku 5.5) | medium | Default for simple work: docs, receipts, small-diff reviews, validators, ledger/COMMS edits, lint/test fixes with a failing test | `--model haiku --effort medium` |
| **C1** | `sonnet` (Sonnet 5.5) | medium, high on retry | Medium tasks, and simple tasks that failed twice at C0 | `--model sonnet --effort medium` (or `high`) |
| **C2** | `opus` (Opus 5.5) | high (xhigh on 2nd attempt) | Hard tasks, and tasks that failed at C1 | `--model opus --effort high` |
| **C3** | `fable` (Fable 5.1) | high | LAST RESORT, only after C2 failed twice. Needs `FABLE JUSTIFICATION <task>: ...` in the CHANGELOG and coordination log | `--model fable --effort high` |

**Keep C0 prompts under 100K tokens (5x cheaper):** one task per session. Start a fresh session (`--print` / `--no-session-persistence` runs, or `/clear` in an interactive session) for each new task. Don't `--resume` a huge session for a simple task. Point Haiku at specific files, not whole trees.

**How to set it (don't edit `~/.claude/settings.json`; its global default is `"model": "opus"`, `"effortLevel": "xhigh"` and other screens share it):**
- New interactive screen: `claude --model haiku --effort medium ...`
- Headless runner: add `'--model','haiku','--effort','medium'` (or the right tier) to the command list. **Any runner without `--model` silently runs opus xhigh.** Fix every launcher for NEW packets: Jarambit `.agent/xfinal-*/**/worker_runner.py` (jr_xfinal_*), Joil `claude-resume-stage-*.py` and `lease.json` `workers.claude.model`, JopeDime `launch_jope_review_claude.sh`, and any DOPE paper/review launcher. Don't modify in-flight runs.
- Live interactive session: `/model haiku` (or `sonnet`) at an idle prompt, between tasks, then `/clear`.

**Codex vs Claude:** Codex credits and Claude USD are different currencies, so don't compare them directly. Use the cheapest tier of whichever vendor still has quota (Codex T0 gpt-6-luna, Claude C0 haiku). If one vendor's quota is low, route simple packets to the other's cheapest tier. Post `NEEDS JEPSON:` if both run low.

**Captain checks each cycle (add to §7):** list every Claude screen and run with its model, and flag any running without `--model`, on a banned model, or on Fable without a justification. Include in the tier mix: `claude C0:n C1:n C2:n C3:n`.
