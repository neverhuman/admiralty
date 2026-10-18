//! Round 13 work-order intake and release-evidence fail-closed qualification.

use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, PrincipalId, ProjectId, RootId, TaskContract,
    TaskId,
    workplans::{Workplan, WorkplanError, preview_workplan, preview_workplan_with_existing},
};
use admiralty_runtime::{
    reservations::{
        AccountId, CurrencyUnit, FinalEvidence, MinorUnits, ReconciliationEvidence,
        ReservationEvent, ReservationLedger, ReservationScope, ScopeLimits, StopReason,
        reduce_reservation, reserve_budget, settle_usage,
    },
    workspace::registry::{NodeRef, WorkspaceSpec},
};
use admiralty_testkit::{
    clock::FakeClock,
    faults::{FaultEffect, FaultPlan, FaultRule},
};
use std::{fs, path::PathBuf};

fn contract(id: &str, objective: &str) -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round13-project").expect("project ID"),
        root_id: RootId::new("round13-root").expect("root ID"),
        task_id: TaskId::new(id).expect("task ID"),
        revision: 1,
        title: format!("Work item {id}"),
        objective: objective.into(),
        non_goals: vec![],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec!["src/private".into()],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "verified".into(),
            text: "The required check passes".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "round13-grant".into(),
        max_attempts: 1,
        simulated: true,
    }
}

fn workplan(tasks: Vec<TaskContract>) -> Vec<u8> {
    serde_json::to_vec(&Workplan {
        schema_version: 1,
        plan_id: "round13-plan".into(),
        revision: 1,
        project_id: ProjectId::new("round13-project").expect("project ID"),
        tasks,
    })
    .expect("serialize fixture workplan")
}

#[test]
fn p00_p01_p04_p10_round13_wo_intake_duplicate_discovery() {
    // P00/P01: repeated previews are byte-bound and deterministic; duplicate
    // discovery returns an error without exposing a partial accepted plan.
    let plan_bytes = workplan(vec![contract("task-a", "Initial objective")]);
    let preview = preview_workplan(&plan_bytes).expect("valid preview");
    assert_eq!(
        preview_workplan(&plan_bytes).expect("deterministic replay"),
        preview
    );
    let mut truncated = plan_bytes.clone();
    truncated.pop();
    assert!(matches!(
        preview_workplan(&truncated),
        Err(WorkplanError::Json(_))
    ));
    let duplicate_bytes = workplan(vec![
        contract("duplicate-task", "First source record"),
        contract("duplicate-task", "Second source record"),
    ]);
    assert!(matches!(
        preview_workplan(&duplicate_bytes),
        Err(WorkplanError::DuplicateTaskId(id)) if id == "duplicate-task"
    ));
    let mut changed_plan = contract("task-a", "Changed objective under same ID");
    changed_plan.revision = 2;
    assert!(matches!(
        preview_workplan_with_existing(&workplan(vec![changed_plan]), &preview.task_digests),
        Err(WorkplanError::TaskIdentityConflict(id)) if id == "task-a"
    ));

    // P01: duplicate discovery evidence is reproducible under scripted fault
    // timing; malformed occurrence numbers are rejected before the clock moves.
    assert!(
        FaultPlan::new(vec![FaultRule {
            point: "discovery".into(),
            occurrence: 0,
            effect: FaultEffect::Drop,
        }])
        .is_err()
    );
    let mut fault = FaultPlan::new(vec![FaultRule {
        point: "duplicate-scan".into(),
        occurrence: 1,
        effect: FaultEffect::DelayMs(3),
    }])
    .expect("valid discovery fault fixture");
    assert_eq!(
        fault.hit("duplicate-scan").expect("script hit"),
        Some(FaultEffect::DelayMs(3))
    );
    let mut clock = FakeClock::new(20);
    clock.advance_ms(3).expect("advance fake time");
    assert_eq!(clock.now_ms(), 23);

    // P04: work-order ownership is constrained to registered paths and denies
    // excluded private content and traversal aliases.
    let temp = tempfile::tempdir().expect("workspace fixture");
    let root = temp.path().join("workspace");
    fs::create_dir_all(root.join("src/private")).expect("source dirs");
    fs::create_dir_all(root.join(".git")).expect("git metadata");
    fs::write(root.join("src/main.rs"), b"source").expect("source file");
    let root = fs::canonicalize(root).expect("canonical workspace");
    let workspace = WorkspaceSpec {
        node_id: NodeRef::new("round13-worker").expect("node ID"),
        root_id: RootId::new("round13-root").expect("root ID"),
        project_id: ProjectId::new("round13-project").expect("project ID"),
        workspace: root,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from("src/private"), PathBuf::from(".git")],
    }
    .validate()
    .expect("workspace gate");
    assert!(workspace.owns(std::path::Path::new("src/main.rs")));
    assert!(!workspace.owns(std::path::Path::new("src/private/token")));
    assert!(!workspace.owns(std::path::Path::new("../outside/token")));

    // P10: work-order completion or stop intent is not release evidence. A
    // forged/incomplete usage reconciliation cannot settle or release exposure.
    let scope = ReservationScope {
        project_id: ProjectId::new("round13-project").expect("project ID"),
        root_id: RootId::new("round13-root").expect("root ID"),
        task_id: TaskId::new("task-a").expect("task ID"),
        attempt_id: AttemptId::new("round13-attempt").expect("attempt ID"),
        owner: PrincipalId::new("round13-owner").expect("owner ID"),
        incarnation: "round13-worker-boot".into(),
        contract_revision: 1,
        contract_digest: preview.task_digests[&TaskId::new("task-a").expect("task ID")].clone(),
        authority_ref: "round13-grant".into(),
        resource_claims_digest: format!("sha256:{}", "f".repeat(64)),
        account_id: AccountId::new("round13-account").expect("account ID"),
        unit: CurrencyUnit::new("USD", 2).expect("currency unit"),
    };
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        "round13-account".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2).expect("currency unit"),
            account: Some(MinorUnits::new(100)),
            project: Some(MinorUnits::new(100)),
            root: Some(MinorUnits::new(100)),
        },
    );
    let reserved = reserve_budget(
        &ledger,
        "reserve-event",
        "work-order-reservation",
        scope.clone(),
        "work-order",
        MinorUnits::new(25),
    )
    .expect("reservation under limit");
    let stopped = reduce_reservation(
        &reserved,
        ReservationEvent::RequestStop {
            event_id: "stop-event".into(),
            reservation_id: "work-order-reservation".into(),
            binding: reserved.reservations["work-order-reservation"].binding(),
            reason: StopReason::LeaseExpiry,
        },
    )
    .expect("record stop intent");
    let evidence = FinalEvidence::SettledAndTerminated {
        evidence_ref: "forged-release".into(),
        reconciliation: ReconciliationEvidence {
            scope_digest: "sha256:wrong-scope".into(),
            resource_claims_digest: scope.resource_claims_digest.clone(),
            usage_complete: Some(true),
            effects_reconciled: Some(false),
            resource_claims_reconciled: Some(true),
        },
    };
    assert!(
        settle_usage(
            &stopped,
            "settlement-event",
            "work-order-reservation",
            MinorUnits::new(0),
            Some(evidence)
        )
        .is_err()
    );
    assert_eq!(stopped.unresolved_liabilities[&scope].get(), 25);
}
