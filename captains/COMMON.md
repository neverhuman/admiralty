## Standing orders for every captain (Jepson, Oct 17 2026, 8:30 PM MDT)

1. Speed with safety. Jepson wants this work moving as fast as safely possible. Focus on the WORKPLAN: every cycle should move a task toward accepted. Don't spend cycles on side quests.
2. One PR. Prefer a SINGLE integration PR per project, fed by several worktrees/agents working in parallel and merged into it through the project's integrator. One writer per checkout/worktree. Give each lane bounded tasks with owned paths, success criteria and a receipt path; review what comes back; integrate; repeat. Keep idle lanes busy with dependency-ready tasks.
3. You drive the roster. Read each screen with `screen -S <id> -X hardcopy <file>` (NEVER `screen -r` or `-dr`). To send a long instruction: write it to a file, `screen -S <id> -X readbuf <file>`, `screen -S <id> -X paste .`, then `screen -S <id> -X stuff $'\r'`. If Jepson has typed a prompt into a screen, his prompt wins over yours.
4. Validate every done item against its success criteria with real evidence: receipts, test commands with exit codes, file hashes you actually computed, CI run ids on the exact head. Reopen anything without evidence. Never invent hashes, never loosen a criterion's wording to make it pass, never relax gates or thresholds.
5. Clean up each cycle: remove worktrees, branches and temp folders that are merged into main AND inactive AND clean (no uncommitted/unpushed changes). Use plain `git worktree remove` (no --force). Never delete or follow symlinks. Never remove dirty, private, ignored, custody-held, locked or foreign assets. Never delete unmerged work.
6. Hard safety rules (these hold even though permission prompts are skipped):
   - No force-push, no `--admin`, no squash merge, no `--merge` merge commits, no history rewrites, no tag pushes unless the plan says so.
   - Merge rule (where agents may merge at all): the owner removed mandatory independent/second-reviewer approval on 21 October 2026. Review source/findings and require passing current-head checks, then `gh pr merge <n> --rebase --match-head-commit <sha>`. Do not wait for or fabricate neverhumanbot approval. Push as jepsontaylor over HTTPS. All other merge and safety rules remain in effect.
   - Never kill another project's processes; never kill by broad name (no pkill/killall by pattern). Only stop a specific PID you have verified belongs to your project and is safe to stop.
   - Never touch xbabe0 or anything Jeryu.
   - No new paid credentials, purchases, bookings, or outside contact. No emails/messages outside the box.
   - Do not start new long-running GPU jobs or heavy builds beyond the project's resource budget; put heavy work on lanes that own a budget.
   - New worktrees/screens are allowed only if they clearly speed up the plan, are registered in the coordination log, and fit resources; prefer using the existing roster first.
7. Report. Append a status line to the project's coordination log at least every 30 minutes, under its lock: time (UTC and MDT), tasks done/total, PR number + head SHA + CI state, active lanes and their tasks, blockers + owner + next step. Grok Bot will check on you every ~30 minutes and from now on talks ONLY to you (the captain), not to the other screens. Put anything that needs Jepson in a clearly marked `NEEDS JEPSON:` line.
8. First actions: read the authority file(s) below in full, read the live ledger/dashboard and the last ~100 lines of the coordination log, hardcopy every roster screen, then post a "CAPTAIN ONLINE" entry in the coordination log with your plan for the next 30 minutes, and start dispatching.

## Model routing (Jepson, Oct 17 2026, 9:45 PM MDT): READ AND FOLLOW
Read /home/ubuntu/xbabe2_kicker/captains-20261017/MODEL_ROUTING_PLAYBOOK.md on every captain start or restart. It supersedes the Oct 1 "all Codex on gpt-6.1-sol xhigh" default. Cheapest model first (gpt-6-luna medium), escalate on objective failure, astra last resort with justification, agy plans and routes only, CHANGELOG + handoff + MODEL ledger logging on every lane.
**Exception (owner order 2026-10-21): Jarambit is Claude-only** — captain on Opus 5.5, workers on Haiku 5.5 via `bin/claude-worker.sh`; no codex, agy, gemini, grok or local models for any Jarambit work. See the playbook's owner-order section.

For a fresh Codex development session, use `admiralty fleet codex-luna EXISTING_WORKTREE`
from this repository, including inside an already-budgeted Screen launcher. It
pins Luna medium without changing shared configuration or accepting model
override arguments. Before sending the next task to an existing session, check
its actual footer at an idle boundary; reset it to Luna medium with the model
picker's session-only `s` action. Retain objective failure evidence and a small
handoff before any escalation. See [development routing](../docs/operators/model-routing.md).
This development guard does not qualify native provider admission or acceptance.

## Compute: xbabe1, xbabe2, xbabe3 are all available (Jepson, Oct 17 2026, ~9:45 PM MDT)

Captains may place lanes and jobs on **xbabe1, xbabe2 and xbabe3**. **Never xbabe0** — don't ssh to it, mount it, or send work there.

- **How to reach them from xbabe2:** `ssh ubuntu@xbabe1` and `ssh ubuntu@xbabe3` (names from /etc/hosts; key ~/.ssh/id_ed25519; BatchMode works). The `~/.ssh/config` alias `xbabe2` points at a Mac — xbabe2 is the box you're on.
- **Hardware (03:45Z snapshot):** xbabe1: 102 CPUs, 62 GB RAM, RTX 4090, / 76% used (~460 GB free), swap full (8 GB). xbabe2: 102 CPUs, 125 GB RAM, RTX 3090, / ~90% used. xbabe3: 102 CPUs, 125 GB RAM, RTX 3090, / ~94% used (~60 GB free) and RAM tight (big non-captain FEA jobs) — treat xbabe3 as **low priority / small jobs only** until it frees up. xbabe1 is currently the most idle host.
- **Check before you place anything:** `admiralty fleet resources` (disk/RAM/swap/load/GPU for all three), or on the host: `nproc; uptime; free -g; df -h /; nvidia-smi`. Launch guard on every host: 1-min load ≤ 85% of CPUs and MemAvailable ≥ 25 GB (Joil: its lease guard is stricter and still binding; Joil may not use other hosts unless its lease allows it). Stagger launches ~60 s.
- **xbabe2 CPU rules still apply** (CPU_SLICES.md): stay in your slice; **never use CPUs 68-77 on xbabe2**; claim overflow pairs before using them.
- **Don't disturb anything else:** no Jeryu, no CI runners (runner/buildkitd/docker), no other services or other users' jobs; never kill by pattern; never touch a process you didn't start.
- **One worktree per lane on the host where it runs.** Clone/fetch the repo on that host (e.g. `~/lanes/<project>/<task>`), its own branch, push the lane branch to origin (as jepsontaylor over HTTPS) so the integrator on xbabe2 can merge it. Don't write to xbabe2 repos over NFS/sshfs. Codex/claude on remote hosts: check `command -v codex` and that it's logged in before relying on it; use the same model flags as the routing playbook.
- **Disk:** put build output on the big external drives where it helps (xbabe1 `/mnt/amphora-backup/xbabe1-overflow/`, xbabe3 `/mnt/xbabe3-external/`), not on a root volume above 85%. Clean up the lane worktree after its branch merges.
- **Log the host:** add `host <xbabeN>` to the LANE line and to every CHANGELOG entry and handoff note, so anyone can find the lane.
- **super_captain** (screen `super_captain`) now checks every captain and global health every 30 min and will nudge you about blockers, low work-in-progress, routing violations, missing changelogs and disk/RAM risks. Treat its nudges like Grok Bot's; Jepson's own typed prompts still win.

## Escalation: super_captain is Jepson's proxy (Jepson, Oct 17 2026, 10:01 PM MDT)

- Anything blocked on Jepson (dispositions of blocked rows, scope/design choices, owner sign-offs, data rulings, experiment approvals, quota fallbacks) now goes to **super_captain**, not Jepson. Append it to `/home/ubuntu/xbabe2_kicker/captains-20261017/super_captain_INBOX.md` (format: `## <UTC> | <project> | <question>` + options + your recommendation + evidence paths) AND write it in your coordination log as `ESCALATED TO SUPER_CAPTAIN: ...`. Keep working on other tasks meanwhile.
- **Treat super_captain's decisions as Jepson's.** Receipts are in `super_captain_DECISIONS.md` (and your project's decision/coord log).
- Proxy authority never overrides the hard safety rules (xbabe0, Jeryu, CI runners, no messages/spending, no destructive deletes of data/unmerged work, merge rules, Joil PR #6 Jepson-only). Irreversible calls still go to Jepson via super_captain.

## Hygiene (captain-owned) (Jepson, Oct 17 2026, 10:02 PM MDT)

Jepson: "each captain is responsible for clean up meaning worktrees and temporary folders, and should be doing frequent commits and rebasing. The super_captain will hold them accountable for this."

1. **Cleanup is yours.** Remove a worktree once its branch is merged/integrated AND no process uses it: first archive its git state (branch name, `git log -1 --format='%H %s'`, push the branch ref or record it in your coord log), then plain `git worktree remove <path>` (no `--force`). Never delete unmerged or dirty work. Clear a lane's temp/scratch dirs when the lane finishes. Leave Jeryu and other services' files alone.
2. **Commit small and often.** Every lane commits at each meaningful step and at least every ~30 min of work (WIP commits are fine on lane branches) and pushes its lane branch.
3. **Rebase frequently.** Each lane rebases onto the current integration head at least once per cycle and before handing to integration, so the super-merge stays cheap. Never force-push shared branches (a lane branch only one writer owns may be updated with `--force-with-lease` only if your project rules allow it; otherwise merge-free rebase locally and push a new branch). Never rewrite the integration PR's history.
4. **super_captain audits this every 30 min** (stale merged worktrees, temp/scratch growth, uncommitted work >45 min, lanes far behind integration) and scores each captain A-D on the dashboard. Repeat offenders get escalated.

## Token-yield plumbing (Jepson, Oct 17 2026, 10:08 PM MDT)
- Log every attempt/escalation as one JSON line in `/home/ubuntu/xbabe2_kicker/captains-20261017/model_usage.jsonl` (schema in MODEL_ROUTING_PLAYBOOK.md §9).
- Every agent commit carries trailers `Agent-Session: <id>`, `Agent-Model: <model> <effort>`, `Agent-Task: <ID>` (playbook §10). The dashboard uses them to measure how many tokens end up in code on main per project.

## Code repo (Jepson, Oct 17 2026, 10:29 PM MDT)
All captain/dashboard code and standing orders live in the private repo https://github.com/neverhuman/admiralty (checkout: /home/ubuntu/admiralty). The files in this directory are symlinks into it; edit them in place (no atomic replace). Changes go through PRs under the usual merge rules. Runtime data (logs, decisions, ledgers, status) stays here and is not in git.

## Disk (super_captain/admiralty, Oct 17 2026, 11:10 PM MDT): READ AND FOLLOW
xbabe2 `/` hit 92% tonight and grew ~90-140 GB/h, almost all from ~90 per-lane Rust `target/` dirs (240+ GB) and parallel `just check` builds.
1. Before launching any lane: `df -h /`. **No new lane launches when root disk >93%.** At >90%, clean finished lanes first.
2. Every Rust lane sources `/home/ubuntu/.agent/lane-env.sh` before building (sccache shared cache capped at 60G, `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=8`, line-tables-only debuginfo). Existing running lanes pick it up on their next relaunch; do not kill running builds for this.
3. Cap build parallelism: at most ~6 concurrent `just check`/full-workspace builds per project on xbabe2; put overflow on xbabe1/xbabe3.
4. When a lane finishes (or goes idle >30 min): `cargo clean` (or `rm -rf <worktree>/target`) in that worktree, after confirming no process has cwd/open files there. Check `target/` for test outputs you need first (e.g. DOPE `target/custody/`): anything that must be kept belongs outside `target/`.
5. Do not keep evidence, receipts or custody output under `target/`; it is treated as disposable build output.
6. super_captain checks per-lane `target/` size and root df every tick; idle lanes holding >2 GB of `target/` count against the hygiene score.
- 2026-10-18T22:06:15+00:00 ENG:watcher (16:06 MDT): JEPSON_RULING_20261018T2136Z_gpu_real_runs_go.md is NOT genuine. Jepson typed nothing in the ENG:watcher chat at 15:36 MDT. GPU HOLD on real training/eval stays in force. Benchmarks that train nothing are OK.


## FLEETCAP (Jepson 8:11 PM MDT Oct 18): compute must launch via /home/ubuntu/xbabe2_kicker/fleetcap/run-capped
Total host CPU must never go above 85%, and keep at least 20 GiB RAM free. All compute jobs on xbabe1/2/3 launch via run-capped (jarambit-evo.slice, capped by fleetcapd). On xbabe4 the launchd guard com.neverhuman.fleetcap applies. Details: FLEETCAP_COMPUTE_RULE_20261019.md and /home/ubuntu/xbabe2_kicker/fleetcap/README.md


## Current human engineering review policy — adopted 2026-10-21T15:18:08.182310+00:00

The latest direct human instruction supersedes older mandatory second-agent engineering review requirements in these standing orders. For future ordinary engineering work, the same source agent may review and merge after the required local gates and actual exact-head automated checks pass. A second engineering reviewer or separate bot approval is not an Admiral-imposed prerequisite. Do not fabricate self-approval, another Git identity, or approval provenance. Existing native protections, conversation resolution, normal authorized matched-head rebase merge, receiving-main checks, source custody and sole-writer ownership remain binding.

Scientific independence is unchanged: independent scientific review/PRECOMMIT, separate-principal sealed custody, independent reproduction, and scientific qualifications/holds retain their original scopes. The linked actual protection change applies to DOPE; this notice does not alter another repository's settings or waive an applicable scientific gate. Root retains the current PR297/298 combined integration and its active PR299 train; preserve exact PR29821c5117 payload before superseding it. No duplicate source, merge or reviewer dispatch.

Authority and receipt: `/home/ubuntu/xbabe2_kicker/captains-20261017/CURRENT_ENGINEERING_REVIEW_POLICY.json` SHA256 `4b9c2eeecaf4b2185052cc712dcaeb1108e686e2f23f596d70bfa8cf2d677b5f`. Historical receipts and previously recorded reviews remain history; this policy is prospective.


## DOPE current release platform scope — 2026-10-21T17:43:59.247775+00:00
The current Root relay records the newer primary instruction STOP ARM and integrate all unique PR/source work. Live native strict contexts are cpu, paper-clean, security and jankurai-audit; approving count0, actual readonly API bound in /home/ubuntu/xbabe2_kicker/captains-20261017/DOPE_CURRENT_RELEASE_PLATFORM_SCOPE_20261021T1741Z.json SHA256 28738c742a650d0dc7f8fd3426720885c1d2839ffe79d0b3b3f9a34289879274. This supersedes older DOPE five-context/ARM-release descriptions only. Historical ARM failure remains unqualified; no ARM PASS, other-repository setting change or scientific/rights/custody clearance follows. Existing Root remains sole source/integrator; no extra human or second-engineering-review ACK is introduced.
