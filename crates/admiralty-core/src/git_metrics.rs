//! Descriptive Git signals from one already observed repository cut.
//!
//! Metrics complement [`crate::git_health`]: health decides whether evidence is
//! actionable; metrics describe inventory, hygiene pressure and activity so a
//! dashboard or agent can rank attention without rereading the full graph.
//! They never run Git, never raise alerts and are published separately so the
//! health fingerprint (and therefore alert deduplication) is unchanged.
//!
//! Commit times are the observer's reported committer times. They describe
//! recorded activity; they do not establish stalled work, authorship or intent.
//! Every count that may grow when unobserved history arrives is marked
//! `lower_bound`, and values that cannot be established are `null`.
use crate::git_health::{GitHealthCount, GitHealthRelation, GitHealthReport, ancestry};
use crate::views::{GitCommit, GitRepository};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const SCHEMA_VERSION: u32 = 1;
const HOUR: i64 = 3_600;
const DAY: i64 = 24 * HOUR;
/// Age buckets for unmerged branch tips. Age describes the reported commit
/// time only; it never establishes stalled or abandoned work.
pub const TIP_AGE_DAYS: [i64; 2] = [7, 30];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitRefInventory {
    pub local_branches: u32,
    pub remote_branches: u32,
    pub tags: u32,
    pub other: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitBranchMetrics {
    /// Branches compared with the configured integration ref; zero without policy.
    pub compared: u32,
    pub equal: u32,
    pub ahead_only: u32,
    pub behind_only: u32,
    pub diverged: u32,
    pub unrelated: u32,
    pub unknown: u32,
    /// Equal or behind: already contained in integration. Review candidates for
    /// cleanup only; this module never deletes or recommends deleting a ref.
    pub merged: u32,
    pub max_ahead: Option<GitHealthCount>,
    pub max_behind: Option<GitHealthCount>,
    /// Distinct commits reachable from any branch tip but not from integration.
    pub unintegrated_commits: Option<GitHealthCount>,
    /// Unmerged (ahead/diverged/unrelated/unknown) tips whose reported commit
    /// time is older than 7 and 30 days. Descriptive age, not a stall verdict.
    pub unmerged_tips_older_7d: u32,
    pub unmerged_tips_older_30d: u32,
    /// Unmerged tips whose commit time is missing or unparsable.
    pub tip_time_unknown: u32,
    pub oldest_unmerged_tip_utc: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitCheckoutMetrics {
    pub total: u32,
    pub linked: u32,
    pub detached: u32,
    pub locked: u32,
    pub prunable: u32,
    pub dirty: u32,
    pub clean: u32,
    /// Working state not observed; never counted as clean.
    pub state_unknown: u32,
    pub with_conflicts: u32,
    pub staged_files: u32,
    pub unstaged_files: u32,
    pub untracked_files: u32,
    pub conflict_files: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitActivityWindow {
    pub window_hours: u32,
    /// Commits on the integration first-parent chain inside the window.
    pub integration_first_parent: Option<GitHealthCount>,
    /// Merge commits among those first-parent commits.
    pub integration_merges: Option<GitHealthCount>,
    /// Every loaded commit inside the window, on any ref; null without a clock.
    pub all_observed: Option<GitHealthCount>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitIntegrationMetrics {
    pub tip_committed_utc: Option<String>,
    /// Observation time minus reported tip time; negative values mean clock skew.
    pub tip_age_seconds: Option<i64>,
    /// Loaded first-parent chain length and whether it reached a root commit.
    pub first_parent_loaded: u32,
    pub first_parent_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitMetricsReport {
    pub schema_version: u32,
    pub instance_id: String,
    pub observed_utc: String,
    pub integration_ref: Option<String>,
    /// False when the scan was shallow, truncated, failed or lost parents.
    pub complete: bool,
    pub refs: GitRefInventory,
    pub branches: GitBranchMetrics,
    pub checkouts: GitCheckoutMetrics,
    pub integration: GitIntegrationMetrics,
    pub activity: Vec<GitActivityWindow>,
}

fn n(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn count(value: usize, lower_bound: bool) -> GitHealthCount {
    GitHealthCount {
        value: n(value),
        lower_bound,
    }
}

fn seconds(utc: &str) -> Option<i64> {
    utc.parse::<jiff::Timestamp>().ok().map(|t| t.as_second())
}

fn larger(
    current: Option<GitHealthCount>,
    next: Option<&GitHealthCount>,
) -> Option<GitHealthCount> {
    match (current, next) {
        (None, Some(next)) => Some(next.clone()),
        (Some(current), Some(next)) if next.value > current.value => Some(next.clone()),
        (Some(mut current), Some(next)) if next.value == current.value => {
            current.lower_bound |= next.lower_bound;
            Some(current)
        }
        (current, _) => current,
    }
}

/// Summarize one cut. `health` must be the report evaluated from the same
/// repository; its proved relations are reused instead of recomputed.
pub fn summarize(repository: &GitRepository, health: &GitHealthReport) -> GitMetricsReport {
    let graph = repository
        .commits
        .iter()
        .map(|c| (c.oid.as_str(), c))
        .collect::<BTreeMap<&str, &GitCommit>>();
    let observed = seconds(&repository.observed_utc);
    let complete = repository.error.is_none()
        && !repository.shallow
        && !repository.truncated
        && health.missing_parent_oids.is_empty();

    let mut refs = GitRefInventory::default();
    for r in &repository.refs {
        match r.kind.as_str() {
            "branch" => refs.local_branches += 1,
            "remote" => refs.remote_branches += 1,
            "tag" => refs.tags += 1,
            _ => refs.other += 1,
        }
    }

    let mut branches = GitBranchMetrics {
        compared: n(health.branches.len()),
        ..GitBranchMetrics::default()
    };
    let mut oldest: Option<(i64, &str)> = None;
    for branch in &health.branches {
        match branch.relation {
            GitHealthRelation::Equal => branches.equal += 1,
            GitHealthRelation::Ahead => branches.ahead_only += 1,
            GitHealthRelation::Behind => branches.behind_only += 1,
            GitHealthRelation::Diverged => branches.diverged += 1,
            GitHealthRelation::Unrelated => branches.unrelated += 1,
            GitHealthRelation::Unknown => branches.unknown += 1,
        }
        let merged = matches!(
            branch.relation,
            GitHealthRelation::Equal | GitHealthRelation::Behind
        );
        if merged {
            branches.merged += 1;
        }
        branches.max_ahead = larger(branches.max_ahead.take(), branch.ahead.as_ref());
        branches.max_behind = larger(branches.max_behind.take(), branch.behind.as_ref());
        if merged {
            continue;
        }
        let tip = graph
            .get(branch.oid.as_str())
            .and_then(|c| seconds(&c.committed_utc).map(|s| (s, c.committed_utc.as_str())));
        match (tip, observed) {
            (Some((tip, utc)), Some(now)) => {
                let age = now - tip;
                if age > TIP_AGE_DAYS[0] * DAY {
                    branches.unmerged_tips_older_7d += 1;
                }
                if age > TIP_AGE_DAYS[1] * DAY {
                    branches.unmerged_tips_older_30d += 1;
                }
                if oldest.is_none_or(|(seen, _)| tip < seen) {
                    oldest = Some((tip, utc));
                }
            }
            _ => branches.tip_time_unknown += 1,
        }
    }
    branches.oldest_unmerged_tip_utc = oldest.map(|(_, utc)| utc.to_owned());

    let integration_oid = health.integration_oid.as_deref();
    let integration_ancestry = integration_oid.map(|oid| ancestry(oid, &graph));
    if let Some(integration) = integration_ancestry.as_ref().filter(|a| a.complete) {
        // Union of every branch tip's ancestry minus integration's ancestry.
        let mut unintegrated = BTreeSet::new();
        let mut lower_bound = false;
        for branch in &health.branches {
            let tip = ancestry(branch.oid.as_str(), &graph);
            lower_bound |= !tip.complete;
            unintegrated.extend(tip.seen.difference(&integration.seen).copied());
        }
        // Missing tips/parents are not commits we can count; keep only loaded ones.
        unintegrated.retain(|oid| graph.contains_key(oid));
        if !health.branches.is_empty() {
            branches.unintegrated_commits = Some(count(unintegrated.len(), lower_bound));
        }
    }

    let mut checkouts = GitCheckoutMetrics {
        total: n(repository.checkouts.len()),
        ..GitCheckoutMetrics::default()
    };
    for c in &repository.checkouts {
        checkouts.linked += u32::from(c.kind == "linked");
        checkouts.detached += u32::from(c.detached);
        checkouts.locked += u32::from(c.locked);
        checkouts.prunable += u32::from(c.prunable);
        match &c.status {
            Some(s) => {
                let dirty = s.staged > 0 || s.unstaged > 0 || s.untracked > 0 || s.conflicts > 0;
                if dirty {
                    checkouts.dirty += 1;
                } else {
                    checkouts.clean += 1;
                }
                checkouts.with_conflicts += u32::from(s.conflicts > 0);
                checkouts.staged_files = checkouts.staged_files.saturating_add(s.staged);
                checkouts.unstaged_files = checkouts.unstaged_files.saturating_add(s.unstaged);
                checkouts.untracked_files = checkouts.untracked_files.saturating_add(s.untracked);
                checkouts.conflict_files = checkouts.conflict_files.saturating_add(s.conflicts);
            }
            None => checkouts.state_unknown += 1,
        }
    }

    // First-parent chain from the integration tip over the loaded graph only.
    let mut chain = Vec::new();
    let mut chain_complete = false;
    if let Some(mut oid) = integration_oid {
        let mut visited = BTreeSet::new();
        while let Some(commit) = graph.get(oid) {
            if !visited.insert(oid) {
                break; // invalid cyclic graph; health already reports it
            }
            chain.push(*commit);
            match commit.parents.first() {
                Some(parent) => oid = parent.as_str(),
                None => {
                    chain_complete = true;
                    break;
                }
            }
        }
    }
    let tip = chain.first();
    let integration = GitIntegrationMetrics {
        tip_committed_utc: tip.map(|c| c.committed_utc.clone()),
        tip_age_seconds: tip
            .and_then(|c| seconds(&c.committed_utc))
            .zip(observed)
            .map(|(tip, now)| now - tip),
        first_parent_loaded: n(chain.len()),
        first_parent_complete: chain_complete,
    };

    let activity = [24_u32, 24 * 7, 24 * 30]
        .into_iter()
        .map(|hours| {
            let since = observed.map(|now| now - i64::from(hours) * HOUR);
            let within = |c: &GitCommit| {
                since.is_some_and(|since| seconds(&c.committed_utc).is_some_and(|t| t >= since))
            };
            // A partial chain/graph can only add commits later: lower bound.
            let first_parent = since.filter(|_| integration_oid.is_some()).map(|_| {
                let inside = chain
                    .iter()
                    .copied()
                    .filter(|c| within(c))
                    .collect::<Vec<_>>();
                let merges = inside.iter().filter(|c| c.parents.len() > 1).count();
                (
                    count(inside.len(), !chain_complete),
                    count(merges, !chain_complete),
                )
            });
            GitActivityWindow {
                window_hours: hours,
                integration_first_parent: first_parent.as_ref().map(|(c, _)| c.clone()),
                integration_merges: first_parent.map(|(_, m)| m),
                all_observed: since.map(|_| {
                    count(
                        repository.commits.iter().filter(|c| within(c)).count(),
                        !complete,
                    )
                }),
            }
        })
        .collect();

    GitMetricsReport {
        schema_version: SCHEMA_VERSION,
        instance_id: repository.instance_id.clone(),
        observed_utc: repository.observed_utc.clone(),
        integration_ref: health.integration_ref.clone(),
        complete,
        refs,
        branches,
        checkouts,
        integration,
        activity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_health::evaluate;
    use crate::views::{GitCheckout, GitRef, GitWorkingState};

    fn oid(n: usize) -> String {
        format!("{n:040x}")
    }
    fn commit(n: usize, parents: &[usize], at: &str) -> GitCommit {
        GitCommit {
            oid: oid(n),
            parents: parents.iter().map(|n| oid(*n)).collect(),
            subject: format!("commit {n}"),
            committed_utc: at.into(),
        }
    }
    fn reference(name: &str, kind: &str, n: usize) -> GitRef {
        GitRef {
            name: name.into(),
            oid: oid(n),
            commit_oid: Some(oid(n)),
            kind: kind.into(),
        }
    }
    fn checkout(id: &str, n: usize, status: Option<GitWorkingState>) -> GitCheckout {
        GitCheckout {
            id: id.into(),
            name: id.into(),
            kind: "linked".into(),
            head: Some(oid(n)),
            branch: Some(id.into()),
            detached: false,
            locked: false,
            prunable: false,
            status,
        }
    }
    /// main: 1 <- 2 <- 4(merge 2,3) ; feature 3 off 1 (merged) ; old 5 off 1 ; fresh 6 off 4
    fn repository() -> GitRepository {
        GitRepository {
            instance_id: "fixture:repository".into(),
            node: "fixture".into(),
            observed_utc: "2026-10-20T00:00:00Z".into(),
            error: None,
            shallow: false,
            truncated: false,
            commits: vec![
                commit(1, &[], "2026-08-01T00:00:00Z"),
                commit(2, &[1], "2026-10-10T00:00:00Z"),
                commit(3, &[1], "2026-10-15T00:00:00Z"),
                commit(4, &[2, 3], "2026-10-19T12:00:00Z"),
                commit(5, &[1], "2026-08-15T00:00:00Z"),
                commit(6, &[4], "2026-10-19T23:00:00Z"),
            ],
            refs: vec![
                reference("refs/heads/main", "branch", 4),
                reference("refs/heads/feature", "branch", 3),
                reference("refs/heads/old", "branch", 5),
                reference("refs/heads/fresh", "branch", 6),
                reference("refs/remotes/origin/main", "remote", 4),
                reference("refs/tags/v1", "tag", 2),
            ],
            checkouts: vec![
                checkout("clean", 6, Some(GitWorkingState::default())),
                checkout(
                    "dirty",
                    5,
                    Some(GitWorkingState {
                        staged: 1,
                        unstaged: 2,
                        untracked: 3,
                        conflicts: 1,
                    }),
                ),
                checkout("unknown", 4, None),
            ],
        }
    }
    fn metrics(repository: &GitRepository) -> GitMetricsReport {
        summarize(
            repository,
            &evaluate(repository, Some("refs/heads/main"), 20),
        )
    }

    #[test]
    fn inventory_relations_and_tip_ages_use_proved_health_relations() {
        let report = metrics(&repository());
        assert!(report.complete);
        assert_eq!(
            report.refs,
            GitRefInventory {
                local_branches: 4,
                remote_branches: 1,
                tags: 1,
                other: 0
            }
        );
        let b = &report.branches;
        assert_eq!(b.compared, 5);
        // main + origin/main equal; feature behind (merged); fresh ahead; old diverged.
        assert_eq!(
            (b.equal, b.behind_only, b.ahead_only, b.diverged),
            (2, 1, 1, 1)
        );
        assert_eq!(b.merged, 3);
        assert_eq!(b.unintegrated_commits, Some(count(2, false)));
        // Only `old` (2026-08-15) is an unmerged tip older than 7 and 30 days.
        assert_eq!(
            (
                b.unmerged_tips_older_7d,
                b.unmerged_tips_older_30d,
                b.tip_time_unknown
            ),
            (1, 1, 0)
        );
        assert_eq!(
            b.oldest_unmerged_tip_utc.as_deref(),
            Some("2026-08-15T00:00:00Z")
        );
        assert_eq!(b.max_ahead, Some(count(1, false)));
        assert_eq!(b.max_behind, Some(count(3, false)));
    }

    #[test]
    fn checkouts_never_count_unknown_state_as_clean() {
        let c = metrics(&repository()).checkouts;
        assert_eq!((c.total, c.clean, c.dirty, c.state_unknown), (3, 1, 1, 1));
        assert_eq!(c.with_conflicts, 1);
        assert_eq!(
            (
                c.staged_files,
                c.unstaged_files,
                c.untracked_files,
                c.conflict_files
            ),
            (1, 2, 3, 1)
        );
    }

    #[test]
    fn activity_windows_follow_first_parent_and_mark_partial_chains() {
        let report = metrics(&repository());
        assert_eq!(
            report.integration.tip_committed_utc.as_deref(),
            Some("2026-10-19T12:00:00Z")
        );
        assert_eq!(report.integration.tip_age_seconds, Some(12 * HOUR));
        assert_eq!(report.integration.first_parent_loaded, 3);
        assert!(report.integration.first_parent_complete);
        let day = &report.activity[0];
        assert_eq!(day.window_hours, 24);
        assert_eq!(day.integration_first_parent, Some(count(1, false)));
        assert_eq!(day.integration_merges, Some(count(1, false)));
        assert_eq!(day.all_observed, Some(count(2, false)));
        let month = &report.activity[2];
        assert_eq!(month.integration_first_parent, Some(count(2, false)));
        assert_eq!(month.all_observed, Some(count(4, false)));

        let mut partial = repository();
        partial.commits.retain(|c| c.oid != oid(1));
        partial.truncated = true;
        let report = metrics(&partial);
        assert!(!report.complete);
        assert!(!report.integration.first_parent_complete);
        let month = &report.activity[2];
        assert!(
            month
                .integration_first_parent
                .as_ref()
                .expect("count")
                .lower_bound
        );
        assert!(month.all_observed.as_ref().expect("count").lower_bound);
        // Integration ancestry is incomplete: unique counts cannot be proved.
        assert!(report.branches.unintegrated_commits.is_none());
    }

    #[test]
    fn absent_policy_and_bad_clock_stay_unknown_instead_of_zero() {
        let mut repository = repository();
        let report = summarize(&repository, &evaluate(&repository, None, 20));
        assert_eq!(report.branches.compared, 0);
        assert!(report.branches.unintegrated_commits.is_none());
        assert!(report.integration.tip_committed_utc.is_none());
        assert!(
            report
                .activity
                .iter()
                .all(|w| w.integration_first_parent.is_none())
        );
        repository.observed_utc = "not a time".into();
        let report = metrics(&repository);
        assert!(report.integration.tip_age_seconds.is_none());
        assert!(report.activity.iter().all(|w| w.all_observed.is_none()));
        // Unmerged tips cannot be aged without an observation clock.
        assert_eq!(report.branches.tip_time_unknown, 2);
    }
}
