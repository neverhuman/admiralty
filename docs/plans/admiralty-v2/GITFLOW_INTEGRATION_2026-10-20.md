# Combined Git Flow integration — 20 October 2026

User mandate: resolve the pending Admiralty branches, combine linear history
compression with project chat/Git health, pass local checks before publication,
obtain green CI and required review, and merge into main. This packet succeeds
only when the combined PR is merged and its source is verified on main.

## Scope, dependencies and preservation

- Existing integration PR163 remains its owner's work until its required checks,
  exact-head approval and ordinary matched rebase merge finish. Its observed
  head is `2763be549442c5a5cb98576f6bb4c279bcceedef`. Its pending Rust retry follows
  a runner communication incident; no source failure is inferred from that
  incident. Do not replace its branch or start a competing PR.
- Reuse this chat's managed checkout:
  `/Users/bentaylor/.codex/worktrees/git-protector-chat/admiralty`.
  New unpublished integration branch: `codex/gitflow-unified`.
- Preserve published input refs `codex/git-protector-chat` at
  `d1f99e7d751f6628dc3e4d9d5582ec1ea5cb2764` and
  `codex/git-linear-collapse` at `acd73c4db959d0d05e304c16fe40e5352b3c8990`.
  Both input bases are `6e974790371454f60bcf946d69b388927dacfa56`.
- The original dirty checkout, independent worktrees, ignored/private custody
  and historical receipts are preserved. No resetting, stashing, pruning,
  force-pushing, source-history deletion or speculative branch merging.
- "All branches" requires classifying pending work versus old rebased aliases;
  non-ancestral history alone does not prove unique missing content. Inventory
  findings and exact-head owner dispositions belong in the final receipt.
- DOPE's dirty canonical index remains a separate unresolved custody question.
  This integration does not mutate DOPE or establish scientific qualification.
- Production installation/enrollment remains the sole controller's work. This
  user request authorizes source integration and merge, not a competing service
  or provider controller.

## Read-first order and owned paths

Read `AGENTS.md`, `captains/COMMON.md`, `MASTER_PLAN.md`, `EXECUTION.md`,
`docs/operators/release-status.md`, then both source-bound feature receipts and
`GIT_PROTECTOR_CHAT_PLAN.md`. Read the exact independent collapse review
`PRELIMINARY_REVIEW_ACD.json` and `COVERAGE_ACD.json`, copied with hashes into
private scratch. Review the complete combined GitFlow/geometry/CSS files after
resolving their seams, not only the conflict markers.

The integrator owns this checkout and Git operations, server/startup/auth,
shared wiring, docs, receipts, commits, full matrix and PR. The UI packet owns
only `apps/dashboard/src/features/live-views/GitFlow.tsx`, adjacent `geometry.ts`
and compression state/tests if needed, and
`apps/dashboard/e2e/{live-views,git-chat}.e2e.ts`. CSS changes require a concrete
layout need. The branch-audit packet is read-only. The role-coordination packet
only appends authorized locked coordination logs and collects evidence outside
source. No parallel agent writes the same file or runs competing builds.

Generated schemas/exports, production configuration, databases and private
transcripts are outside the write scope. No new Python source is required.

## Ordered implementation

1. Freeze inputs, inventory current refs/worktrees and observe PR163's actual
   head/check state. Notify its owner and Admiral, reserve the next one-PR slot,
   identify any further genuinely pending Admiralty lane and request a custody
   disposition for uncommitted foreign work.
2. Rebase the nine protector/chat commits on a new unpublished branch onto the
   observed PR163 source provisionally. Keep both additive execution-ledger
   sections. Cherry-pick collapse commits `c36e934`, `ebadc73`, `acd73c4`.
3. Resolve `GitFlow.tsx` explicitly: retain `layoutGit(repo, revealed, zoom)`,
   project-scoped focus, complete `orderedCommits` inventory, range anchors,
   chat/health minimum band height and all memo dependencies. CSS must retain
   range controls and bounded viewport chat layers together.
4. Repair independent review ACD-F1: global Collapse linear preserves explicit
   commit/ref/checkout selections and their inspector/focus route. Clear an
   interior selection only when it is actually hidden by that action.
5. Repair ACD-F2: per-range manual collapse remains authoritative during normal
   live tip growth under an unchanged matching query. Track user intent beyond
   the old interior OID set; a genuinely new search/focus/expand action can
   supersede it. Preserve project isolation and topology boundaries.
6. Scope camera lookup and its applied-focus identity by project, including
   identical commit OIDs in two projects. Expansion and camera movement must
   resolve the same project.
7. Add meaningful browser controls for both review findings and project focus.
   Run new controls against the unfixed combined source and preserve actual
   negative logs. Fix the chat pan fixture by expanding enough history to
   overflow, asserting real positive scrollLeft, then proving the bubble stays
   viewport-anchored. Do not weaken accessibility/auth/retry/simulation gates.
8. After actual PR163 merge, fetch receiving main, verify its tree/source and
   move the unpublished candidate onto it without rewriting input refs. Refresh
   overlap and ownership. No PR is opened while the slot remains occupied.
9. Run affected focused checks, then clean-source full `pnpm check`. Repair
   attributable failures, retain original logs and requalify affected paths.
   Record exact source/tree, versions, commands/exits, counts and artifact hashes.
10. Commit source first, then a source-bound evidence receipt. Freeze a changed
    source archive, exact base/head/tree manifest, binary patch/bundle and logs
    for the reserved independent reviewer. Preserve any prior findings and
    explicitly show their disposition; do not transfer an old-head approval.
11. Push as jepsontaylor over HTTPS and open one clean PR after the slot clears.
    Attach it to this task. CI failures require actual logs and diagnosis;
    infrastructure retry is bounded and owned, not a reason to alter assertions.
12. Obtain actual independent current-source PASS and eligible neverhumanbot
    approval of the exact final PR head through the existing verified transport.
    Verify actual GitHub actor and approved commit without reading credentials.
    Never self-approve as the author or spoof reviewer identity.
13. Require all current-head CI checks green; use ordinary
    `gh pr merge <PR> --rebase --match-head-commit <HEAD>` with no bypass. Fetch
    main and verify the merged feature paths/tree and PR state. Record the merge
    receipt and notify captain/Admiral. Retain foreign/dirty/custody work and
    only clean up owned inactive merged work through normal managed lifecycle.

## Validation and failure interpretation

Use Node24 and pnpm11.15.1, Rust1.95, bounded two compiler jobs and one Rust test
thread through `scripts/check/all.sh`. The host's default Cargo target is
inaccessible; use `/Users/bentaylor/Documents/ChatGPT/admiralty/target` only for
build output, never as authority over original source.

```sh
rtk proxy env PATH=/Users/bentaylor/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin:$PATH ADMIRALTY_TARGET_DIR=/Users/bentaylor/Documents/ChatGPT/admiralty/target pnpm check
rtk proxy gh pr checks <PR> --repo neverhuman/admiralty
rtk proxy gh pr view <PR> --repo neverhuman/admiralty --json headRefOid,reviews,statusCheckRollup,mergeStateStatus
rtk proxy gh pr merge <PR> --repo neverhuman/admiralty --rebase --match-head-commit <HEAD>
```

Expected artifacts: source-bound combined receipt in `receipts/integration/`,
raw retained local stage logs, negative regression logs, browser PNG/axe proof,
branch classification, exact independent path/blob coverage, hosted run IDs,
actual review actor/commit, merged PR/main identity and role-notice receipt.
No current-source pass, approval or merge is inferred from an earlier receipt.

Stop affected effects on failed required checks, mismatch of reviewed/published
head, unknown live/source ownership, data loss or private-route leakage. Keep
independent local work progressing. Do not add a fresh human permission step
for the user's already-authorized ordinary merge.

## Current record

Host Mac: protector/chat rebased provisionally onto2763 and collapse imported;
only additive ledger and the two GitFlow shared-state seams conflicted. Both
input published refs remain intact. Seven controlled pre-fix browser assertions
reproduce the two independent findings and project camera bug; an additional
filtered-project control reproduces intent loss outside the visible project.
Initial fixture/selector failures are retained separately and are not product
negative proof. Repairs preserve explicit anchors, manual collapse over growth,
project-scoped camera/intent and real horizontal chat pan. The combined affected
browser suites pass22/22; full exact-source qualification follows.

PR163 merged at07:54:06UTC after all five hosted checks passed and actual
neverhumanbot approved its exact head. Receiving main
`35b497838c7e78bd366f2a0fabb6a455692fd93e` has tree
`ff9bea2f58f9c1a97cafe878c4397319b96f005a`, exactly the2763 source tree. The slot
is clear. Move the unpublished candidate onto that actual main before freezing
the full local check. Final source and check/PR/review/merge evidence will be
recorded in the execution ledger, source receipt and retained final merge proof.
