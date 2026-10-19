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
use admiralty_testkit::clock::FakeClock;

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
fn clock_past_lease_expiry_does_not_authorize_reclaim_or_execution() {
    let mut clock = FakeClock::new(10);
    let lease = LeaseObservation::Held {
        expires_tick: Some(20),
    };
    let input = snapshot(scheduled("task", lease));

    assert_eq!(clock.advance_to_ms(21), Ok(21));
    let proposal = propose_ready_tasks(&input, &scope()).expect("valid expired-lease snapshot");

    assert!(proposal.ready.is_empty());
    assert!(!proposal.execution_granted);
    assert_eq!(
        proposal.blocked.get(&TaskId::new("task").expect("task ID")),
        Some(&vec![ReadinessBlocker::LeaseHeld {
            expires_tick: Some(20),
        }])
    );
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
fn zero_capacity_and_quota_block_readiness_at_boundary() {
    let mut input = snapshot(scheduled("task", LeaseObservation::Absent));
    input.available_quota = Knowledge::Known(0);
    input.available_capacity = Knowledge::Known(0);

    let proposal = propose_ready_tasks(&input, &scope()).expect("valid zero-resource snapshot");
    assert!(proposal.ready.is_empty());
    assert!(!proposal.execution_granted);
    let blockers = proposal
        .blocked
        .get(&TaskId::new("task").expect("task ID"))
        .expect("zero resources block task");
    assert!(blockers.contains(&ReadinessBlocker::QuotaLimit));
    assert!(blockers.contains(&ReadinessBlocker::CapacityLimit));
}

#[test]
fn replaying_the_same_snapshot_produces_the_same_non_authorizing_proposal() {
    let input = snapshot(scheduled("task", LeaseObservation::Absent));

    let first = propose_ready_tasks(&input, &scope()).expect("valid snapshot");
    let replay = propose_ready_tasks(&input, &scope()).expect("identical replay");

    assert_eq!(first, replay);
    assert!(!replay.execution_granted);
    assert!(input.tasks.iter().all(|task| {
        replay.ready.contains(&task.contract.task_id)
            ^ replay.blocked.contains_key(&task.contract.task_id)
    }));
}

#[test]
fn unsupported_snapshot_schema_is_rejected_before_proposal() {
    let mut input = snapshot(scheduled("task", LeaseObservation::Absent));
    input.schema_version = u32::MAX;

    assert_eq!(
        propose_ready_tasks(&input, &scope()),
        Err(SchedulerError::UnsupportedVersion)
    );
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

#[test]
fn stale_observed_contract_revision_is_rejected_before_proposal() {
    let mut input = snapshot(scheduled("task", LeaseObservation::Absent));
    input.tasks[0].observed_contract_revision += 1;

    assert_eq!(
        propose_ready_tasks(&input, &scope()),
        Err(SchedulerError::StaleContractRevision("task".into()))
    );
}

use admiralty_core::scheduler::fairness::{
    FairnessPolicy, FairnessState, ProjectPolicy, ProjectQueue, QueuedTask, WorkClass,
    propose_fair_round,
};

fn fair_queue(id: &str, count: usize, weight: u16, class: WorkClass) -> ProjectQueue {
    let project = ProjectId::new(id).expect("project");
    let mut tasks = Vec::new();
    let mut metadata = std::collections::BTreeMap::new();
    for n in 0..count {
        let mut task = scheduled(&format!("{id}-{n:03}"), LeaseObservation::Absent);
        task.contract.project_id = project.clone();
        task.contract_digest = content_digest(&task.contract).expect("digest");
        metadata.insert(
            task.contract.task_id.clone(),
            QueuedTask {
                queued_at: 0,
                class,
            },
        );
        tasks.push(task);
    }
    let mut project_scope = scope();
    project_scope.project_id = project;
    ProjectQueue {
        snapshot: SchedulerSnapshot {
            schema_version: 1,
            scope: project_scope,
            tasks,
            available_quota: Knowledge::Known(count as u16),
            available_capacity: Knowledge::Known(count as u16),
        },
        tasks: metadata,
        policy: ProjectPolicy {
            weight,
            feature_wip_limit: 1000,
            ..ProjectPolicy::default()
        },
        active_feature_wip: 0,
        throughput_known: true,
        red_train_age_secs: Knowledge::Known(0),
        since_last_accept_secs: Knowledge::Known(0),
    }
}

fn no_reserves() -> FairnessPolicy {
    FairnessPolicy {
        verification_percent: 0,
        repair_percent: 0,
        ..FairnessPolicy::default()
    }
}

#[test]
fn weighted_projects_receive_three_to_one_service_without_starvation() {
    let queues = [
        fair_queue("a", 40, 3, WorkClass::Feature),
        fair_queue("b", 40, 1, WorkClass::Feature),
    ];
    let state = FairnessState::default();
    let result = propose_fair_round(
        &queues,
        &state,
        &no_reserves(),
        Knowledge::Known(40),
        Knowledge::Known(40),
        0,
    )
    .expect("round");
    assert_eq!(result.selected.len(), 40);
    assert_eq!(
        result
            .selected
            .iter()
            .filter(|s| s.project_id.as_str() == "a")
            .count(),
        30
    );
    assert_eq!(
        result
            .selected
            .iter()
            .filter(|s| s.project_id.as_str() == "b")
            .count(),
        10
    );
    assert!(!result.execution_granted);
    assert_eq!(
        result,
        propose_fair_round(
            &queues,
            &state,
            &no_reserves(),
            Knowledge::Known(40),
            Knowledge::Known(40),
            0
        )
        .expect("replay")
    );
}

#[test]
fn aged_low_weight_project_precedes_new_high_weight_work() {
    let mut new = fair_queue("new", 1, 1000, WorkClass::Feature);
    new.tasks.values_mut().for_each(|t| t.queued_at = 590);
    let old = fair_queue("old", 1, 1, WorkClass::Feature);
    let result = propose_fair_round(
        &[new, old],
        &FairnessState::default(),
        &no_reserves(),
        Knowledge::Known(1),
        Knowledge::Known(1),
        600,
    )
    .expect("aging");
    assert_eq!(result.selected[0].project_id.as_str(), "old");
}

#[test]
fn critical_path_is_ranked_before_allocating_a_projects_last_slot() {
    let mut queue = fair_queue("p", 3, 1, WorkClass::Feature);
    let root = queue.snapshot.tasks[1].contract.task_id.clone();
    queue.snapshot.tasks[2]
        .contract
        .dependencies
        .push(root.clone());
    queue.snapshot.tasks[2].contract_digest =
        content_digest(&queue.snapshot.tasks[2].contract).expect("digest");
    queue.snapshot.available_capacity = Knowledge::Known(1);
    let result = propose_fair_round(
        &[queue],
        &FairnessState::default(),
        &no_reserves(),
        Knowledge::Known(1),
        Knowledge::Known(1),
        0,
    )
    .expect("critical path");
    assert_eq!(result.selected[0].task_id, root);
}

#[test]
fn verification_and_repair_pools_leave_capacity_for_finishing() {
    let feature = fair_queue("feature", 10, 1, WorkClass::Feature);
    let repair = fair_queue("repair", 1, 1, WorkClass::Repair);
    let result = propose_fair_round(
        &[feature, repair],
        &FairnessState::default(),
        &FairnessPolicy::default(),
        Knowledge::Known(10),
        Knowledge::Known(10),
        0,
    )
    .expect("reserved pools");
    assert_eq!(
        result
            .selected
            .iter()
            .filter(|s| s.class == WorkClass::Feature)
            .count(),
        7
    );
    assert_eq!(
        result
            .selected
            .iter()
            .filter(|s| s.class == WorkClass::Repair)
            .count(),
        1
    );
    assert_eq!(result.selected.len(), 8);
}

#[test]
fn default_reserves_do_not_deadlock_a_single_slot_pool() {
    let result = propose_fair_round(
        &[fair_queue("small", 1, 1, WorkClass::Feature)],
        &FairnessState::default(),
        &FairnessPolicy::default(),
        Knowledge::Known(1),
        Knowledge::Known(1),
        0,
    )
    .expect("single slot");
    assert_eq!(result.selected.len(), 1);
}

#[test]
fn zero_acceptance_finish_mode_respects_the_minimum_wip_boundary() {
    let mut queue = fair_queue("p", 1, 1, WorkClass::Feature);
    queue.since_last_accept_secs = Knowledge::Known(3600);
    queue.active_feature_wip = 1;
    let below = propose_fair_round(
        &[queue.clone()],
        &FairnessState::default(),
        &no_reserves(),
        Knowledge::Known(1),
        Knowledge::Known(1),
        0,
    )
    .expect("below minimum");
    assert_eq!(below.selected.len(), 1);
    queue.active_feature_wip = 2;
    let at = propose_fair_round(
        &[queue],
        &FairnessState::default(),
        &no_reserves(),
        Knowledge::Known(1),
        Knowledge::Known(1),
        0,
    )
    .expect("at minimum");
    assert!(at.selected.is_empty());
    assert_eq!(at.next_state.finish_mode.len(), 1);
}

#[test]
fn finish_mode_is_project_scoped_and_has_hysteresis() {
    let mut red = fair_queue("red", 1, 1, WorkClass::Feature);
    red.red_train_age_secs = Knowledge::Known(1200);
    let green = fair_queue("green", 1, 1, WorkClass::Feature);
    let first = propose_fair_round(
        &[red.clone(), green.clone()],
        &FairnessState::default(),
        &no_reserves(),
        Knowledge::Known(2),
        Knowledge::Known(2),
        0,
    )
    .expect("enter finish mode");
    assert_eq!(first.selected.len(), 1);
    assert_eq!(first.selected[0].project_id.as_str(), "green");
    red.red_train_age_secs = Knowledge::Known(600);
    let second = propose_fair_round(
        &[red.clone(), green.clone()],
        &first.next_state,
        &no_reserves(),
        Knowledge::Known(2),
        Knowledge::Known(2),
        0,
    )
    .expect("retain mode");
    assert!(
        second
            .next_state
            .finish_mode
            .contains(&red.snapshot.scope.project_id)
    );
    red.red_train_age_secs = Knowledge::Known(300);
    let third = propose_fair_round(
        &[red, green],
        &second.next_state,
        &no_reserves(),
        Knowledge::Known(2),
        Knowledge::Known(2),
        0,
    )
    .expect("exit mode");
    assert!(third.next_state.finish_mode.is_empty());
    assert_eq!(third.selected.len(), 2);
    assert_eq!(third.expected_generation, 2);
    assert_ne!(first.source_digest, second.source_digest);
}

#[test]
fn bootstrap_unknown_permits_and_process_ownership_remain_conservative() {
    let mut queue = fair_queue("p", 4, 1, WorkClass::Feature);
    queue.throughput_known = false;
    let result = propose_fair_round(
        &[queue.clone()],
        &FairnessState::default(),
        &no_reserves(),
        Knowledge::Known(4),
        Knowledge::Known(4),
        0,
    )
    .expect("bootstrap");
    assert_eq!(result.selected.len(), 1);
    let unknown = propose_fair_round(
        &[queue.clone()],
        &FairnessState::default(),
        &no_reserves(),
        Knowledge::Unknown,
        Knowledge::Known(4),
        0,
    )
    .expect("unknown inference");
    assert!(unknown.selected.is_empty());
    assert_ne!(result.source_digest, unknown.source_digest);
    queue
        .snapshot
        .tasks
        .iter_mut()
        .for_each(|t| t.lease = LeaseObservation::Unknown);
    let held = propose_fair_round(
        &[queue],
        &FairnessState::default(),
        &no_reserves(),
        Knowledge::Known(4),
        Knowledge::Known(4),
        9999,
    )
    .expect("unknown processes");
    assert!(held.selected.is_empty());
}

#[test]
fn stale_forged_scope_and_invalid_fairness_metadata_are_rejected() {
    let mut queue = fair_queue("p", 1, 1, WorkClass::Feature);
    queue.snapshot.tasks[0].contract_digest = "sha256:forged".into();
    assert!(
        propose_fair_round(
            &[queue.clone()],
            &FairnessState::default(),
            &no_reserves(),
            Knowledge::Known(1),
            Knowledge::Known(1),
            0
        )
        .is_err()
    );
    queue.snapshot.tasks[0].contract_digest =
        content_digest(&queue.snapshot.tasks[0].contract).expect("digest");
    queue.tasks.values_mut().for_each(|t| t.queued_at = 1);
    assert!(
        propose_fair_round(
            &[queue.clone()],
            &FairnessState::default(),
            &no_reserves(),
            Knowledge::Known(1),
            Knowledge::Known(1),
            0
        )
        .is_err()
    );
    queue.tasks.values_mut().for_each(|t| t.queued_at = 0);
    queue.policy.weight = 0;
    assert!(
        propose_fair_round(
            &[queue],
            &FairnessState::default(),
            &no_reserves(),
            Knowledge::Known(1),
            Knowledge::Known(1),
            0
        )
        .is_err()
    );
}

#[test]
fn store_composition_rejects_forged_completion_and_detects_racing_claims() {
    use admiralty_core::{AttemptId, DomainEvent, PrincipalId, SourceIdentity};
    use admiralty_server::services::coordinator::{propose_store_round, read_set_is_current};
    use admiralty_store::Store;
    let mut builder = tempfile::Builder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let fixture = builder.tempdir().expect("private fixture");
    let store = Store::open(fixture.path().join("scheduler.sqlite")).expect("Store");
    let mut queue = fair_queue("project", 1, 1, WorkClass::Feature);
    let contract = queue.snapshot.tasks[0].contract.clone();
    let source = |sequence: &str| SourceIdentity {
        producer_id: "scheduler-fixture".into(),
        incarnation: "one".into(),
        sequence: sequence.into(),
    };
    store
        .append(
            source("1"),
            DomainEvent::TaskSubmitted {
                contract: contract.clone(),
            },
            "2026-10-19T00:00:00Z".into(),
        )
        .expect("submit");
    let result = propose_store_round(
        &store,
        &[queue.clone()],
        &FairnessState::default(),
        &no_reserves(),
        Knowledge::Known(1),
        Knowledge::Known(1),
        0,
    )
    .expect("current projection");
    assert_eq!(result.round.selected[0].task_id, contract.task_id);
    assert!(read_set_is_current(&store, &result).expect("unchanged read set"));
    let mut forged = queue.clone();
    forged.snapshot.tasks[0].state = TaskSchedulingState::Completed;
    assert!(
        propose_store_round(
            &store,
            &[forged],
            &FairnessState::default(),
            &no_reserves(),
            Knowledge::Known(1),
            Knowledge::Known(1),
            0
        )
        .is_err()
    );
    store
        .append(
            source("2"),
            DomainEvent::TaskClaimed {
                task_id: contract.task_id,
                revision: 1,
                attempt_id: AttemptId::new("attempt").expect("attempt"),
                owner: PrincipalId::new("worker").expect("owner"),
            },
            "2026-10-19T00:00:01Z".into(),
        )
        .expect("racing actual claim");
    assert!(!read_set_is_current(&store, &result).expect("changed read set"));
    assert!(
        propose_store_round(
            &store,
            &[queue.clone()],
            &FairnessState::default(),
            &no_reserves(),
            Knowledge::Known(1),
            Knowledge::Known(1),
            0
        )
        .is_err()
    );
    queue.snapshot.tasks[0].state = TaskSchedulingState::Active;
    queue.snapshot.tasks[0].lease = LeaseObservation::Unknown;
    queue.active_feature_wip = 1;
    let held = propose_store_round(
        &store,
        &[queue],
        &result.round.next_state,
        &no_reserves(),
        Knowledge::Known(1),
        Knowledge::Known(1),
        0,
    )
    .expect("actual unknown process");
    assert!(held.round.selected.is_empty());
    assert!(!held.round.execution_granted);
}
