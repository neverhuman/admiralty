//! Pure, bounded scheduling proposals. This module never starts work.
use crate::{ProjectId, RootId, TaskContract, TaskId, content_digest};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
};

pub const SCHEDULER_SCHEMA_VERSION: u32 = 1;
pub const MAX_SCHEDULED_TASKS: usize = 1000;
pub const MAX_SCHEDULER_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SchedulingScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Knowledge<T> {
    Known(T),
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskSchedulingState {
    Pending,
    Active,
    Completed,
    Blocked,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum LeaseObservation {
    Absent,
    Held { expires_tick: Option<u64> },
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScheduledTask {
    pub contract: TaskContract,
    pub contract_digest: String,
    pub observed_contract_revision: u32,
    pub state: TaskSchedulingState,
    pub lease: LeaseObservation,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SchedulerSnapshot {
    pub schema_version: u32,
    pub scope: SchedulingScope,
    pub tasks: Vec<ScheduledTask>,
    /// Caller-observed remaining task slots, not a permit or an inferred token balance.
    pub available_quota: Knowledge<u16>,
    /// Caller-observed remaining task slots, not a resource reservation.
    pub available_capacity: Knowledge<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessBlocker {
    DependencyNotCompleted {
        dependency: TaskId,
        observed_state: TaskSchedulingState,
    },
    LeaseHeld {
        expires_tick: Option<u64>,
    },
    LeaseUnknown,
    QuotaUnknown,
    CapacityUnknown,
    QuotaLimit,
    CapacityLimit,
    NotPending,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SchedulingProposal {
    pub scope: SchedulingScope,
    pub source_digest: String,
    pub ready: Vec<TaskId>,
    pub blocked: BTreeMap<TaskId, Vec<ReadinessBlocker>>,
    /// A proposal is never an execution authorization.
    pub execution_granted: bool,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum SchedulerError {
    #[error("scheduler snapshot exceeds the 1 MiB input limit")]
    SnapshotTooLarge,
    #[error("invalid scheduler snapshot JSON: {0}")]
    Json(String),
    #[error("unsupported scheduler snapshot schema version")]
    UnsupportedVersion,
    #[error("scheduler scope revision must be nonzero")]
    InvalidScopeRevision,
    #[error("scheduler snapshot scope does not match the expected scope")]
    ScopeMismatch,
    #[error("scheduler snapshot exceeds the 1000 task limit")]
    TooManyTasks,
    #[error("invalid contract for task {task_id}: {reason}")]
    InvalidContract { task_id: String, reason: String },
    #[error("task contract digest mismatch for task {0}")]
    ContractDigestMismatch(String),
    #[error("observed contract revision mismatch for task {0}")]
    StaleContractRevision(String),
    #[error("task {task_id} contract belongs to a different {field}")]
    ContractScopeMismatch {
        task_id: String,
        field: &'static str,
    },
    #[error("duplicate task ID: {0}")]
    DuplicateTaskId(String),
    #[error("task {task_id} has a missing dependency {dependency}")]
    MissingDependency { task_id: String, dependency: String },
    #[error("task dependency graph contains a cycle")]
    DependencyCycle,
    #[error("could not compute scheduler source digest: {0}")]
    Digest(String),
}

/// Decode a bounded snapshot. Semantic validation occurs in `propose_ready_tasks`.
/// This does not authenticate the source or turn reported completion into acceptance.
pub fn parse_scheduler_snapshot(bytes: &[u8]) -> Result<SchedulerSnapshot, SchedulerError> {
    if bytes.len() > MAX_SCHEDULER_BYTES {
        return Err(SchedulerError::SnapshotTooLarge);
    }
    let snapshot: SchedulerSnapshot =
        serde_json::from_slice(bytes).map_err(|error| SchedulerError::Json(error.to_string()))?;
    if snapshot.tasks.len() > MAX_SCHEDULED_TASKS {
        return Err(SchedulerError::TooManyTasks);
    }
    Ok(snapshot)
}

struct SnapshotByteLimit(usize);

impl Write for SnapshotByteLimit {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.0 {
            return Err(io::ErrorKind::FileTooLarge.into());
        }
        self.0 -= bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Validate a bounded snapshot and propose ready pending tasks in lexical ID
/// order. This function has no clock, filesystem, network, or execution effects.
pub fn propose_ready_tasks(
    snapshot: &SchedulerSnapshot,
    expected_scope: &SchedulingScope,
) -> Result<SchedulingProposal, SchedulerError> {
    if snapshot.schema_version != SCHEDULER_SCHEMA_VERSION {
        return Err(SchedulerError::UnsupportedVersion);
    }
    if snapshot.scope.revision == 0 || expected_scope.revision == 0 {
        return Err(SchedulerError::InvalidScopeRevision);
    }
    if snapshot.scope != *expected_scope {
        return Err(SchedulerError::ScopeMismatch);
    }
    if snapshot.tasks.len() > MAX_SCHEDULED_TASKS {
        return Err(SchedulerError::TooManyTasks);
    }
    // Typed callers bypass the byte parser. Bound nested contracts before their
    // validation, graph construction or digesting, without allocating a JSON copy.
    serde_json::to_writer(SnapshotByteLimit(MAX_SCHEDULER_BYTES), snapshot)
        .map_err(|_| SchedulerError::SnapshotTooLarge)?;

    let mut tasks = BTreeMap::new();
    for scheduled in &snapshot.tasks {
        let contract = &scheduled.contract;
        let id = contract.task_id.clone();
        contract
            .validate()
            .map_err(|error| SchedulerError::InvalidContract {
                task_id: id.to_string(),
                reason: error.0,
            })?;
        if contract.project_id != snapshot.scope.project_id {
            return Err(SchedulerError::ContractScopeMismatch {
                task_id: id.to_string(),
                field: "project",
            });
        }
        if contract.root_id != snapshot.scope.root_id {
            return Err(SchedulerError::ContractScopeMismatch {
                task_id: id.to_string(),
                field: "root",
            });
        }
        if scheduled.observed_contract_revision != contract.revision {
            return Err(SchedulerError::StaleContractRevision(id.to_string()));
        }
        let digest =
            content_digest(contract).map_err(|error| SchedulerError::Digest(error.to_string()))?;
        if scheduled.contract_digest != digest {
            return Err(SchedulerError::ContractDigestMismatch(id.to_string()));
        }
        if tasks.insert(id.clone(), scheduled).is_some() {
            return Err(SchedulerError::DuplicateTaskId(id.to_string()));
        }
    }

    // Validate all references and detect cycles iteratively, so graph depth
    // cannot consume the call stack. The 1000-task cap bounds this traversal.
    let mut indegree: BTreeMap<TaskId, usize> = tasks.keys().cloned().map(|id| (id, 0)).collect();
    let mut dependents: BTreeMap<TaskId, Vec<TaskId>> = BTreeMap::new();
    for (id, scheduled) in &tasks {
        for dependency in &scheduled.contract.dependencies {
            if !tasks.contains_key(dependency) {
                return Err(SchedulerError::MissingDependency {
                    task_id: id.to_string(),
                    dependency: dependency.to_string(),
                });
            }
            if let Some(count) = indegree.get_mut(id) {
                *count += 1;
            }
            dependents
                .entry(dependency.clone())
                .or_default()
                .push(id.clone());
        }
    }
    let mut ready_nodes: BTreeSet<TaskId> = indegree
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(id.clone()))
        .collect();
    let mut visited = 0;
    while let Some(id) = ready_nodes.pop_first() {
        visited += 1;
        if let Some(children) = dependents.get(&id) {
            for child in children {
                if let Some(count) = indegree.get_mut(child) {
                    *count -= 1;
                    if *count == 0 {
                        ready_nodes.insert(child.clone());
                    }
                }
            }
        }
    }
    if visited != tasks.len() {
        return Err(SchedulerError::DependencyCycle);
    }

    let mut eligible = Vec::new();
    let mut blocked = BTreeMap::new();
    for (id, scheduled) in &tasks {
        if scheduled.state != TaskSchedulingState::Pending {
            blocked.insert(id.clone(), vec![ReadinessBlocker::NotPending]);
            continue;
        }

        let mut reasons = Vec::new();
        for dependency in &scheduled.contract.dependencies {
            if let Some(dependency_task) = tasks.get(dependency)
                && dependency_task.state != TaskSchedulingState::Completed
            {
                reasons.push(ReadinessBlocker::DependencyNotCompleted {
                    dependency: dependency.clone(),
                    observed_state: dependency_task.state.clone(),
                });
            }
        }
        match &scheduled.lease {
            LeaseObservation::Absent => {}
            LeaseObservation::Held { expires_tick } => {
                // Expiry alone is not authority to reclaim; reconciliation is required.
                reasons.push(ReadinessBlocker::LeaseHeld {
                    expires_tick: *expires_tick,
                });
            }
            LeaseObservation::Unknown => reasons.push(ReadinessBlocker::LeaseUnknown),
        }
        if reasons.is_empty() {
            eligible.push(id.clone());
        } else {
            blocked.insert(id.clone(), reasons);
        }
    }

    let quota = match &snapshot.available_quota {
        Knowledge::Known(value) => Some(*value),
        Knowledge::Unknown => None,
    };
    let capacity = match &snapshot.available_capacity {
        Knowledge::Known(value) => Some(*value),
        Knowledge::Unknown => None,
    };

    let mut ready = Vec::new();
    for id in eligible {
        let mut reasons = Vec::new();
        match quota {
            None => reasons.push(ReadinessBlocker::QuotaUnknown),
            Some(0) => reasons.push(ReadinessBlocker::QuotaLimit),
            Some(_) => {}
        }
        match capacity {
            None => reasons.push(ReadinessBlocker::CapacityUnknown),
            Some(0) => reasons.push(ReadinessBlocker::CapacityLimit),
            Some(_) => {}
        }
        if !reasons.is_empty() {
            blocked.insert(id, reasons);
        } else {
            ready.push(id);
        }
    }

    if let (Some(quota), Some(capacity)) = (quota, capacity) {
        let admission_limit = usize::from(quota.min(capacity));
        let deferred = ready.split_off(admission_limit.min(ready.len()));
        for id in deferred {
            let mut reasons = Vec::new();
            if usize::from(quota) <= ready.len() {
                reasons.push(ReadinessBlocker::QuotaLimit);
            }
            if usize::from(capacity) <= ready.len() {
                reasons.push(ReadinessBlocker::CapacityLimit);
            }
            blocked.insert(id, reasons);
        }
    }

    let source_digest =
        content_digest(snapshot).map_err(|error| SchedulerError::Digest(error.to_string()))?;
    Ok(SchedulingProposal {
        scope: snapshot.scope.clone(),
        source_digest,
        ready,
        blocked,
        execution_granted: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AcceptanceCriterion, CheckSpec};

    fn scope() -> SchedulingScope {
        SchedulingScope {
            project_id: ProjectId::new("project").expect("project ID"),
            root_id: RootId::new("root").expect("root ID"),
            revision: 3,
        }
    }

    fn contract(id: &str, dependencies: &[&str]) -> TaskContract {
        TaskContract {
            schema_version: 1,
            project_id: ProjectId::new("project").expect("project ID"),
            root_id: RootId::new("root").expect("root ID"),
            task_id: TaskId::new(id).expect("task ID"),
            revision: 1,
            title: format!("Task {id}"),
            objective: "Complete the bounded task".into(),
            non_goals: vec![],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec![],
            read_first: vec![],
            dependencies: dependencies
                .iter()
                .map(|dependency| TaskId::new(*dependency).expect("dependency ID"))
                .collect(),
            criteria: vec![AcceptanceCriterion {
                id: "criterion".into(),
                text: "Named check passes".into(),
                check_ids: vec!["check".into()],
            }],
            checks: vec![CheckSpec {
                id: "check".into(),
                program: "cargo".into(),
                args: vec!["check".into()],
                cwd: ".".into(),
                timeout_secs: 30,
            }],
            authority_ref: "authority".into(),
            max_attempts: 1,
            simulated: true,
        }
    }

    fn scheduled(
        task: TaskContract,
        state: TaskSchedulingState,
        lease: LeaseObservation,
    ) -> ScheduledTask {
        let contract_digest = content_digest(&task).expect("contract digest");
        ScheduledTask {
            observed_contract_revision: task.revision,
            contract: task,
            contract_digest,
            state,
            lease,
        }
    }

    fn snapshot(tasks: Vec<ScheduledTask>) -> SchedulerSnapshot {
        SchedulerSnapshot {
            schema_version: SCHEDULER_SCHEMA_VERSION,
            scope: scope(),
            tasks,
            available_quota: Knowledge::Known(10),
            available_capacity: Knowledge::Known(10),
        }
    }

    fn propose(snapshot: &SchedulerSnapshot) -> Result<SchedulingProposal, SchedulerError> {
        propose_ready_tasks(snapshot, &scope())
    }

    fn ids(values: &[&str]) -> Vec<TaskId> {
        values
            .iter()
            .map(|value| TaskId::new(*value).expect("task ID"))
            .collect()
    }

    #[test]
    fn proposes_only_dependency_ready_tasks_in_stable_order_and_preserves_input() {
        let tasks = vec![
            scheduled(
                contract("z", &[]),
                TaskSchedulingState::Pending,
                LeaseObservation::Absent,
            ),
            scheduled(
                contract("b", &["a"]),
                TaskSchedulingState::Pending,
                LeaseObservation::Absent,
            ),
            scheduled(
                contract("a", &[]),
                TaskSchedulingState::Completed,
                LeaseObservation::Absent,
            ),
            scheduled(
                contract("c", &[]),
                TaskSchedulingState::Pending,
                LeaseObservation::Absent,
            ),
        ];
        let first_snapshot = snapshot(tasks.clone());
        let before = first_snapshot.clone();
        let first = propose(&first_snapshot).expect("valid snapshot");
        let mut reversed = tasks;
        reversed.reverse();
        let second = propose(&snapshot(reversed)).expect("permuted snapshot");

        assert_eq!(first.ready, ids(&["b", "c", "z"]));
        assert_eq!(first.ready, second.ready);
        assert_ne!(first.source_digest, second.source_digest);
        assert_eq!(first_snapshot, before);
        assert!(!first.execution_granted);
        assert_eq!(propose(&first_snapshot).expect("replay"), first);
    }

    #[test]
    fn rejects_contract_digest_scope_and_revision_mismatches() {
        let original = scheduled(
            contract("a", &[]),
            TaskSchedulingState::Pending,
            LeaseObservation::Absent,
        );
        let mut wrong_digest = snapshot(vec![original.clone()]);
        wrong_digest.tasks[0].contract_digest = "sha256:bad".into();
        assert!(matches!(
            propose(&wrong_digest),
            Err(SchedulerError::ContractDigestMismatch(_))
        ));

        let mut wrong_project_task = original.clone();
        wrong_project_task.contract.project_id = ProjectId::new("other").expect("project ID");
        wrong_project_task.contract_digest =
            content_digest(&wrong_project_task.contract).expect("digest");
        assert!(matches!(
            propose(&snapshot(vec![wrong_project_task])),
            Err(SchedulerError::ContractScopeMismatch {
                field: "project",
                ..
            })
        ));

        let mut wrong_root_task = original.clone();
        wrong_root_task.contract.root_id = RootId::new("other").expect("root ID");
        wrong_root_task.contract_digest =
            content_digest(&wrong_root_task.contract).expect("digest");
        assert!(matches!(
            propose(&snapshot(vec![wrong_root_task])),
            Err(SchedulerError::ContractScopeMismatch { field: "root", .. })
        ));

        let mut stale = original.clone();
        stale.observed_contract_revision += 1;
        assert!(matches!(
            propose(&snapshot(vec![stale])),
            Err(SchedulerError::StaleContractRevision(_))
        ));

        let mut invalid = original;
        invalid.contract.title.clear();
        invalid.contract_digest = content_digest(&invalid.contract).expect("digest");
        assert!(matches!(
            propose(&snapshot(vec![invalid])),
            Err(SchedulerError::InvalidContract { .. })
        ));
    }

    #[test]
    fn rejects_bad_scopes_duplicates_missing_self_duplicate_and_cyclic_dependencies() {
        let task = || {
            scheduled(
                contract("a", &[]),
                TaskSchedulingState::Pending,
                LeaseObservation::Absent,
            )
        };
        let mut changed_expected = scope();
        changed_expected.revision += 1;
        assert_eq!(
            propose_ready_tasks(&snapshot(vec![task()]), &changed_expected),
            Err(SchedulerError::ScopeMismatch)
        );
        let mut invalid_scope = snapshot(vec![task()]);
        invalid_scope.scope.revision = 0;
        assert_eq!(
            propose(&invalid_scope),
            Err(SchedulerError::InvalidScopeRevision)
        );
        assert!(matches!(
            propose(&snapshot(vec![task(), task()])),
            Err(SchedulerError::DuplicateTaskId(_))
        ));
        assert!(matches!(
            propose(&snapshot(vec![scheduled(
                contract("a", &["missing"]),
                TaskSchedulingState::Pending,
                LeaseObservation::Absent
            )])),
            Err(SchedulerError::MissingDependency { .. })
        ));
        assert!(matches!(
            propose(&snapshot(vec![scheduled(
                contract("a", &["a"]),
                TaskSchedulingState::Pending,
                LeaseObservation::Absent
            )])),
            Err(SchedulerError::InvalidContract { .. })
        ));
        assert!(matches!(
            propose(&snapshot(vec![
                scheduled(
                    contract("a", &["b"]),
                    TaskSchedulingState::Pending,
                    LeaseObservation::Absent
                ),
                scheduled(
                    contract("b", &["a"]),
                    TaskSchedulingState::Pending,
                    LeaseObservation::Absent
                ),
            ])),
            Err(SchedulerError::DependencyCycle)
        ));
    }

    #[test]
    fn rejects_oversized_snapshots_and_unknown_serialized_fields() {
        let too_many = snapshot(
            (0..=MAX_SCHEDULED_TASKS)
                .map(|index| {
                    scheduled(
                        contract(&format!("task-{index}"), &[]),
                        TaskSchedulingState::Pending,
                        LeaseObservation::Absent,
                    )
                })
                .collect(),
        );
        assert_eq!(propose(&too_many), Err(SchedulerError::TooManyTasks));

        let mut value = serde_json::to_value(snapshot(vec![])).expect("serialize snapshot");
        value["unexpected"] = serde_json::Value::Bool(true);
        assert!(serde_json::from_value::<SchedulerSnapshot>(value).is_err());
    }

    #[test]
    fn traverses_a_deep_bounded_dependency_chain_iteratively() {
        let chain: Vec<_> = (0..MAX_SCHEDULED_TASKS)
            .map(|index| {
                let id = format!("t{index:04}");
                let dependencies = if index == 0 {
                    vec![]
                } else {
                    vec![format!("t{:04}", index - 1)]
                };
                let dependency_refs: Vec<_> = dependencies.iter().map(String::as_str).collect();
                scheduled(
                    contract(&id, &dependency_refs),
                    if index + 1 == MAX_SCHEDULED_TASKS {
                        TaskSchedulingState::Pending
                    } else {
                        TaskSchedulingState::Completed
                    },
                    LeaseObservation::Absent,
                )
            })
            .collect();
        let mut input = snapshot(chain);
        input.available_quota = Knowledge::Known(1);
        input.available_capacity = Knowledge::Known(1);
        assert_eq!(
            propose(&input).expect("bounded chain").ready,
            ids(&["t0999"])
        );
    }

    #[test]
    fn unknown_and_zero_limits_are_explicit_blockers() {
        let mut input = snapshot(vec![scheduled(
            contract("a", &[]),
            TaskSchedulingState::Pending,
            LeaseObservation::Absent,
        )]);
        input.available_quota = Knowledge::Unknown;
        input.available_capacity = Knowledge::Unknown;
        let proposal = propose(&input).expect("unknown limits remain a proposal");
        assert!(proposal.ready.is_empty());
        assert_eq!(
            proposal.blocked[&TaskId::new("a").expect("task ID")],
            [
                ReadinessBlocker::QuotaUnknown,
                ReadinessBlocker::CapacityUnknown
            ]
        );

        input.available_quota = Knowledge::Known(0);
        input.available_capacity = Knowledge::Known(0);
        let proposal = propose(&input).expect("zero limits remain a proposal");
        assert_eq!(
            proposal.blocked[&TaskId::new("a").expect("task ID")],
            [
                ReadinessBlocker::QuotaLimit,
                ReadinessBlocker::CapacityLimit
            ]
        );
    }

    #[test]
    fn unknown_held_and_expired_leases_never_start() {
        for lease in [
            LeaseObservation::Unknown,
            LeaseObservation::Held { expires_tick: None },
            LeaseObservation::Held {
                expires_tick: Some(0),
            },
        ] {
            let proposal = propose(&snapshot(vec![scheduled(
                contract("a", &[]),
                TaskSchedulingState::Pending,
                lease,
            )]))
            .expect("lease uncertainty is represented");
            assert!(proposal.ready.is_empty());
            assert!(
                proposal
                    .blocked
                    .contains_key(&TaskId::new("a").expect("task ID"))
            );
        }
    }

    #[test]
    fn active_or_candidate_like_task_never_satisfies_a_dependency() {
        let input = snapshot(vec![
            scheduled(
                contract("dep", &[]),
                TaskSchedulingState::Active,
                LeaseObservation::Absent,
            ),
            scheduled(
                contract("child", &["dep"]),
                TaskSchedulingState::Pending,
                LeaseObservation::Absent,
            ),
        ]);
        let proposal = propose(&input).expect("active dependency is an explicit blocker");
        assert!(proposal.ready.is_empty());
        assert_eq!(
            proposal.blocked[&TaskId::new("child").expect("task ID")],
            [ReadinessBlocker::DependencyNotCompleted {
                dependency: TaskId::new("dep").expect("task ID"),
                observed_state: TaskSchedulingState::Active,
            }]
        );
        assert!(
            proposal
                .blocked
                .contains_key(&TaskId::new("dep").expect("task ID"))
        );
    }

    #[test]
    fn admission_uses_minimum_quota_capacity_and_reports_deferred_work() {
        let make_tasks = || {
            ["a", "b", "c"]
                .into_iter()
                .map(|id| {
                    scheduled(
                        contract(id, &[]),
                        TaskSchedulingState::Pending,
                        LeaseObservation::Absent,
                    )
                })
                .collect()
        };
        let mut input = snapshot(make_tasks());
        input.available_quota = Knowledge::Known(2);
        input.available_capacity = Knowledge::Known(1);
        let proposal = propose(&input).expect("bounded proposal");
        assert_eq!(proposal.ready, ids(&["a"]));
        assert_eq!(
            proposal.blocked[&TaskId::new("b").expect("task ID")],
            [ReadinessBlocker::CapacityLimit]
        );
        assert!(!proposal.execution_granted);

        input.available_quota = Knowledge::Known(1);
        input.available_capacity = Knowledge::Known(2);
        let proposal = propose(&input).expect("quota bounded proposal");
        assert_eq!(proposal.ready, ids(&["a"]));
        assert!(
            proposal
                .blocked
                .values()
                .any(|reasons| reasons == &[ReadinessBlocker::QuotaLimit])
        );
    }

    #[test]
    fn byte_parser_and_typed_input_bound_nested_contracts_without_clock_or_effects() {
        let input = snapshot(vec![]);
        let mut bytes = serde_json::to_vec(&input).expect("snapshot JSON");
        bytes.resize(MAX_SCHEDULER_BYTES, b' ');
        assert_eq!(
            parse_scheduler_snapshot(&bytes).expect("exact byte bound"),
            input
        );
        bytes.push(b' ');
        assert_eq!(
            parse_scheduler_snapshot(&bytes),
            Err(SchedulerError::SnapshotTooLarge)
        );

        let mut oversized = snapshot(vec![scheduled(
            contract("a", &[]),
            TaskSchedulingState::Pending,
            LeaseObservation::Absent,
        )]);
        oversized.tasks[0].contract.objective = "x".repeat(MAX_SCHEDULER_BYTES);
        assert_eq!(propose(&oversized), Err(SchedulerError::SnapshotTooLarge));

        // Held is an observation, never an expiry/reclaim permit. Neither an
        // old tick, zero nor a far-future tick triggers a wall-clock decision.
        for expires_tick in [None, Some(0), Some(1), Some(u64::MAX)] {
            let held = snapshot(vec![scheduled(
                contract("held", &[]),
                TaskSchedulingState::Pending,
                LeaseObservation::Held { expires_tick },
            )]);
            let before = held.clone();
            let first = propose(&held).expect("held lease observation");
            assert_eq!(propose(&held).expect("repeat pure proposal"), first);
            assert_eq!(held, before);
            assert!(first.ready.is_empty());
            assert!(!first.execution_granted);
            assert_eq!(
                first.blocked[&TaskId::new("held").expect("task ID")],
                [ReadinessBlocker::LeaseHeld { expires_tick }]
            );
        }
    }

    #[test]
    fn parser_rejects_duplicate_fields_nested_unknowns_and_execution_injection() {
        let input = snapshot(vec![scheduled(
            contract("a", &[]),
            TaskSchedulingState::Pending,
            LeaseObservation::Held {
                expires_tick: Some(7),
            },
        )]);
        let original = serde_json::to_value(&input).expect("snapshot value");
        let mut extra = original.clone();
        extra["tasks"][0]["lease"]["held"]["reclaim"] = true.into();
        assert!(matches!(
            parse_scheduler_snapshot(&serde_json::to_vec(&extra).expect("JSON")),
            Err(SchedulerError::Json(_))
        ));
        let mut extra = original;
        extra["execution_granted"] = true.into();
        assert!(matches!(
            parse_scheduler_snapshot(&serde_json::to_vec(&extra).expect("JSON")),
            Err(SchedulerError::Json(_))
        ));
        let duplicate =
            serde_json::to_string(&input)
                .expect("JSON")
                .replacen("{", "{\"schema_version\":1,", 1);
        assert!(matches!(
            parse_scheduler_snapshot(duplicate.as_bytes()),
            Err(SchedulerError::Json(_))
        ));
    }

    #[test]
    fn all_slot_combinations_respect_independent_capacity_and_quota_boundaries() {
        for quota in 0..=4_u16 {
            for capacity in 0..=4_u16 {
                let mut input = snapshot(
                    ["a", "b", "c"]
                        .into_iter()
                        .map(|id| {
                            scheduled(
                                contract(id, &[]),
                                TaskSchedulingState::Pending,
                                LeaseObservation::Absent,
                            )
                        })
                        .collect(),
                );
                input.available_quota = Knowledge::Known(quota);
                input.available_capacity = Knowledge::Known(capacity);
                let proposal = propose(&input).expect("valid observed task-slot counts");
                let limit = usize::from(quota.min(capacity)).min(3);
                assert_eq!(proposal.ready, ids(&["a", "b", "c"][..limit]));
                for reasons in proposal.blocked.values() {
                    assert!(!reasons.is_empty());
                    assert_eq!(
                        reasons.contains(&ReadinessBlocker::QuotaLimit),
                        quota <= capacity && usize::from(quota) < 3
                    );
                    assert_eq!(
                        reasons.contains(&ReadinessBlocker::CapacityLimit),
                        capacity <= quota && usize::from(capacity) < 3
                    );
                }
                assert!(!proposal.execution_granted);
            }
        }
    }
}
