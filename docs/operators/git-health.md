# Git health and captain alerts

The Rust observer evaluates Git health once per collection cycle from its already
collected commit graph, refs and checkout states. Evaluation launches **zero Git
commands**, reads no repository files, performs no fetch, and never merges,
rebases, resets, deletes or repairs a repository. Browser requests read the
published report and do not trigger scans.

Add an explicit policy to each observer project that should be monitored:

```json
{
  "id": "example",
  "name": "Example",
  "repository": "/absolute/path/to/repository",
  "tracker": {},
  "git_health": {
    "integration_ref": "refs/heads/main",
    "divergence_threshold": 20
  }
}
```

`integration_ref` must name a complete `refs/heads/...` or `refs/remotes/...`
branch ref. Choose the actual integration policy for that repository; `main`,
`master`, the newest commit and a manager's checkout are never guessed. Remote
refs are the locally observed refs, with no claim about the current remote.
An absent policy produces `disabled` for ancestry checks; directly observed
conflicts still produce actionable findings. The default divergence threshold is 20
integration-only commits; allowed range is 0 through 100,000. Set 0 to flag every
proved fork with unique commits on both sides. A missing configured ref produces
`unknown`, never an implicit fallback.

`status.json` publishes `git_health`, keyed by configured project ID. Every report
contains schema version, repository instance identity, source observation time,
configured integration ref and its full OID, observed inventory counts, graph
coverage flags and missing parent OIDs, branch comparisons, checkout evidence,
issues, and a SHA-256 content fingerprint. The fingerprint excludes observation
time, commit subjects and commit dates; repeated observation of an unchanged
problem keeps the same identity. Checkout names and source private paths are not
needed for health evaluation.

States have a concrete meaning:

| State | Meaning |
| --- | --- |
| `disabled` | No integration policy was configured and no conflicts were observed. |
| `healthy` | Complete observed evidence contains no configured health trigger. |
| `attention` | Complete evidence proves divergence beyond policy, unrelated branch history, unresolved conflicts, or detached work to preserve. |
| `unknown` | A scan failed, history is shallow/truncated, parents/tips are missing, the graph is invalid, integration ref is absent, or an eligible checkout's working state is unavailable. Other proved issues remain visible. |

A branch that is only ahead is an ordinary worker fork. A branch that is only
behind has no unique work to reconcile. Neither alone raises divergence alerts.
A diverged branch is actionable only when its integration-only commit count is
greater than the configured threshold. The report shows every loaded local and
remote branch, full commit IDs, relation, ahead/behind counts, and all proved
maximal common ancestors. Counts are exact where ancestry is complete. A count
marked `lower_bound` may increase when missing ancestors arrive. Counts are null
when the opposite ancestry is incomplete and a set difference cannot be proved.
An absent merge base in a partial graph never proves unrelated histories.

Dirty files on an attached worker branch are normal work. Dirty files on the
explicitly configured integration branch produce `integration_worktree_dirty`:
review that working state before integration or cleanup. This finding makes no
claim about rollback intent, authorship or whether changes are authorized.
Unresolved conflicts always require attention, including when ancestry policy
is absent. Detached dirty work or commits absent from integration
raise a preservation issue; this does not claim those commits are absent from
every named ref. Unknown checkout status never becomes clean. Commit age does
not establish stalled work, and a disappearing or changed ref does not establish
a force push, lost work, or an agent's exit.

`git_health_alerts` contains deterministic changed actionable reports with project
ID, fingerprint, source observation time, state and unique issue codes. The
observer compares the last atomically published `status.json`, so deduplication
survives an observer restart. Unchanged reports remain visible in `git_health`;
alerts are change records for that cycle. Consumers should compare report
fingerprints against their own durable delivery receipts so a skipped polling
cycle does not lose a notification. Captain delivery is handled by the server's
configured command inbox. Evaluation itself sends no messages or model turns.

## Hygiene and activity metrics

Each report also carries a `metrics` block computed from the same observer cut.
Metrics are descriptive: they never change `state`, `actionable` or the
fingerprint, so they cannot create alerts or captain messages. Ages are relative
to the report's `observed_utc`, so a retained report gives the same numbers when
re-read.

| Field | Meaning |
| --- | --- |
| `local_branches` / `unmerged_branches` / `merged_branches` / `diverged_branches` | Local `refs/heads/*` other than the integration ref, split by relation. *Merged* means fully contained in the integration ref: a cleanup candidate once its owner confirms; Admiralty never deletes refs. |
| `remote_unmerged_refs` | Remote-tracking refs (excluding `*/HEAD`) with commits absent from integration: pushed but not integrated work. |
| `unknown_refs` | Refs whose ancestry left the loaded graph. |
| `max_unmerged_behind` / `max_unmerged_ahead` | How far the most-behind lane trails integration, and the largest pending batch. `lower_bound` follows the branch counts. |
| `oldest_unmerged_tip_age_s` | Age of the oldest unmerged local tip commit. Commit age is history, not proof of stalled work. |
| `dirty_checkouts`, `conflicted_checkouts`, `detached_checkouts`, `locked_checkouts`, `prunable_checkouts`, `unknown_state_checkouts` | Worktree counts. |
| `cleanup_candidate_checkouts` | Linked checkouts that are clean, unlocked, not prunable and whose HEAD is fully contained in integration: the captain hygiene "merged and clean" rule. Process activity is not observed; confirm before `git worktree remove`. |
| `integration_tip_age_s` | Age of the integration tip. |
| `integration_commits_24h` / `integration_commits_7d` | First-parent commits on the integration ref inside the window (`lower_bound` when the walk leaves the loaded graph first). |
| `integration_merges_7d` / `integration_reverts_7d` / `integration_fixups_7d` | Merge commits (zero under a rebase-only policy), `Revert "…"` commits and `fixup!`/`squash!`/`amend!` commits that reached integration. |

`history.jsonl` keeps a compact per-project sample of these metrics (`git:
{st, un, bh, dt, cc, c7}`) for trend plots, re-validated through the history
allowlist like every other retained metric.

This metric describes a bounded observation, not acceptance or merge safety.
Before integration, inspect full worktree status, exact diff, merge bases,
repository policy and combined candidate tests. A clean textual merge cannot
establish semantic correctness. Fix observation/configuration gaps before using
a health report to justify cleanup.

Validation:

```sh
rtk cargo test -p admiralty-core git_health --locked
rtk cargo test -p admiralty-observe git_health --locked
rtk cargo test -p admiralty-core --release git_health::tests::benchmark_64_checkouts_512_commits --locked -- --ignored --nocapture
```

The optional in-memory benchmark evaluates 64 checkouts, 512 commits and 65 refs
100 times, reporting elapsed and mean duration. It contains no subprocess or
filesystem work and intentionally asserts no timing threshold on shared CI.
Record the actual machine, build profile and result before making a latency
claim. The observer's existing bounded Git scan remains the dominant source cost.
