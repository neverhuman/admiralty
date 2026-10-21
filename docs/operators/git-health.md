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

## Descriptive metrics

`status.json` also publishes `git_metrics`, keyed by project ID, from the same
cut as `git_health`. Metrics never raise alerts and are excluded from the health
fingerprint, so adding them did not change alert deduplication.

| Group | Fields |
| --- | --- |
| `refs` | Local branches, remote branches, tags and other refs. |
| `branches` | Branches compared with the integration ref and their relation counts (`equal`, `ahead_only`, `behind_only`, `diverged`, `unrelated`, `unknown`); `merged` (equal or behind: review candidates for cleanup, never deleted); `max_ahead`/`max_behind`; `unintegrated_commits` (distinct commits reachable from any branch but not integration); unmerged tips whose reported commit time is older than 7 and 30 days; `tip_time_unknown`; `oldest_unmerged_tip_utc`. |
| `checkouts` | Totals for linked, detached, locked, prunable, dirty, clean and **unknown** working state (never counted as clean), checkouts with conflicts and summed staged/unstaged/untracked/conflict file counts. |
| `integration` | Integration tip commit time and age, loaded first-parent length and whether it reached a root commit. |
| `activity` | For 24 h, 7 d and 30 d windows ending at observation time: integration first-parent commits, merges among them, and all loaded commits. |

Counts use the same `{value, lower_bound}` shape as health. A partial scan sets
`complete: false` and marks counts that may grow; values that cannot be
established (no policy, no clock, incomplete integration ancestry) are `null`.
Commit times are reported committer times: an old tip describes age, not
stalled or abandoned work. The **Fleet health** view and the
[agent brief](agent-brief.md) summarize these metrics.

This metric describes a bounded observation, not acceptance or merge safety.
Before integration, inspect full worktree status, exact diff, merge bases,
repository policy and combined candidate tests. A clean textual merge cannot
establish semantic correctness. Fix observation/configuration gaps before using
a health report to justify cleanup.

Validation:

```sh
rtk cargo test -p admiralty-core git_health --locked
rtk cargo test -p admiralty-core git_metrics --locked
rtk cargo test -p admiralty-observe git_health --locked
rtk cargo test -p admiralty-core --release git_health::tests::benchmark_64_checkouts_512_commits --locked -- --ignored --nocapture
```

The optional in-memory benchmark evaluates 64 checkouts, 512 commits and 65 refs
100 times, reporting elapsed and mean duration. It contains no subprocess or
filesystem work and intentionally asserts no timing threshold on shared CI.
Record the actual machine, build profile and result before making a latency
claim. The observer's existing bounded Git scan remains the dominant source cost.
