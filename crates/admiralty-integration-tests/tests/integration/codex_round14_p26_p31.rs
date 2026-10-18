use admiralty_core::{
    AcceptanceCriterion, CheckSpec, ProjectId, RootId, TaskContract, TaskId, content_digest,
    economics::{
        BoundedId, CohortSpec, Coverage, DecimalU64, DecimalU128, EconomicsError, ObservedAmount,
        ParentInclusion, RootBinding, UsageRecord, summarize_cohort,
    },
    scheduler::{
        Knowledge, LeaseObservation, ReadinessBlocker, ScheduledTask, SchedulerSnapshot,
        SchedulingScope, TaskSchedulingState, propose_ready_tasks,
    },
};
use std::error::Error;

const HEX_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HEX_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HEX_C: &str = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

fn binding(root: RootId, task: TaskId) -> RootBinding {
    RootBinding {
        root_id: root,
        task_id: task,
        contract_revision: 1,
        contract_digest: HEX_A.to_owned(),
        protected_oracle_digest: HEX_B.to_owned(),
    }
}

fn usage(
    project: &ProjectId,
    binding: &RootBinding,
    id: &str,
    unit: &str,
    amount: ObservedAmount,
) -> Result<UsageRecord, Box<dyn Error>> {
    Ok(UsageRecord {
        project_id: project.clone(),
        binding: binding.clone(),
        observation_id: BoundedId::new(id)?,
        revision: DecimalU64::parse("1")?,
        root_scope: binding.root_id.clone(),
        role: BoundedId::new("worker")?,
        purpose: BoundedId::new("generation")?,
        provider: BoundedId::new("provider")?,
        unit: BoundedId::new(unit)?,
        amount,
        parent_inclusion: ParentInclusion::Standalone,
    })
}

fn contract(
    project: &ProjectId,
    root: &RootId,
    name: &str,
) -> Result<TaskContract, Box<dyn Error>> {
    Ok(TaskContract {
        schema_version: 1,
        project_id: project.clone(),
        root_id: root.clone(),
        task_id: TaskId::new(name)?,
        revision: 1,
        title: format!("Task {name}"),
        objective: "Bounded deterministic quota fixture".to_owned(),
        non_goals: Vec::new(),
        owned_paths: vec!["src".to_owned()],
        forbidden_paths: vec![".git".to_owned()],
        read_first: Vec::new(),
        dependencies: Vec::new(),
        criteria: vec![AcceptanceCriterion {
            id: "check-passes".to_owned(),
            text: "Named check passes".to_owned(),
            check_ids: vec!["check".to_owned()],
        }],
        checks: vec![CheckSpec {
            id: "check".to_owned(),
            program: "true".to_owned(),
            args: Vec::new(),
            cwd: ".".to_owned(),
            timeout_secs: 10,
        }],
        authority_ref: "fixture:round14".to_owned(),
        max_attempts: 1,
        simulated: true,
    })
}

fn scheduled(task: TaskContract) -> Result<ScheduledTask, Box<dyn Error>> {
    Ok(ScheduledTask {
        contract_digest: content_digest(&task)?,
        observed_contract_revision: task.revision,
        contract: task,
        state: TaskSchedulingState::Pending,
        lease: LeaseObservation::Absent,
    })
}

#[test]
fn codex_round14_p26_p31() -> Result<(), Box<dyn Error>> {
    let project = ProjectId::new("round14-project")?;
    let root = RootId::new("round14-root")?;
    let task = TaskId::new("round14-task")?;
    let binding = binding(root.clone(), task);
    let cohort = CohortSpec {
        cohort_id: BoundedId::new("round14-cohort")?,
        project_id: project.clone(),
        fixed_assignment_roots: vec![binding.root_id.clone()],
        required_delivery_stage: BoundedId::new("release")?,
        valuation_digest: HEX_C.to_owned(),
        assigned_at: DecimalU64::parse("42")?,
        bindings: vec![binding.clone()],
    };

    // Economics ledger replay is order independent and byte-identical retries
    // do not double-count; exact cost units stay separate from token usage.
    let token_a = usage(
        &project,
        &binding,
        "token-a",
        "tokens",
        ObservedAmount::Known {
            amount: DecimalU128(5),
        },
    )?;
    let token_b = usage(
        &project,
        &binding,
        "token-b",
        "tokens",
        ObservedAmount::Known {
            amount: DecimalU128(7),
        },
    )?;
    let cost = usage(
        &project,
        &binding,
        "cost-a",
        "USD:6",
        ObservedAmount::Known {
            amount: DecimalU128(1250),
        },
    )?;
    let entries = vec![token_a.clone(), token_b.clone(), cost];
    let replay = vec![
        token_b.clone(),
        token_a.clone(),
        token_a.clone(),
        usage(
            &project,
            &binding,
            "cost-a",
            "USD:6",
            ObservedAmount::Known {
                amount: DecimalU128(1250),
            },
        )?,
    ];
    let ledger = summarize_cohort(&cohort, &entries, &[], 0)?;
    assert_eq!(ledger, summarize_cohort(&cohort, &replay, &[], 0)?);
    assert!(ledger.units.iter().any(|total| {
        total.unit.as_str() == "tokens"
            && total
                .amount
                .as_ref()
                .is_some_and(|amount| amount.as_u128() == 12)
            && total.coverage == Coverage::Complete
    }));
    assert!(ledger.units.iter().any(|total| {
        total.unit.as_str() == "USD:6"
            && total
                .amount
                .as_ref()
                .is_some_and(|amount| amount.as_u128() == 1250)
    }));

    // Conflicting revisions and arithmetic overflow fail closed; unknown cost
    // never becomes a zero-valued complete envelope.
    let mut collision = token_a.clone();
    collision.amount = ObservedAmount::Known {
        amount: DecimalU128(6),
    };
    assert_eq!(
        summarize_cohort(&cohort, &[token_a.clone(), collision], &[], 0),
        Err(EconomicsError::RevisionCollision)
    );
    let overflow = vec![
        usage(
            &project,
            &binding,
            "max-cost",
            "USD:6",
            ObservedAmount::Known {
                amount: DecimalU128(u128::MAX),
            },
        )?,
        usage(
            &project,
            &binding,
            "overflow-cost",
            "USD:6",
            ObservedAmount::Known {
                amount: DecimalU128(1),
            },
        )?,
    ];
    assert_eq!(
        summarize_cohort(&cohort, &overflow, &[], 0),
        Err(EconomicsError::Overflow)
    );
    let unknown_cost = usage(
        &project,
        &binding,
        "unknown-cost",
        "USD:6",
        ObservedAmount::Unknown {
            reason: "provider omitted cost".to_owned(),
        },
    )?;
    let unknown_summary = summarize_cohort(&cohort, &[unknown_cost], &[], 0)?;
    assert!(unknown_summary.units.iter().any(|total| {
        total.unit.as_str() == "USD:6"
            && total.amount.is_none()
            && total.coverage == Coverage::Unknown
    }));

    // Quota and capacity are bounded observations: proposals replay stably,
    // quota exhaustion and unknown quota never admit extra work or execute it.
    let scope = SchedulingScope {
        project_id: project.clone(),
        root_id: root.clone(),
        revision: 1,
    };
    let mut snapshot = SchedulerSnapshot {
        schema_version: 1,
        scope: scope.clone(),
        tasks: vec![
            scheduled(contract(&project, &root, "task-b")?)?,
            scheduled(contract(&project, &root, "task-a")?)?,
        ],
        available_quota: Knowledge::Known(1),
        available_capacity: Knowledge::Known(2),
    };
    let first = propose_ready_tasks(&snapshot, &scope)?;
    assert_eq!(first, propose_ready_tasks(&snapshot, &scope)?);
    assert!(!first.execution_granted);
    assert_eq!(first.ready, vec![TaskId::new("task-a")?]);
    assert!(
        first
            .blocked
            .get(&TaskId::new("task-b")?)
            .is_some_and(|reasons| { reasons.contains(&ReadinessBlocker::QuotaLimit) })
    );
    snapshot.available_quota = Knowledge::Unknown;
    let unknown_quota = propose_ready_tasks(&snapshot, &scope)?;
    assert!(unknown_quota.ready.is_empty());
    assert!(
        unknown_quota
            .blocked
            .values()
            .flatten()
            .any(|reason| { *reason == ReadinessBlocker::QuotaUnknown })
    );
    let wrong_scope = SchedulingScope {
        revision: 2,
        ..scope.clone()
    };
    assert!(propose_ready_tasks(&snapshot, &wrong_scope).is_err());
    Ok(())
}
