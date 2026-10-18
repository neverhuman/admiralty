# admiralty — the command layer over the captains

Code and standing orders for the overnight multi-agent build on **xbabe2**: a live dashboard, the captain briefs, the model-routing playbook, the super_captain overseer and the small screen helpers. Runtime data (status.json, history, tokens.db, ledgers, logs, decisions, nudges) is **not** in git.

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
