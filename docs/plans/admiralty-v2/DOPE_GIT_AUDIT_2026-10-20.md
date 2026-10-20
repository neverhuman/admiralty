# DOPE Git audit — 20 October 2026

Observation window: 2026-10-20 06:33–06:42 UTC (00:33–00:42 MDT). Repository evidence is a snapshot of active work. Machine evidence: [dope-git-audit-20261020.json](../../../receipts/integration/dope-git-audit-20261020.json).

## Finding and scope

The pictured trees have three different meanings:

- `dope/unified-spec-workplan-v2` is an older history whose 67 content-bearing commits already have patch-equivalent counterparts on main; its two remaining unique commits are empty.
- `captain/e03-pilot-runner-bridge` has different commit identities after rebase but exactly the same complete tree as main.
- `codex/e03-confined-control-worker` is one genuine pending commit directly on main, already in [PR #290](https://github.com/neverhuman/dope/pull/290).

The highest priority finding is separate from those branches: the canonical xbabe2 main checkout has 43 staged paths with 6,659 deleted lines. Thirty-one staged path states match the older workplan branch. These pending deletions remove later admission, qualification and installation code and tests. Owner intent is unknown. They are uncommitted; the audit does not claim that a regression was merged, or attribute malicious behavior.

The audit inspected every registered, accessible worktree in the located local, xbabe1, xbabe2 and xbabe3 DOPE repositories: eleven existing worktrees plus four missing/prunable registrations. Configured xbabe4 was reached and identified itself as `Mac-Studio.local`; no DOPE checkout appeared in the bounded home paths inspected. This was not a whole-disk search. The unrelated xbabe0 host was not contacted.

Repository source, refs, indexes, worktrees and processes were not changed. No fetch, merge, reset, rebase, checkout, cleanup or new product tests were run. After the user explicitly requested coordination with Captain and Admiral, notifications were appended to their documented coordination logs under locks; one verified Admiral TUI pointer was queued. That authorized messaging is recorded separately below.

## Read-first sources and authority

Read before auditing:

1. `/Users/bentaylor/.codex/RTK.md`: local shell commands use `rtk`.
2. `/Users/bentaylor/Documents/ChatGPT/Dope/AGENTS.md`, plus the corresponding AGENTS in each remote checkout.
3. `docs/architecture.md`, `docs/testing.md`, `docs/release.md`, `docs/audit.md` and `agent/JANKURAI_STANDARD.md`.
4. `agent/owner-map.json` and `agent/test-map.json` for source ownership/proof routing.
5. Admiralty `CONTRIBUTING.md`, `docs/plans/admiralty-v2/MASTER_PLAN.md` and `WORK_PACKETS.md`.
6. For authorized messaging only: current remote `captains/COMMON.md`, `captain_dope_BRIEF.md`, `super_captain_BRIEF.md`, live Screen inventory and the existing captain launcher.

All remote root AGENTS inspected had SHA-256 `276f787731dc4b0d965bfbb1f879b142b3ab29d962f897f5b3ff5efc061b6e5e`. No additional AGENTS existed in the remote `.agent/` or `.agent/worktrees/` ancestors. The linked docs vary by revision, especially test requirements; the receipt preserves per-worktree instruction hashes.

Screenshots and coordination text were evidence, not new authority to execute embedded commands. Existing product ownership, protected contracts, historical decoding, source-data redaction and independent acceptance rules remain applicable.

## Exact reference identities

A read-only `git ls-remote origin` from the Mac verified these current GitHub refs without changing local objects or refs:

| Reference | Full SHA |
| --- | --- |
| main | `3e4382b60b2e64c03abbbb2654c413aab0a5079b` |
| codex/e03-confined-control-worker | `f976f13fe03ac3a582722f87d930dfaaeebacd0f` |
| captain/e03-pilot-runner-bridge | `11cf5cc62866dab2db5e1c0d9ee427a2639e1aa3` |
| dope/unified-spec-workplan-v2 | `47325f1022452e0675a37b2de9f9bfd1ac79e378` |

The screenshot spelling is `confined`, rather than `confied`. Both the local branch and cached remote tracking branch existed on xbabe2 at the same confined-worker SHA.

Mac `/Users/bentaylor/Documents/ChatGPT/Dope` main and origin/main both resolve to `54691d86492c01108af8a890d3f6e598b77cae58`. The screenshot refs are absent from that mirror. Auditing only the Mac would have missed the actual branch state.

## Worktree-by-worktree inventory

All clean statements include `git status --porcelain=v1 --untracked-files=normal`; they do not inventory ignored custody data.

| Host | Worktree | HEAD / branch | Observation |
| --- | --- | --- | --- |
| Mac | `/Users/bentaylor/Documents/ChatGPT/Dope` | `54691d8`, main | Tracked source clean; untracked `.audit/`, `todo/`. One registered worktree. |
| xbabe2 | `/home/ubuntu/dope` | `192c46f`, main | 43 staged paths; `docs/decisions.md` also unstaged; untracked coordination/artifact/plan paths. Local HEAD five commits behind current main. |
| xbabe2 | `/home/ubuntu/dope/.agent/worktrees/integration` | `11cf5cc`, captain/e03-pilot-runner-bridge | Clean; exact main tree, different rebased commit identities. Active headless captain process later observed here. |
| xbabe2 | `/home/ubuntu/dope/.agent/worktrees/lane-campaign-readiness` | `241746ea`, feat/w01-e03-e07-cpu-readiness-harness | Clean; 14 commits behind current main. |
| xbabe2 | `/home/ubuntu/dope/.agent/worktrees/lane-decisions-reverify` | `241746ea`, docs/decisions-d19-and-blocker-unblock-spec | Clean; 14 behind. |
| xbabe2 | `/home/ubuntu/dope/.agent/worktrees/lane-durable-acceptance` | `f976f13`, codex/e03-confined-control-worker | Clean; current main plus one commit; existing PR290. |
| xbabe2 | `/home/ubuntu/dope/.agent/worktrees/lane-e07-bikeshare` | `241746ea`, codex/dope-sc-dec-106-e07 | Clean; 14 behind. |
| xbabe2 | `/home/ubuntu/dope/.agent/worktrees/rt-L2` | `241746ea`, codex/f65-01-w01 | Clean; 14 behind. |
| xbabe2 | `/home/ubuntu/dope/.agent/worktrees/verify-016068fc` | `241746ea`, detached | Clean verification tree; directory name is not its actual HEAD. |
| xbabe1 | `/home/ubuntu/dope/.agent/worktrees/v01-ci` | `402954ed9dc34c66c2a7c986b6d3000b6a58746a`, detached | Clean; separate repository with older cached main. |
| xbabe3 | `/home/ubuntu/dope/.agent/worktrees/v01-ci` | Same `402954ed`, detached | Clean; separate repository with older cached main. |

xbabe1 additionally lists `fast-5`, `fast-6`, `fast-7` and `fast-8` under `/home/ubuntu/dope/.agent/worktrees/`, each detached at `0ac99c68ecd697abe19d5bab488f620d5c51517b`. All four directories are missing and Git reports “gitdir file points to non-existent location.” These are stale registrations, not four active workers. No pruning occurred.

The `integration` and `tabsyn-runtime-adapter` directories on xbabe1/3 lack Git metadata. They are runtime/source directories, not valid worktrees in this audit. Their presence is not proof that a Git checkout is healthy or safe to delete.

Cross-host warning: xbabe1/3 local `dope/unified-spec-workplan-v2` is `d1f6d10075248ee9477011a474ff0100284e35fa`, unlike xbabe2/GitHub's `47325f10`. Comparing these older workplan states gives 59/83 raw unique commits and 35 changed files, +116/-542. Preserve the host and exact SHA in every claim; identical branch names across repositories do not establish identical contents.

## Ancestry, equivalence and merge options

“Behind/ahead” below means counts from `git rev-list --left-right --count origin/main...<branch>`.

| Branch | Merge base with current main | Behind / ahead | Content finding |
| --- | --- | --- | --- |
| canonical local main | `192c46f1eed43b0897a36dd6f2cb182d3214edeb` | 5 / 0 | Local checkout is stale and dirty. |
| confined worker | `3e4382b60b2e64c03abbbb2654c413aab0a5079b` | 0 / 1 | One new candidate; no divergent parent history. |
| captain bridge | `cf67e73e5a1dbb2001967ed23a4f5ef40ee00371` | 3 / 3 | All three commits patch-equivalent; whole tree exactly equals main. |
| old workplan | `2bfee82e05c264bdfb8ff26049a560a64d984817` | 124 / 69 | 67 equivalent patches, two empty commits. |
| control artifact adapters | `cf67e73e5a1dbb2001967ed23a4f5ef40ee00371` | 3 / 2 | Older sibling implementation; inspect overlap before any reuse. |

The captain/main whole-tree identity is `95b7893c2072fd900298150b3ed5dd17f63c28f4`. The rewritten pairs are:

| Captain SHA | Current-main SHA | Subject |
| --- | --- | --- |
| `5aae82a5ec8a1efee190d08cd1f306d0b3828144` | `40441b78` | Add signed pilot stage execution bridge |
| `230058353f9a41903a53786e5d7aeeaa4a4b2105` | `32cc61a5` | Require native pilot ABI verification |
| `11cf5cc62866dab2db5e1c0d9ee427a2639e1aa3` | `3e4382b6` | Add bounded controls and isolated sample handoff |

The two old-workplan unmatched commits are `e85b247ca5e7d10d2173cb6a32e200dbb75c1ca6` (“W01: verify official runtime pins; qualification held”) and `154bc5cc45278f9a667faa45e695d5ee09627a97` (“E05: record sealed confirmation custody blocker”). Both have zero `git diff-tree --no-commit-id --name-status -r` output. Their provenance may matter; they have no file changes to merge.

A legacy three-tree, read-only `git merge-tree <base> <main> <branch>` check produced:

- Old workplan: 33 conflict hunks, including README, ownership/test maps, docs/testing, Bikeshare protocol/runner/tests, lock pins, scheduler, Rust dispatch, claims rendering and requirements.
- Captain bridge: zero output, consistent with exact whole-tree equality.
- Confined worker: no conflict markers, consistent with its direct-parent relationship.
- Old artifact-adapter sibling: three conflict hunks in `research/transfer/tdl/ADAPTERS.md`, `generators.py` and `tests/test_tdl_access_trace.py`.

Legacy merge-tree is an advisory predictor; its exit zero does not mean conflict-free, and it is not an exact `ort` merge or a current hosted mergeability result. No merge candidate was written.

Recommended reconciliation: retain old proof refs and mark them superseded/content-equivalent in the observer. Do not merge old workplan merely to make graph lines join. The bridge needs an integration disposition, not another content merge. Use existing PR290 for the real one-commit delta. Evaluate the old artifact-adapter branch only for a specific missing behavior, with a semantic comparison and exact tests, before selectively porting anything.

## Regression risks in canonical main's index

At 06:33 and again 06:37 UTC, canonical main's staged diff was stable: 43 files, 196 insertions and 6,659 deletions. The worktree is not a pristine main candidate.

Concrete pending removals:

1. `research/transfer/campaign_admission.py`, `campaign_artifacts.py`, `campaign_cli.py`, `campaign_execution.py`, `campaign_native.py`, `campaign_runner.py` and their regression suites: removes later development-stage admission/execution/accounting interfaces.
2. `native_qualification.py`, `native_qualification_runtime.py`, `run_native_qualification.py`, their tests and later lock metadata: removes later actual runtime qualification surfaces.
3. `workplan_packet.py`, its tests and the explicit qualification section of `docs/testing.md`: removes the separation between source fixtures and an admitted real private packet.
4. `scripts/install-dope.sh`, `scripts/uninstall-dope.sh`, `tests/vnext_dope_install.rs`, `docs/install.md` and native release-smoke content: removes later installation support and its proof.
5. `rust/main/facade/dispatch.rs`: removes doctor output `executable_sha256`, executable byte count, OS/architecture, Cargo.lock hash, declared toolchain hash and GPU feature state.
6. `schemas/pilot-admission-v1.schema.json`, `pilot-execution-v1.schema.json`, release readiness and historical errata/plan files: removes associated contracts and governance evidence.

Thirty-one path states exactly match the old workplan tree, including matching deletions where that old tree also lacks a file. This establishes a rollback-shaped candidate; it does not identify the command or principal that created it. The remaining twelve modified paths include ownership/proof maps, acceptance tests and claims code, so blindly restoring everything would also risk losing unique work.

`docs/decisions.md` is `MM`: staged and unstaged bytes differ. `research/transfer/manifests/release-readiness-v1.json` is staged-deleted while an untracked file exists at the same pathname. Recording only HEAD or only `git diff` would miss those distinct states.

No fresh product regression was executed against this dirty candidate. These are source-level removals with concrete lost interfaces and proof, not claims that a particular test failed. The safer next action is preservation and owner clarification.

## Confined-worker candidate review and existing proof

`f976f13` changes twelve files, +643/-4. It adds:

- A Rust cdylib boundary ABI in `rust/control_isolation_ffi.rs`, reusing existing Landlock and TSYNC sampling boundaries.
- A pinned dedicated numeric-control worker in `research/transfer/tdl/control_boundary.py` and `control_isolated_worker.py`.
- Eight focused tests for actual source/FD/network/exec denial, exact deterministic samples, source removal, corrupt pins/ABI/artifacts, symlink/FIFO/decompression refusal, output shape/finite checks and redacted startup failure.
- Ownership/proof routing and documentation.

The API requires a caller-admitted trusted runtime and Linux x86_64. It is not automatically substituted into legacy generators. Successful fixture proof does not qualify an E03 scientific pilot, vendor SDK, custody, E06 or release.

Existing receipt `target/control-boundary-qualification-20261020/FINAL_CHECK_RESULT.json` in the durable lane has SHA-256 `9fc9b5174c8b2bb039a9d27ced28ac658a0dbde8e0688f5213631ac86c07f28a`. Its tested staged tree `52f2a350aa2015e261093d9bfc973de51927f9e3` is the committed candidate tree. This audit independently matched all twelve recorded source hashes to `git show f976f13:<path>` and all three recorded log hashes to actual log bytes:

| Existing check | Recorded exit | Recorded elapsed |
| --- | --- | --- |
| Focused control-boundary unittest, eight tests | 0 | 16.8 s |
| just fast | 0 | 377.3 s |
| just check | 0 | 478.5 s |

These are inspected prior executions, not test reruns by this auditor. The original proof ran the parent plus the exact staged patch before commit; it must not be relabeled as a clean-commit run.

PR290's existing publication receipt binds head `f976f13`, base `3e4382b6` and that tree. Saved native observation at 06:34:15 UTC had ARM64 success; CPU, paper-clean, security and Jankurai audit were still running, while public-smoke was skipped. A later saved observation at 06:39:16 UTC records **CPU failure** on exact `f976f13`, job `118488691672`; ARM64 remained successful, security/paper/audit were still running. Its SHA-256 is `4960d1396ae7d421b10285253c3df8e2e456ae00dbb14c3e7d3b793739a34083`. Failure cause was not established by this snapshot. Exact-head independent approval was pending. No current full hosted success is claimed.

## Ordered owner work plan

Objective: preserve unique work, explain the dirty canonical index, reconcile actual pending code through existing owner/review gates, and represent rewritten history truthfully. Non-goals: rewrite history for visual neatness, silently delete stale branches, change science/acceptance policy, or duplicate the active captain/PR290.

Dependencies: existing canonical source/index owner; captain/Admiral disposition; exact current ref snapshots; private custody destination; current project review rules. Completion requires a source-bound disposition for every dirty path and each observed branch, preserved original evidence, an exact resulting tree, actual applicable validation, and observer labels that distinguish ancestry from content equivalence.

1. **Existing owner records custody before mutation.** Preserve separately `git diff --cached --binary --full-index`, `git diff --binary --full-index`, `git status --porcelain=v1 -z`, current HEAD/ref identities, index hash/metadata, and an explicit inventory of untracked/ignored evidence. Keep private outputs outside public Git. The staged-deleted/untracked manifest and MM decisions file require separate byte-preserving copies. Record each file SHA-256 and storage location. Do not run `reset`, `checkout`, `restore`, `clean` or any worktree deletion first.
2. **Determine intended author and baseline.** Review existing ownership/lease receipts and recent command provenance with the owning captain. Explain whether the index is deliberate pending work, an export/review staging surface, or an accidental old-tree overlay. A missing explanation is a scoped hold on committing that candidate, not permission to discard it.
3. **Classify each staged/unstaged path.** For the 31 old-state matches, preserve later main behavior unless the owner supplies a reviewed replacement. For the twelve other changes, inspect and rescue unique content onto an owned clean candidate. Preserve the independent unstaged decisions and untracked manifest. Keep the original candidate unchanged until the rescue verifies.
4. **Keep the existing integration candidate.** Continue PR290 under its current owner and exact-head rules. Recheck source/base if main advances. A new candidate must retain the real Rust boundary tests and existing proof routing. Do not reopen or reproduce the three bridge commits already on main.
5. **Resolve old branch presentation.** Record a bounded “superseded by patch-equivalent commits” disposition for old workplan and “same complete tree” for bridge. Preserve provenance/empty blocker commits. No source merge is necessary for these labels.
6. **Review older adapter sibling only on demand.** Compare the exact behaviors in the three overlapping paths before any port. Require header permutation, target order and isolated sample-shape tests on resulting main behavior. Default to the already integrated implementation unless a concrete missing behavior is demonstrated.
7. **Treat stale worktrees as inventory findings.** For each old clean/detached tree and missing registration, establish active process/receipt/custody ownership. Cleanup is a separate owner action after preservation. The health observer may flag prunable metadata; it must not prune it automatically.
8. **Validate the resulting owned candidate.** Run the focused affected tests from `agent/test-map.json`, then required `just fast`, `just check`, security/audit and current exact-head review gates as applicable. Installer/doctor changes require `tests/vnext_dope_install.rs` and the native release smoke route; campaign/workplan changes require their actual qualification/refusal suites. Run from the candidate worktree, not dirty canonical main. Preserve failures and bind logs to exact source/tree/environment.
9. **Close with receipts.** Captain records final branch/path dispositions, resulting full SHAs, preserved custody references, actual checks and remaining limitations. Admiral verifies owner acknowledgment and that pending work remains with one integration owner. Mark resolved only after those receipts exist.

Stop conditions: unknown source/index writer, new simultaneous writer, changing refs during a claimed stable snapshot, missing/contradictory custody receipts, loss of an untracked or ignored unique asset, or any attempt to weaken existing review/acceptance gates. Continue unrelated read-only/UX/protector work while affected mutations are held.

Parallel writes are allowed only with disjoint scopes: Admiralty UI/protector implementation may proceed independently of DOPE source reconciliation. DOPE canonical index and active integration tree remain with their existing owners. Runtime logs and notifications are append-only coordination surfaces, not source integration worktrees.

## Fast Git-health design evidence

The actual xbabe2 fast probe took 94.213 ms total local subprocess time for eight worktree tracked statuses, one reference inventory and four selected ahead/behind relations. Individual statuses took about 8–11 ms. This excludes SSH overhead, untracked walking and patch-equivalence analysis; it is one warm sample, not a performance guarantee.

A fast Rust check should group worktrees by actual common Git directory, read refs once, compare only configured active/base relations, and use `git --no-optional-locks status --porcelain=v1 -z --untracked-files=no`. Record tracked-only scope explicitly. Use bounded command count, output bytes, wall time and branch count, with cancellation and unknown/partial states.

Raw divergence should request a cached secondary analysis keyed by both full SHAs. That analysis computes ancestry, empty commits, patch IDs and whole-tree equality. Only then classify “unmerged unique code,” “already landed equivalent content,” “stale historical ref” or “unknown.” This case demonstrates why raw ahead/behind counts alone would produce false alarms.

High-priority signals should include a dirty primary/integration checkout, staged deletion of protected/critical paths, HEAD movement under a writer lease, conflicts in owner scopes, stale real upstream evidence, and contradictory index/worktree states. Detached CI worktrees and prunable metadata need separate lower-priority labels. A health score must never hide unknown coverage or grant permission to repair/delete/merge.

## Captain and Admiral coordination

User explicitly authorized direct coordination after the initial read-only audit.

- DOPE Captain identity: Screen `2716151.captain_dope`; existing active headless Codex thread `01a1574d-f2ca-79b0-92d8-d41179b39644`, process observed in the integration worktree. Its Screen shows a shell/launcher history; no prose or keystrokes were sent there.
- Admiral identity: Screen `4060655.super_captain`; live descendant Codex verified before input.
- Full factual message `DOPE-GIT-AUDIT-20261020-0637Z` was durably appended at 06:40:23 UTC under `/home/ubuntu/dope/COMMS.md.lock` and the Admiral's `super_captain_INBOX.md.lock`.
- Private message file: `/home/ubuntu/xbabe2_kicker/captains-20261017/DOPE_GIT_AUDIT_20261020_vpmrncxl/NOTIFICATION.md`, SHA-256 `8a8e461859ece429440b14af749049bc8f9f78a1cb3680efbd9d4221ab602cde`.
- Admiral pointer was submitted to the verified Codex TUI, then queued using its displayed “tab to queue message” action. Postflight showed the alert under queued follow-up inputs with an empty composer.
- Request asks both roles to clarify canonical index ownership/intent and supply a safe owner-based integration plan; it does not order a reset, merge, deletion, new source writer or duplicate native run.

At this artifact version, durable delivery and Admiral queueing are verified; neither receipt is yet an acknowledgment or completed reconciliation. Bounded checks at 06:44 and 06:47 UTC found no new Captain coordination entry and the Admiral pointer still queued during another active turn. The existing captain process was not resumed concurrently. Private Screen captures remain outside Git. Append any actual role response as a new observation with its timestamp and artifact hash; preserve the distinction between delivery, consumption, owner acceptance and correction.

