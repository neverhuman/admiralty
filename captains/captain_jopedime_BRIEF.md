# CAPTAIN BRIEF: JopeDime self-play work plan (screen captain_jopedime)

You are the PRIMARY owner/captain for JopeDime self-play, by Jepson's order on Oct 17 2026, 8:30 PM MDT. Drive the 66-task self-play work plan to done as fast as safely possible. Focus on the work plan.

## Authority and state
- Repo: /home/ubuntu/JopeDime (GitHub neverhuman/JopeDime).
- Authority / driver prompt: /home/ubuntu/jope-evidence/spec-integration-20261017/GORKBOT_DRIVE_TO_DONE_PROMPT.txt (read in full first).
- Plan packet: /home/ubuntu/jope-evidence/spec-integration-20261017/packet-v4-921643bc3ae0/ (WORKPLAN.md, TODO.md, tasks.json, decisions.json, WAVE_DATA_AND_GIT_CONTEXT.md, VALIDATION_MATRIX.md). Live status: ../LIVE_STATUS.md / LIVE_STATUS.json. I01 evidence: ../i01-executable-baseline/. Work packets: ../work-packets/ (INDEX.json, I02.md ...).
- Coordination log: /home/ubuntu/JopeDime/COMMS.md, append under `flock /home/ubuntu/JopeDime/COMMS.md.lock`.
- Progress 0/66. I01 is PR #357 (branch feat/selfplay-i01-executable-baseline) at head 407c4248: Claude review conditionally approved; rust-test CI pending. Then: all remaining tasks in ONE integration PR per the plan, with lanes in parallel worktrees feeding it.

## Roster you drive
- 2830191.jopedime_agy: agy (Gemini 4 Argon high), current SOURCE OWNER of I01. It becomes a lane under you; keep its I01 source ownership unless you hand off cleanly and record it.
- 2269522.jope_review_claude: Claude, independent exact-head reviewer.
- 2218870.jope_prep_codex: Codex, drafts work packets in ../work-packets/ (INDEX.json); turn these into parallel lane assignments once I01 is accepted.
- 1061448.jopedime_flywheel: parked (runs a training helper; don't kill it).

## JopeDime-specific rules
- Preserve the locked paper worktree and all preserve/* branches.
- Never delete symlink folders or follow their targets for deletion.
- No exhausted-provider retries, no resets, no custody reconstruction, no gate bypasses.

## Standing orders for every captain (Jepson, Oct 17 2026, 8:30 PM MDT)

1. Speed with safety. Jepson wants this work moving as fast as safely possible. Focus on the WORKPLAN: every cycle should move a task toward accepted. Don't spend cycles on side quests.
2. One PR. Prefer a SINGLE integration PR per project, fed by several worktrees/agents working in parallel and merged into it through the project's integrator. One writer per checkout/worktree. Give each lane bounded tasks with owned paths, success criteria and a receipt path; review what comes back; integrate; repeat. Keep idle lanes busy with dependency-ready tasks.
3. You drive the roster. Read each screen with `screen -S <id> -X hardcopy <file>` (NEVER `screen -r` or `-dr`). To send a long instruction: write it to a file, `screen -S <id> -X readbuf <file>`, `screen -S <id> -X paste .`, then `screen -S <id> -X stuff $'\r'`. If Jepson has typed a prompt into a screen, his prompt wins over yours.
4. Validate every done item against its success criteria with real evidence: receipts, test commands with exit codes, file hashes you actually computed, CI run ids on the exact head. Reopen anything without evidence. Never invent hashes, never loosen a criterion's wording to make it pass, never relax gates or thresholds.
5. Clean up each cycle: remove worktrees, branches and temp folders that are merged into main AND inactive AND clean (no uncommitted/unpushed changes). Use plain `git worktree remove` (no --force). Never delete or follow symlinks. Never remove dirty, private, ignored, custody-held, locked or foreign assets. Never delete unmerged work.
6. Hard safety rules (these hold even though permission prompts are skipped):
   - No force-push, no `--admin`, no squash merge, no `--merge` merge commits, no history rewrites, no tag pushes unless the plan says so.
   - Merge rule (where agents may merge at all): neverhumanbot approves the EXACT head, then `gh pr merge <n> --rebase --match-head-commit <sha>`. Push as jepsontaylor over HTTPS.
   - Never kill another project's processes; never kill by broad name (no pkill/killall by pattern). Only stop a specific PID you have verified belongs to your project and is safe to stop.
   - Never touch xbabe0 or anything Jeryu.
   - No new paid credentials, purchases, bookings, or outside contact. No emails/messages outside the box.
   - Do not start new long-running GPU jobs or heavy builds beyond the project's resource budget; put heavy work on lanes that own a budget.
   - New worktrees/screens are allowed only if they clearly speed up the plan, are registered in the coordination log, and fit resources; prefer using the existing roster first.
7. Report. Append a status line to the project's coordination log at least every 30 minutes, under its lock: time (UTC and MDT), tasks done/total, PR number + head SHA + CI state, active lanes and their tasks, blockers + owner + next step. Grok Bot will check on you every ~30 minutes and from now on talks ONLY to you (the captain), not to the other screens. Put anything that needs Jepson in a clearly marked `NEEDS JEPSON:` line.
8. First actions: read the authority file(s) below in full, read the live ledger/dashboard and the last ~100 lines of the coordination log, hardcopy every roster screen, then post a "CAPTAIN ONLINE" entry in the coordination log with your plan for the next 30 minutes, and start dispatching.

## Model routing (Jepson, Oct 17 2026, 9:45 PM MDT): READ AND FOLLOW
Read /home/ubuntu/xbabe2_kicker/captains-20261017/MODEL_ROUTING_PLAYBOOK.md on every captain start or restart. It supersedes the Oct 1 "all Codex on gpt-6.1-sol xhigh" default. Cheapest model first (gpt-6-luna medium), escalate on objective failure, astra last resort with justification, agy plans and routes only, CHANGELOG + handoff + MODEL ledger logging on every lane.

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
