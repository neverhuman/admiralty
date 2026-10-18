//! Main-branch scheduler readiness qualification.
//!
//! Main currently exposes pure proposals, not transactional admission. These
//! checks cover the safety properties that the public proposal API can prove.

use admiralty_core::scheduler::{
    Knowledge, LeaseObservation, ReadinessBlocker, ScheduledTask, SchedulerError,
    SchedulerSnapshot, SchedulingScope, TaskSchedulingState, propose_ready_tasks,
};
use admiralty_core::{
    AcceptanceCriterion, CheckSpec, ProjectId, RootId, TaskContract, TaskId, content_digest,
};

fn scope() -> SchedulingScope {
    SchedulingScope {
        project_id: ProjectId::new("project").expect("valid project ID"),
        root_id: RootId::new("root").expect("valid root ID"),
        revision: 1,
    }
}

fn contract(id: &str) -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: scope().project_id,
        root_id: scope().root_id,
        task_id: TaskId::new(id).expect("valid task ID"),
        revision: 1,
        title: format!("Task {id}"),
        objective: "Complete a bounded task".into(),
        non_goals: vec![],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "verified".into(),
            text: "The named check passes".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "cargo".into(),
            args: vec!["check".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "test-authority".into(),
        max_attempts: 1,
        simulated: true,
    }
}

fn scheduled(id: &str, lease: LeaseObservation) -> ScheduledTask {
    let contract = contract(id);
    let contract_digest = content_digest(&contract).expect("contract digest");
    ScheduledTask {
        observed_contract_revision: contract.revision,
        contract,
        contract_digest,
        state: TaskSchedulingState::Pending,
        lease,
    }
}

fn snapshot(task: ScheduledTask) -> SchedulerSnapshot {
    SchedulerSnapshot {
        schema_version: 1,
        scope: scope(),
        tasks: vec![task],
        available_quota: Knowledge::Known(1),
        available_capacity: Knowledge::Known(1),
    }
}

#[test]
fn unknown_or_held_process_ownership_blocks_readiness_without_granting_execution() {
    for lease in [
        LeaseObservation::Unknown,
        LeaseObservation::Held { expires_tick: None },
    ] {
        let proposal = propose_ready_tasks(&snapshot(scheduled("task", lease)), &scope())
            .expect("valid snapshot yields a conservative proposal");
        assert!(proposal.ready.is_empty());
        assert!(!proposal.execution_granted);
        assert!(
            proposal
                .blocked
                .contains_key(&TaskId::new("task").expect("task ID"))
        );
    }
}

#[test]
fn unknown_capacity_and_quota_do_not_create_admission() {
    let mut input = snapshot(scheduled("task", LeaseObservation::Absent));
    input.available_quota = Knowledge::Unknown;
    input.available_capacity = Knowledge::Unknown;

    let proposal = propose_ready_tasks(&input, &scope()).expect("valid snapshot");
    assert!(proposal.ready.is_empty());
    assert!(!proposal.execution_granted);
    let blockers = proposal
        .blocked
        .get(&TaskId::new("task").expect("task ID"))
        .expect("unknown resource state blocks task");
    assert!(blockers.contains(&ReadinessBlocker::QuotaUnknown));
    assert!(blockers.contains(&ReadinessBlocker::CapacityUnknown));
}

#[test]
fn changed_scope_is_rejected_instead_of_escalating_or_claiming() {
    let mut expected = scope();
    expected.revision += 1;
    let result = propose_ready_tasks(
        &snapshot(scheduled("task", LeaseObservation::Absent)),
        &expected,
    );

    assert_eq!(result, Err(SchedulerError::ScopeMismatch));
}

#[test]
fn tampered_contract_digest_is_rejected_before_proposal() {
    let mut input = snapshot(scheduled("task", LeaseObservation::Absent));
    input.tasks[0].contract_digest = "sha256:forged".into();

    assert_eq!(
        propose_ready_tasks(&input, &scope()),
        Err(SchedulerError::ContractDigestMismatch("task".into()))
    );
}
