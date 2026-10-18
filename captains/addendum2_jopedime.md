## ADDENDUM 2: Phase "max parallel code" (Jepson's order, Oct 17 2026, 9:24 PM MDT)

Jepson's words: "I would prefer to quickly do ALL of the code work, and then figure out the super merger and fix. We want many agents working on these quickly."

This changes the dispatch strategy. It does NOT change any safety rule. Where it conflicts with earlier sequencing guidance from you or an integrator ("one lane at a time", "no duplicate lane", "serialize before dispatch"), this addendum wins for dispatch. Integrators still own the final integration.

1. Fan out now. Every workplan code task that is DAG-ready gets its own worktree, branch and agent screen now. So does every task whose dependencies can be stubbed or written against a draft interface (the dependency's current lane branch, its work packet or its spec). Aim to have ALL code tasks in progress as fast as the CPU and RAM guard allows. Keep starting lanes until your CPU slice is full or the guard stops you.
2. Lane rules:
   - Do a fresh `git fetch` first, then branch from the latest origin/main, or from the integration head where the plan requires it.
   - Use one worktree, one branch (lane/<TASK-ID> or the project's convention) and one writer per lane.
   - Write the code, test it in the lane's worktree with bounded tests, commit to the lane branch, and push the lane branch as jepsontaylor over HTTPS.
   - Write down the draft-interface assumptions the lane relied on, in the lane receipt.
   - Don't wait for each lane to merge before starting the next. Lanes don't merge into main by themselves.
3. Super merge later. When a wave of lanes reaches lane-complete, the integrator merges them into the project's single integration PR. Then it fixes conflicts and broken assumptions, reruns the full gates, and gets neverhumanbot to approve the exact head. Plan this as an explicit step.
4. Done still means verified after integration, with real evidence (commands with exit codes, computed hashes, CI on the exact head). Until then, track each lane as `in_progress`, then `lane_complete`. Never mark a task done from lane evidence alone, and never invent evidence.
5. Track every lane where the dashboard can see it. Put one line per lane in your coordination log, and in the ledger if your ledger has a state for it:
   `LANE <TASK-ID> | screen <name> | model <model> | worktree <path> | branch <branch> | cpus <a,b> | state in_progress|lane_complete|integrated|done | <UTC time>`
   The dashboard counts these lines as in progress, so keep them current.
6. Models:
   - Use Codex (gpt-6.1-sol xhigh, or Luna) freely. Jepson is adding more quota, so run out the current one.
   - Claude lanes are fine too.
   - Use agy/Gemini only where it still has quota. Grok's quota is used up, so don't start Grok lanes.
   - If a provider reports quota exhausted, log `NEEDS JEPSON: <provider> quota exhausted <time>` and switch providers.
7. Caps (these replace the earlier "3 per project / 10 total"):
   - Pin each lane with `taskset -c` to 2 CPUs inside YOUR CPU slice. The slices are listed in /home/ubuntu/xbabe2_kicker/captains-20261017/CPU_SLICES.md.
   - Set `nice 10`, CARGO_BUILD_JOBS=2, RAYON_NUM_THREADS=2, and OMP/OPENBLAS/MKL threads=1. Give each lane its own CARGO_TARGET_DIR or build dir inside the lane's area.
   - Guard: check before EVERY launch. Don't launch if the 1-minute load is above 85 or MemAvailable is below 25 GB. Stagger launches about 60 seconds apart.
   - If you fill your slice, you may borrow 2-CPU pairs from the SHARED overflow. First add a line claiming the pair to CPU_SLICES.md, and never take a pair someone else has claimed.
   - Never touch CI runners (actions-runners/*), dashboards, other projects' screens or processes, xbabe0, or Jeryu.
   - Leave xbabe3 alone unless it clearly has headroom, and never touch its runners.
8. Unchanged safety rules:
   - No force-push, no --admin, no squash, no --merge.
   - Merges go through neverhumanbot approval of the exact head, then `gh pr merge --rebase --match-head-commit`.
   - Joil PR #6 is merged by Jepson only.
   - No relaxing gates or thresholds, and no rewording criteria.
   - One writer per worktree.
   - Clean up only work that is merged, clean and inactive, using `git worktree remove` without --force.
   - Never delete unmerged work.
9. Act now. Acknowledge in your coordination log as "ADDENDUM 2 ACK" with the list of lanes you are launching, then launch them. Post the updated LANE lines within 15 minutes.

## JopeDime-specific orders (captain_jopedime)

- Jepson asked directly why Jarambit shows 31 in progress and JopeDime only 3. Target 10 or more simultaneous code lanes NOW.
- Use the 39 work packets in /home/ubuntu/jope-evidence/spec-integration-20261017/work-packets/ (I02.md through I66.md, finalized by jope_prep_codex) to launch lanes for every ready task, not just I02, I03 and I07.
  - I02 has its tests complete with docs pending, so treat its contract as the draft interface.
  - First wave, beyond I02, I03 and I07 already running: I46, I63, I04, I08, I09, I05, I11, I16, I27, I26, I64. Then I21, I28, I17 and I47 as their draft interfaces settle.
- Write an ASSIGNMENT_<ID>.md in captain-state/ for every lane, since the dashboard counts those, plus a LANE line in COMMS.md.
- Your CPU slice is 22-31 and 92-101 (10 lanes at 2 CPUs each). The existing jope screens on 16-19 stay where they are.
- jope_prep_codex is idle. Reuse it as a lane, or as the "super merge" planner that maps lane-branch overlaps for the integration branch.
- I01's squash-merge exception stands. From now on, rebase merges only.
