# Admiralty

Coordinate complex engineering work across projects, coding agents and a fleet.
The v2 implementation uses Rust for durable coordination and a React/Vite dashboard
for visibility. Compact typed task capsules preserve scope, checks and authority;
agent reports do not grant acceptance or publication authority.

The existing captain dashboard remains operational during migration. Its private
deployment history and standing orders are retained below. Runtime data,
credentials, transcripts and databases stay outside Git.

## Run the current preview

Requires Rust 1.95.0, Node 24+ and pnpm 11.15.1. This is a source-built development
preview; install bundles and native fleet execution are still being implemented.

```sh
pnpm install --frozen-lockfile --ignore-scripts
pnpm live --status /absolute/path/status.json --index /absolute/path/index.html
```

Open http://127.0.0.1:18891/ for real observations from your existing collector.
The launcher creates private preview state and a dashboard access token, whose
file path is printed without displaying its value. The dense fleet page opens
first. Add `--models /absolute/private/models.json` for model settings; use the
token through Session access to edit them. Stop with Ctrl-C.

For an isolated synthetic demo, run `pnpm demo` and open
http://127.0.0.1:18892/. Demo and real observation use separate default ports.
Neither launcher makes provider calls or enrolls machines. The launcher builds the UI
and keeps its Rust target directory in this checkout; `ADMIRALTY_TARGET_DIR`
overrides it. The preview uses one SSE connection and snapshots on changes,
without idle model turns or repeated idle snapshot requests.

## Detailed fleet view and own models

The React dashboard includes the dense fleet view: project tabs, task stages,
blockers, captain and worker details, PR/CI/review, coordination logs, hierarchy,
usage, resources and history. Admiralty's build is an ordinary project tab.
An exact original dashboard remains available under `/legacy/` when configured.
See the [live console guide](docs/operators/live-console.md) for the complete
chart strip, hierarchy, source accounting and view controls.

Models settings lets an operator add providers, native model IDs, tier order,
capacity, capabilities, attempt caps and escalation links. A self-hosted model can
run before Luna. Setup and permission blocks wait for repair; genuine failed
checks can escalate under the configured budget. See [model setup](docs/operators/models.md)
and [OpenCode SSH integration](docs/operators/opencode-ssh.md).

## Admiralty v2 implementation

The fleet is building the Rust control plane and React dashboard on integration PR [#9](https://github.com/neverhuman/admiralty/pull/9). Admiralty appears as an ordinary project in the detailed React view, with its worker lanes and work packets. Packet claims, candidate review, verification and integration are distinct; worker reports do not create accepted work.

The current Rust foundation provides durable scoped task/message state, context capsules, crash replay, backups, generated schemas/TypeScript and a read-only API/SSE demo. Native execution and fleet enrollment remain disabled pending their qualification gates.

```sh
cargo run -p admiralty-cli -- demo --snapshot-only
cargo run -p admiralty-cli -- demo --listen 127.0.0.1:18892
```

See [prototype operations](docs/operators/bootstrap.md), [implementation plan](docs/plans/admiralty-v2/MASTER_PLAN.md), [work packets](docs/plans/admiralty-v2/WORK_PACKETS.md), [execution ledger](docs/plans/admiralty-v2/EXECUTION.md), [bootstrap guide](docs/operators/bootstrap.md), and [live rollout bridge](docs/operators/rollout-dashboard.md). `scripts/check/all.sh` runs contributor checks without making paid provider calls.

## Hierarchy
```
Jepson (owner; his own typed prompts override everyone)
 └─ Grok Bot (ENG:watcher) — 4 routines, every 30 min, talk only to super_captain / captains
     └─ super_captain  (screen `super_captain`, agy; Jepson's proxy for reversible decisions)
         │   ticker: screen `super_captain_tick` → pastes a 30-min check when idle
         ├─ captain_joil      (~/joil)       ─┐
         ├─ captain_dope      (~/dope)        │ agy captains: plan, route ToDos, review, integrate
         ├─ captain_jopedime  (~/JopeDime)    │ (agy plans; Codex Luna/Sol + Claude lanes code)
         └─ captain_jarambit  (~/jarambit)   ─┘
              └─ lanes / worker screens (one worktree + branch each; xbabe1/2/3 — never xbabe0)
```

## Layout
| Path | What |
|---|---|
| `dashboard/collector.py` | read-only collector: screens (hardcopy only), ledgers, PR/CI via gh, writes `status.json` every ~45 s |
| `dashboard/extras.py` | super_captain activity, global resources, hierarchy, model mix / token efficiency, history snapshots, ledger stats |
| `dashboard/globalres.py` | disk / RAM / swap / load / GPU across xbabe1, xbabe2, xbabe3 (`python3 dashboard/globalres.py` prints a table) |
| `dashboard/token_yield.py` | tokens.db: Codex + Claude session tokens → project → commits → landed on integration / main (yield estimate) |
| `dashboard/serve.py`, `run.sh`, `index.html`, `make_snapshot.py` | 127.0.0.1:18890 static server, launcher, page (polls every 15 s), static snapshot |
| `captains/` | COMMON.md, MODEL_ROUTING_PLAYBOOK.md, CPU_SLICES.md, captain briefs, addenda, super_captain brief + ticker, disk-mission template, notices |
| `tools/` | screen helpers (`bin/screen_send`, `bin/screen_snap`, `bin/kick`, `bin/status`, `classify_screen.py`), idle-only delivery scripts (`send_*.sh`), owner peek/send |
| `runtime/` | symlinks to the live runtime dirs (git-ignored) |
| `data/history/` | sanitized history dataset (cost index, Luna failures, in-progress per project, quota, hosts, tokens, yield) exported hourly by `tools/export_history.py`; see `data/history/README.md` |

The live paths are unchanged: files in `/home/ubuntu/xbabe2_kicker/captains-dashboard/` and `/home/ubuntu/xbabe2_kicker/captains-20261017/` are **symlinks into this repo**, so agents keep using the old paths. Runtime data stays in those directories (`CDASH_RUNTIME` overrides the dashboard runtime dir).

## Run the dashboard
```
screen -dmS captains_dash bash -c 'cd /home/ubuntu/xbabe2_kicker/captains-dashboard && ./run.sh 2>&1 | tee -a dash.log'
```
From a Mac: `ssh -N -p 6969 -L 18890:127.0.0.1:18890 ubuntu@162.218.217.123` then open http://localhost:18890/ .
Restart only the collector after a code change: kill the `python3 -B collector.py` child of `run.sh` (it restarts in 10 s). The server never needs a restart for page changes.

## super_captain
```
screen -dmS super_captain bash -lc 'cd /home/ubuntu/xbabe2_kicker/captains-20261017 && agy --dangerously-skip-permissions; exec bash'
# paste captains/super_captain_kickoff.txt via readbuf/paste, then:
FIRST_DELAY=1800 screen -dmS super_captain_tick bash -c /home/ubuntu/xbabe2_kicker/captains-20261017/super_captain_tick.sh
```

## Add a project
1. Write `captains/captain_<x>_BRIEF.md` (authority files, ledger, coord log, roster, rules) and start `captain_<x>` running agy in the repo.
2. Add it to `CAPTAINS`/`NAMES`/`COLLECT` and a `collect_<x>()` ledger parser in `dashboard/collector.py`, `COORD`/`REPOS` in `extras.py`, `PROJ`/`REPO` in `token_yield.py`, and its CPU slice in `CPU_SLICES.md`.
3. Add it to the super_captain brief's captain table.

## Rules that never change
Never xbabe0; never Jeryu, CI runners or other services; screens via hardcopy/readbuf/paste/stuff only (never `screen -r`); no email/messages/spending; merges only after neverhumanbot approves the exact head, then `gh pr merge --rebase --match-head-commit`; no squash/--admin/force-push; Joil PR #6 is merged only by Jepson. Changes to this repo go through PRs under the same rules (the initial commit was made directly on main).
