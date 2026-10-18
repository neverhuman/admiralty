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

## Joil-specific orders (captain_joil)

- Why the dashboard showed 0 in progress: the Joil ledger (docs/v5-plan/progress) only has `todo`, `review` and `done`. Active work lives in the supervisor status files and the COORD claims. The dashboard now also counts active supervisor stages and COORD `LANE` lines. Jepson read "0 in progress" as the captain failing, so make the parallel work visible with LANE lines.
- Lane pool: renewal-002 is in. The lanes are codex_lane1 on CPUs 36-37, lane2 on 38-39, lane3 on 90-91 and lane4 on 110-111. Each gets 6 GiB and 2.5 GiB scratch, a worktree at .private/worktrees/v5-codex-laneN and a branch codex/v5-laneN. Finish your captain-owned gorkbot/lane_supervisor.py (don't edit supervisor.py) and START ALL 4 LANES NOW. Stagger them and check the lease guard (load at or below 70, MemAvailable at or above 40 GiB) before each one.
- Task choice under the new strategy: V5-04 is Claude's (C007) and V5-08 is the Codex integrator's. Don't duplicate them. The DAG-blocked packets that only need V5-08's geometry interface (or interfaces already done) are the next code work, written against a DRAFT interface. Suggested assignments:
  - lane1: V5-10 (mesh/export, needs only V5-08)
  - lane2: V5-09 (geometry observation, needs only V5-08)
  - lane3: V5-11 (genome, needs V5-03 done and V5-08)
  - lane4: V5-14 (reference CFD evaluator code, needs V5-02 and V5-03 done and V5-10), or V5-26 (render recipe registry) if V5-14 would be compute-heavy
  Each lane records the V5-08 or V5-10 interface assumptions in its receipt. Coding and bounded tests only: no simulation, GPU or search.
- The Codex integrator's sequencing ("no duplicate lanes", serialize before dispatch) is replaced for dispatch by Jepson's strategy: fan out now, super-merge later. The integrator still owns integrating the lane branches into PR #6 and the shared public API contracts. Post each lane's scoped write claim in COORD.md under CONTROL/coord.lock before it writes.
- Lanes never push to PR #6 or merge it. Only Jepson merges PR #6. The Joil lease cap (12 cores, 36 GiB, 15 GiB scratch) is binding. If you need more lanes, write `NEEDS JEPSON: raise Joil lease to <N>` instead of borrowing CPUs.
