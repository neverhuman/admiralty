//! Fast, deterministic Git health from already observed facts. Never runs Git.
use crate::views::{GitCommit, GitRepository};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitHealthState {
    Disabled,
    Healthy,
    Attention,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitHealthRelation {
    Equal,
    Ahead,
    Behind,
    Diverged,
    Unrelated,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitHealthCount {
    pub value: u32,
    /// True only when unobserved ancestry may add to this count.
    pub lower_bound: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitHealthIssue {
    pub code: String,
    pub severity: GitHealthState,
    pub ref_name: Option<String>,
    pub checkout_id: Option<String>,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitHealthBranch {
    pub ref_name: String,
    pub oid: String,
    pub relation: GitHealthRelation,
    /// Unknown counts are null, never guessed from a partial graph.
    pub ahead: Option<GitHealthCount>,
    pub behind: Option<GitHealthCount>,
    /// All maximal common ancestors proved by the loaded graph; full OIDs.
    pub merge_base_oids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitHealthCheckout {
    pub checkout_id: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub dirty: Option<bool>,
    pub staged: Option<u32>,
    pub unstaged: Option<u32>,
    pub untracked: Option<u32>,
    pub conflicts: Option<u32>,
    pub unique_commits: Option<GitHealthCount>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitHealthReport {
    pub schema_version: u32,
    pub instance_id: String,
    pub observed_utc: String,
    pub integration_ref: Option<String>,
    pub integration_oid: Option<String>,
    pub divergence_threshold: u32,
    pub observed_commits: u32,
    pub observed_refs: u32,
    pub observed_checkouts: u32,
    pub missing_parent_oids: Vec<String>,
    pub shallow: bool,
    pub truncated: bool,
    pub observation_error: bool,
    pub state: GitHealthState,
    pub actionable: bool,
    /// Content identity excludes observation time and commit messages/dates.
    pub fingerprint: String,
    pub issues: Vec<GitHealthIssue>,
    pub branches: Vec<GitHealthBranch>,
    pub checkouts: Vec<GitHealthCheckout>,
}

struct Ancestry<'a> {
    seen: BTreeSet<&'a str>,
    complete: bool,
}

fn ancestry<'a>(tip: &'a str, graph: &BTreeMap<&'a str, &'a GitCommit>) -> Ancestry<'a> {
    let mut seen = BTreeSet::new();
    let mut pending = vec![tip];
    let mut complete = true;
    while let Some(oid) = pending.pop() {
        if !seen.insert(oid) {
            continue;
        }
        if let Some(commit) = graph.get(oid) {
            pending.extend(commit.parents.iter().map(String::as_str));
        } else {
            complete = false;
        }
    }
    Ancestry { seen, complete }
}

fn count(value: usize, lower_bound: bool) -> GitHealthCount {
    GitHealthCount {
        value: u32::try_from(value).unwrap_or(u32::MAX),
        lower_bound,
    }
}

fn compare(
    name: &str,
    oid: &str,
    integration: &Ancestry<'_>,
    graph: &BTreeMap<&str, &GitCommit>,
) -> GitHealthBranch {
    let branch = ancestry(oid, graph);
    let common = branch
        .seen
        .intersection(&integration.seen)
        .copied()
        .collect::<BTreeSet<_>>();
    // Remove every common ancestor that has a common child. Remaining common
    // nodes are maximal, including criss-cross merges with several merge bases.
    let mut bases = common.clone();
    for shared in &common {
        if let Some(commit) = graph.get(shared) {
            for parent in &commit.parents {
                bases.remove(parent.as_str());
            }
        }
    }
    let ahead = integration.complete.then(|| {
        count(
            branch.seen.difference(&integration.seen).count(),
            !branch.complete,
        )
    });
    let behind = branch.complete.then(|| {
        count(
            integration.seen.difference(&branch.seen).count(),
            !integration.complete,
        )
    });
    let relation = if !branch.complete || !integration.complete {
        GitHealthRelation::Unknown
    } else if common.is_empty() {
        GitHealthRelation::Unrelated
    } else {
        match (
            ahead.as_ref().map_or(0, |n| n.value),
            behind.as_ref().map_or(0, |n| n.value),
        ) {
            (0, 0) => GitHealthRelation::Equal,
            (_, 0) => GitHealthRelation::Ahead,
            (0, _) => GitHealthRelation::Behind,
            _ => GitHealthRelation::Diverged,
        }
    };
    GitHealthBranch {
        ref_name: name.into(),
        oid: oid.into(),
        relation,
        ahead,
        behind,
        merge_base_oids: bases
            .into_iter()
            .filter(|oid| graph.contains_key(oid))
            .map(str::to_owned)
            .collect(),
    }
}

/// Evaluate one immutable observer cut against an explicit full ref name.
/// A routine fork that is only ahead is healthy. Divergence needs both unique
/// commits and more integration-only commits than the configured threshold.
/// Incomplete scans never establish health or prove unrelated histories.
pub fn evaluate(
    repository: &GitRepository,
    integration_ref: Option<&str>,
    divergence_threshold: u32,
) -> GitHealthReport {
    let mut report = GitHealthReport {
        schema_version: 1,
        instance_id: repository.instance_id.clone(),
        observed_utc: repository.observed_utc.clone(),
        integration_ref: integration_ref.map(str::to_owned),
        integration_oid: None,
        divergence_threshold,
        observed_commits: count(repository.commits.len(), false).value,
        observed_refs: count(repository.refs.len(), false).value,
        observed_checkouts: count(repository.checkouts.len(), false).value,
        missing_parent_oids: vec![],
        shallow: repository.shallow,
        truncated: repository.truncated,
        observation_error: repository.error.is_some(),
        state: GitHealthState::Disabled,
        actionable: false,
        fingerprint: String::new(),
        issues: vec![],
        branches: vec![],
        checkouts: vec![],
    };
    let Some(integration_ref) = integration_ref else {
        // Unconfigured ancestry policy must not hide directly observed conflicts.
        for checkout in &repository.checkouts {
            let dirty = checkout.status.as_ref().map(working_dirty);
            let conflicts = checkout.status.as_ref().map(|s| s.conflicts);
            if conflicts.is_some_and(|n| n > 0) {
                report.issues.push(GitHealthIssue {
                    code: "merge_conflicts".into(),
                    severity: GitHealthState::Attention,
                    ref_name: checkout.branch.clone(),
                    checkout_id: Some(checkout.id.clone()),
                    detail: "Checkout contains unresolved merge conflicts.".into(),
                });
            }
            report.checkouts.push(GitHealthCheckout {
                checkout_id: checkout.id.clone(),
                head: checkout.head.clone(),
                branch: checkout.branch.clone(),
                detached: checkout.detached,
                dirty,
                staged: checkout.status.as_ref().map(|s| s.staged),
                unstaged: checkout.status.as_ref().map(|s| s.unstaged),
                untracked: checkout.status.as_ref().map(|s| s.untracked),
                conflicts,
                unique_commits: None,
            });
        }
        report.actionable = !report.issues.is_empty();
        if report.actionable {
            report.state = GitHealthState::Attention;
        }
        return finish(report);
    };
    let mut issue = |code: &str, severity, detail: &str| {
        report.issues.push(GitHealthIssue {
            code: code.into(),
            severity,
            ref_name: None,
            checkout_id: None,
            detail: detail.into(),
        });
    };
    if repository.error.is_some() {
        issue(
            "observation_error",
            GitHealthState::Unknown,
            "Git observation failed; retained facts cannot establish current health.",
        );
    }
    if repository.shallow {
        issue(
            "shallow_history",
            GitHealthState::Unknown,
            "Shallow history cannot establish complete ancestry.",
        );
    }
    if repository.truncated {
        issue(
            "truncated_observation",
            GitHealthState::Unknown,
            "Commit, ref or checkout inventory exceeded the observer window.",
        );
    }
    let graph = repository
        .commits
        .iter()
        .map(|c| (c.oid.as_str(), c))
        .collect::<BTreeMap<_, _>>();
    report.missing_parent_oids = repository
        .commits
        .iter()
        .flat_map(|c| c.parents.iter())
        .filter(|oid| !graph.contains_key(oid.as_str()))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if !report.missing_parent_oids.is_empty() {
        issue(
            "missing_parents",
            GitHealthState::Unknown,
            "Some original commit parents are outside the observed graph window.",
        );
    }
    if graph.len() != repository.commits.len()
        || repository
            .commits
            .iter()
            .any(|c| !full_oid(&c.oid) || c.parents.iter().any(|parent| !full_oid(parent)))
        || !acyclic(&graph)
        || repository.refs.iter().any(|r| {
            r.commit_oid
                .as_deref()
                .is_some_and(|oid| !full_oid(oid) || !graph.contains_key(oid))
        })
    {
        issue(
            "invalid_graph",
            GitHealthState::Unknown,
            "Ancestry requires unique commits with complete object identities.",
        );
    }
    let integration = repository
        .refs
        .iter()
        .find(|r| r.name == integration_ref)
        .and_then(|r| r.commit_oid.as_deref());
    report.integration_oid = integration.map(str::to_owned);
    if integration.is_none() {
        report.issues.push(GitHealthIssue {
            code: "missing_integration_ref".into(),
            severity: GitHealthState::Unknown,
            ref_name: Some(integration_ref.into()),
            checkout_id: None,
            detail: "Configured integration ref was not observed as a commit; no ref is guessed."
                .into(),
        });
    }
    let integration = integration.map(|oid| ancestry(oid, &graph));
    let mut branches = repository
        .refs
        .iter()
        .filter(|r| matches!(r.kind.as_str(), "branch" | "remote"))
        .filter_map(|r| r.commit_oid.as_deref().map(|oid| (r.name.as_str(), oid)))
        .collect::<Vec<_>>();
    branches.sort_unstable();
    for (name, oid) in branches {
        let Some(integration) = &integration else {
            continue;
        };
        let branch = compare(name, oid, integration, &graph);
        let problem = match branch.relation {
            GitHealthRelation::Unknown => Some((
                "missing_ancestry",
                GitHealthState::Unknown,
                "Observed graph has missing parents or a missing tip; ancestry is unknown.",
            )),
            GitHealthRelation::Unrelated => Some((
                "unrelated_branch",
                GitHealthState::Attention,
                "Complete observed histories have no common ancestor; integration review required.",
            )),
            GitHealthRelation::Diverged
                if branch
                    .behind
                    .as_ref()
                    .is_some_and(|n| n.value > divergence_threshold) =>
            {
                Some((
                    "diverged_branch",
                    GitHealthState::Attention,
                    "Branch has unique work and exceeds the integration divergence threshold.",
                ))
            }
            _ => None,
        };
        if let Some((code, severity, detail)) = problem {
            report.issues.push(GitHealthIssue {
                code: code.into(),
                severity,
                ref_name: Some(name.into()),
                checkout_id: None,
                detail: detail.into(),
            });
        }
        report.branches.push(branch);
    }
    for checkout in &repository.checkouts {
        let dirty = checkout.status.as_ref().map(working_dirty);
        let conflicts = checkout.status.as_ref().map(|s| s.conflicts);
        let checkout_ancestry = checkout.head.as_deref().map(|oid| ancestry(oid, &graph));
        let unique_commits = integration.as_ref().and_then(|integration| {
            integration.complete.then_some(())?;
            checkout_ancestry.as_ref().map(|checkout| {
                count(
                    checkout.seen.difference(&integration.seen).count(),
                    !checkout.complete,
                )
            })
        });
        let mut add = |code: &str, severity, detail: &str| {
            report.issues.push(GitHealthIssue {
                code: code.into(),
                severity,
                ref_name: checkout.branch.clone(),
                checkout_id: Some(checkout.id.clone()),
                detail: detail.into(),
            });
        };
        if conflicts.is_some_and(|n| n > 0) {
            add(
                "merge_conflicts",
                GitHealthState::Attention,
                "Checkout contains unresolved merge conflicts.",
            );
        }
        if dirty == Some(true)
            && checkout.branch.as_deref().is_some_and(|name| {
                name == integration_ref || format!("refs/heads/{name}") == integration_ref
            })
        {
            add(
                "integration_worktree_dirty",
                GitHealthState::Attention,
                "Configured integration branch checkout has uncommitted files; review its working state before integration or cleanup.",
            );
        }
        if checkout.detached
            && (dirty == Some(true) || unique_commits.as_ref().is_some_and(|n| n.value > 0))
        {
            add(
                "detached_work",
                GitHealthState::Attention,
                "Detached checkout has dirty files or work absent from the integration ref; preserve before cleanup.",
            );
        }
        if checkout.status.is_none() && checkout.kind != "bare" && !checkout.prunable {
            add(
                "working_state_unknown",
                GitHealthState::Unknown,
                "Working state could not be observed; cleanliness is unknown.",
            );
        }
        if checkout_ancestry.as_ref().is_some_and(|a| !a.complete) {
            add(
                "checkout_ancestry_unknown",
                GitHealthState::Unknown,
                "Checkout HEAD ancestry exceeds the loaded graph.",
            );
        }
        report.checkouts.push(GitHealthCheckout {
            checkout_id: checkout.id.clone(),
            head: checkout.head.clone(),
            branch: checkout.branch.clone(),
            detached: checkout.detached,
            dirty,
            staged: checkout.status.as_ref().map(|s| s.staged),
            unstaged: checkout.status.as_ref().map(|s| s.unstaged),
            untracked: checkout.status.as_ref().map(|s| s.untracked),
            conflicts,
            unique_commits,
        });
    }
    report.state = if report
        .issues
        .iter()
        .any(|i| i.severity == GitHealthState::Unknown)
    {
        GitHealthState::Unknown
    } else if report.issues.is_empty() {
        GitHealthState::Healthy
    } else {
        GitHealthState::Attention
    };
    report.actionable = !report.issues.is_empty();
    finish(report)
}

fn acyclic(graph: &BTreeMap<&str, &GitCommit>) -> bool {
    let mut child_counts = BTreeMap::<&str, usize>::new();
    for commit in graph.values() {
        for parent in &commit.parents {
            if graph.contains_key(parent.as_str()) {
                *child_counts.entry(parent).or_default() += 1;
            }
        }
    }
    let mut tips = graph
        .keys()
        .copied()
        .filter(|oid| !child_counts.contains_key(oid))
        .collect::<Vec<_>>();
    let mut visited = 0;
    while let Some(oid) = tips.pop() {
        visited += 1;
        for parent in &graph[oid].parents {
            if let Some(n) = child_counts.get_mut(parent.as_str()) {
                *n -= 1;
                if *n == 0 {
                    tips.push(parent);
                }
            }
        }
    }
    visited == graph.len()
}

fn full_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit())
}

fn working_dirty(state: &crate::views::GitWorkingState) -> bool {
    state.staged > 0 || state.unstaged > 0 || state.untracked > 0 || state.conflicts > 0
}

fn finish(mut report: GitHealthReport) -> GitHealthReport {
    report.issues.sort_by(|a, b| {
        (&a.code, &a.ref_name, &a.checkout_id).cmp(&(&b.code, &b.ref_name, &b.checkout_id))
    });
    report
        .checkouts
        .sort_by(|a, b| a.checkout_id.cmp(&b.checkout_id));
    let observed = std::mem::take(&mut report.observed_utc);
    // All fields are serializable primitives; serialization has no fallible I/O.
    report.fingerprint = crate::content_digest(&report).unwrap_or_default();
    report.observed_utc = observed;
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::{GitCheckout, GitRef, GitWorkingState};

    fn oid(n: usize) -> String {
        format!("{n:040x}")
    }
    fn commit(n: usize, parents: &[usize]) -> GitCommit {
        GitCommit {
            oid: oid(n),
            parents: parents.iter().map(|n| oid(*n)).collect(),
            subject: format!("commit {n}"),
            committed_utc: "2001-01-01T00:00:00Z".into(),
        }
    }
    fn branch(name: &str, n: usize) -> GitRef {
        GitRef {
            name: format!("refs/heads/{name}"),
            oid: oid(n),
            commit_oid: Some(oid(n)),
            kind: "branch".into(),
        }
    }
    fn checkout(n: usize) -> GitCheckout {
        GitCheckout {
            id: format!("checkout-{n}"),
            name: "worker".into(),
            kind: "linked".into(),
            head: Some(oid(n)),
            branch: Some("worker".into()),
            detached: false,
            locked: false,
            prunable: false,
            status: Some(GitWorkingState::default()),
        }
    }
    fn repository() -> GitRepository {
        GitRepository {
            instance_id: "fixture:repository".into(),
            node: "fixture".into(),
            observed_utc: "2026-10-20T00:00:00Z".into(),
            error: None,
            shallow: false,
            truncated: false,
            commits: vec![commit(1, &[]), commit(2, &[1]), commit(3, &[1])],
            refs: vec![branch("main", 2), branch("worker", 3)],
            checkouts: vec![checkout(3)],
        }
    }
    fn health(repository: &GitRepository) -> GitHealthReport {
        evaluate(repository, Some("refs/heads/main"), 0)
    }
    fn code(report: &GitHealthReport, code: &str) -> bool {
        report.issues.iter().any(|i| i.code == code)
    }

    #[test]
    fn divergence_uses_full_graph_identity_and_configured_threshold() {
        let repository = repository();
        let report = health(&repository);
        assert_eq!(report.state, GitHealthState::Attention);
        assert!(code(&report, "diverged_branch"));
        let worker = report
            .branches
            .iter()
            .find(|b| b.ref_name.ends_with("worker"))
            .expect("worker");
        assert_eq!(worker.relation, GitHealthRelation::Diverged);
        assert_eq!(worker.ahead, Some(count(1, false)));
        assert_eq!(worker.behind, Some(count(1, false)));
        assert_eq!(worker.merge_base_oids, vec![oid(1)]);
        assert_eq!(
            evaluate(&repository, Some("refs/heads/main"), 1).state,
            GitHealthState::Healthy
        );
    }

    #[test]
    fn ordinary_worker_fork_and_dirty_named_checkout_do_not_trigger_alert() {
        let mut repository = repository();
        repository.commits[2].parents = vec![oid(2)];
        repository.checkouts[0]
            .status
            .as_mut()
            .expect("status")
            .unstaged = 1;
        let report = health(&repository);
        assert_eq!(report.state, GitHealthState::Healthy);
        assert!(!report.actionable);
        assert_eq!(report.branches[1].relation, GitHealthRelation::Ahead);
    }

    #[test]
    fn incomplete_shallow_retained_error_and_missing_ref_never_become_healthy() {
        let mut truncated = repository();
        truncated.truncated = true;
        let mut shallow = repository();
        shallow.shallow = true;
        let mut missing = repository();
        missing.commits.remove(0);
        let mut failed = repository();
        failed.error = Some("last scan failed".into());
        let mut no_integration = repository();
        no_integration.refs.remove(0);
        for (repository, expected) in [
            (truncated, "truncated_observation"),
            (shallow, "shallow_history"),
            (missing, "missing_parents"),
            (failed, "observation_error"),
            (no_integration, "missing_integration_ref"),
        ] {
            let report = health(&repository);
            assert_eq!(report.state, GitHealthState::Unknown, "{expected}");
            assert!(code(&report, expected), "{expected}");
            assert!(report.actionable);
        }
    }

    #[test]
    fn missing_base_does_not_prove_unrelated_or_invent_unique_counts() {
        let mut repository = repository();
        repository.commits.remove(0);
        let report = health(&repository);
        assert!(!code(&report, "unrelated_branch"));
        let worker = &report.branches[1];
        assert_eq!(worker.relation, GitHealthRelation::Unknown);
        assert!(worker.ahead.is_none());
        assert!(worker.behind.is_none());
        assert!(worker.merge_base_oids.is_empty());
        assert_eq!(report.missing_parent_oids, vec![oid(1)]);
    }

    #[test]
    fn partial_unique_count_is_lower_bound_only_against_complete_other_side() {
        let mut repository = repository();
        repository.commits[2].parents = vec![oid(9)];
        let report = health(&repository);
        assert_eq!(report.branches[1].ahead, Some(count(2, true)));
        assert!(report.branches[1].behind.is_none());
        assert_eq!(report.state, GitHealthState::Unknown);
    }

    #[test]
    fn complete_unrelated_histories_require_review() {
        let mut repository = repository();
        repository.commits[2].parents.clear();
        let report = health(&repository);
        assert!(code(&report, "unrelated_branch"));
        assert_eq!(report.branches[1].relation, GitHealthRelation::Unrelated);
    }

    #[test]
    fn conflicts_and_detached_unique_or_dirty_work_preserve_evidence() {
        let mut repository = repository();
        let tree = &mut repository.checkouts[0];
        tree.detached = true;
        tree.branch = None;
        tree.status.as_mut().expect("status").conflicts = 2;
        let report = health(&repository);
        assert!(code(&report, "merge_conflicts"));
        assert!(code(&report, "detached_work"));
        assert_eq!(report.checkouts[0].conflicts, Some(2));
        assert_eq!(report.checkouts[0].head, Some(oid(3)));
        // Detached at integration HEAD still needs preservation when dirty.
        repository.checkouts[0].head = Some(oid(2));
        assert!(code(&health(&repository), "detached_work"));
        repository.checkouts[0].status = Some(GitWorkingState::default());
        assert!(!code(&health(&repository), "detached_work"));
    }

    #[test]
    fn absent_working_state_is_unknown_and_absent_policy_is_disabled() {
        let mut repository = repository();
        repository.checkouts[0].status = None;
        assert!(code(&health(&repository), "working_state_unknown"));
        let report = evaluate(&repository, None, 20);
        assert_eq!(report.state, GitHealthState::Disabled);
        assert!(!report.actionable);
    }

    #[test]
    fn configured_integration_checkout_dirty_alerts_without_guessing_intent() {
        let mut repository = repository();
        let checkout = &mut repository.checkouts[0];
        checkout.branch = Some("main".into());
        checkout.head = Some(oid(2));
        checkout.status.as_mut().expect("status").staged = 43;
        let report = health(&repository);
        assert!(code(&report, "integration_worktree_dirty"));
        // A different branch at the same OID does not inherit integration role.
        repository.checkouts[0].branch = Some("worker".into());
        assert!(!code(&health(&repository), "integration_worktree_dirty"));
    }

    #[test]
    fn conflicts_remain_actionable_when_ancestry_policy_is_disabled() {
        let mut repository = repository();
        repository.checkouts[0]
            .status
            .as_mut()
            .expect("status")
            .conflicts = 1;
        let report = evaluate(&repository, None, 20);
        assert!(code(&report, "merge_conflicts"));
        assert!(report.actionable);
        assert_eq!(report.state, GitHealthState::Attention);
    }

    #[test]
    fn cycles_duplicate_and_short_ids_are_invalid_not_healthy() {
        let mut cycle = repository();
        cycle.commits[0].parents = vec![oid(2)];
        let mut duplicate = repository();
        duplicate.commits.push(duplicate.commits[0].clone());
        let mut short_id = repository();
        short_id.commits[0].oid = "1234567".into();
        for repository in [cycle, duplicate, short_id] {
            let report = health(&repository);
            assert_eq!(report.state, GitHealthState::Unknown);
            assert!(code(&report, "invalid_graph"));
        }
    }

    #[test]
    fn fingerprint_ignores_observation_time_order_and_commit_age() {
        let mut repository = repository();
        let before = health(&repository);
        repository.observed_utc = "2026-10-20T05:00:00Z".into();
        repository.commits.reverse();
        repository.refs.reverse();
        repository.commits[0].committed_utc = "2030-01-01T00:00:00Z".into();
        repository.commits[0].subject = "changed private prose".into();
        let after = health(&repository);
        assert_eq!(before.fingerprint, after.fingerprint);
        repository.checkouts[0]
            .status
            .as_mut()
            .expect("status")
            .conflicts = 1;
        assert_ne!(before.fingerprint, health(&repository).fingerprint);
    }

    #[test]
    fn criss_cross_merge_retains_every_maximal_common_ancestor() {
        let mut repository = repository();
        repository
            .commits
            .extend([commit(4, &[2, 3]), commit(5, &[3, 2])]);
        repository.refs = vec![branch("main", 4), branch("worker", 5)];
        let report = health(&repository);
        assert_eq!(report.branches[1].merge_base_oids, vec![oid(2), oid(3)]);
    }

    /// Run explicitly in release mode. Reports measurements, never a timing
    /// assertion that can flake on a shared CI machine.
    #[test]
    #[ignore = "in-memory performance measurement; run explicitly with --release --ignored --nocapture"]
    fn benchmark_64_checkouts_512_commits() {
        let mut repository = repository();
        repository.commits = (1..=512)
            .map(|n| {
                if n == 1 {
                    commit(n, &[])
                } else {
                    commit(n, &[n - 1])
                }
            })
            .collect();
        repository.refs = (0..64)
            .map(|n| branch(&format!("worker-{n}"), 512 - n))
            .collect();
        repository.refs.push(branch("main", 500));
        repository.checkouts = (0..64).map(|n| checkout(512 - n)).collect();
        let started = std::time::Instant::now();
        for _ in 0..100 {
            let report = std::hint::black_box(evaluate(
                std::hint::black_box(&repository),
                Some("refs/heads/main"),
                20,
            ));
            assert_eq!(report.observed_checkouts, 64);
        }
        eprintln!(
            "git_health: 64 checkouts / 512 commits / 65 refs; 100 evaluations in {:?}; mean {:?}; 0 Git subprocesses",
            started.elapsed(),
            started.elapsed() / 100
        );
    }
}
