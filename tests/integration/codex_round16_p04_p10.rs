use admiralty_core::{
    AttemptId, DomainEvent, PrincipalId, Projection, TaskContract, content_digest, reduce,
};
use admiralty_runtime::{
    models::{ModelSpec, Provider},
    providers::{
        codex::{
            CodexAdmission, CodexCliObservation, CodexEffort, CodexError, CodexExecutionOptions,
            CodexSandbox, SUPPORTED_CLI_VERSION, prepare_launch,
        },
        supervision::SupervisionScope,
    },
    reservations::{
        AccountId, CurrencyUnit, FinalEvidence, MinorUnits, ReconciliationEvidence,
        ReservationEvent, ReservationLedger, ReservationScope, ScopeLimits, StopReason,
        reduce_reservation, settle_usage,
    },
    workspace::registry::{NodeRef, WorkspaceSpec},
};
use admiralty_testkit::{
    faults::{FaultEffect, FaultPlan, FaultRule},
    task_fixture,
};
use std::{
    collections::BTreeSet,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

fn fixture() -> Result<(TaskContract, ModelSpec), Box<dyn Error>> {
    let mut contract = task_fixture("round16-cutover-task");
    contract.simulated = false;
    let model = ModelSpec {
        id: "round16-codex-model".into(),
        label: "Round 16 deterministic model".into(),
        provider: Provider::CodexCli,
        provider_model: "fixture-model".into(),
        tier: 0,
        enabled: true,
        node_id: Some("round16-node".into()),
        endpoint: None,
        credential_file_ref: None,
        max_concurrency: 1,
        capabilities: BTreeSet::from(["code".into()]),
        task_max_attempts: 1,
        escalate_to: None,
    };
    Ok((contract, model))
}

fn admission(
    contract: &TaskContract,
    model: &ModelSpec,
    cli: &CodexCliObservation,
    working_dir: &Path,
    spool_root: &Path,
) -> Result<CodexAdmission, Box<dyn Error>> {
    Ok(CodexAdmission {
        scope: SupervisionScope {
            project_id: contract.project_id.clone(),
            root_id: contract.root_id.clone(),
            task_id: contract.task_id.clone(),
            attempt_id: AttemptId::new("round16-attempt")?,
            owner: PrincipalId::new("round16-owner")?,
            incarnation: "round16-worker-boot".into(),
            authority_ref: contract.authority_ref.clone(),
            contract_digest: content_digest(contract)?,
            revision: contract.revision,
        },
        attempt_ordinal: 1,
        registry_revision: "16".into(),
        selected_model_digest: content_digest(model)?,
        cli_observation_digest: cli.fingerprint().into(),
        node_id: "round16-node".into(),
        working_dir: working_dir.to_path_buf(),
        spool_root: spool_root.to_path_buf(),
        lease_ref: "round16-lease".into(),
        resource_permit_ref: "round16-resource-permit".into(),
        custody_ref: "round16-custody".into(),
        auth_context_ref: "round16-auth-context".into(),
        admitted_tick: 10,
        expires_tick: 20,
    })
}

#[test]
fn p00_p01_p04_p10_round16_end_to_end_cutover_fault_containment() -> Result<(), Box<dyn Error>> {
    let (contract, model) = fixture()?;

    // P00: replay the same accepted contract to the same projection, and reject
    // a same-ID mutation before it can change projected state.
    let submission = DomainEvent::TaskSubmitted {
        contract: contract.clone(),
    };
    let projection = reduce(&Projection::default(), &submission)?;
    assert_eq!(reduce(&Projection::default(), &submission)?, projection);
    let mut changed = contract.clone();
    changed.forbidden_paths.push("src/credentials".into());
    assert!(
        reduce(
            &projection,
            &DomainEvent::TaskSubmitted { contract: changed }
        )
        .is_err()
    );
    assert_eq!(projection, reduce(&Projection::default(), &submission)?);

    // P01: cutover fault schedules replay exactly. A failure is contained as
    // recorded data; it does not authorize execution or erase held liability.
    let rules = vec![FaultRule {
        point: "release-cutover".into(),
        occurrence: 2,
        effect: FaultEffect::Fail,
    }];
    let mut first = FaultPlan::new(rules.clone())?;
    let mut replay = FaultPlan::new(rules)?;
    let mut observed = Vec::new();
    for _ in 0..3 {
        let left = first.hit("release-cutover")?;
        assert_eq!(left, replay.hit("release-cutover")?);
        observed.push(left);
    }
    assert_eq!(
        observed,
        vec![None, Some(FaultEffect::Fail), None],
        "the cutover failure fires only at its scripted step"
    );
    assert!(
        FaultPlan::new(vec![FaultRule {
            point: "release-cutover".into(),
            occurrence: 0,
            effect: FaultEffect::Fail,
        }])
        .is_err()
    );

    // P04: custody and a live lease bind the deterministic execution envelope.
    let directory = tempfile::tempdir()?;
    let worktree = directory.path().join("worktree");
    let spool = directory.path().join("spool");
    fs::create_dir_all(worktree.join("src"))?;
    fs::write(worktree.join("src/range.rs"), b"cutover source")?;
    fs::create_dir_all(&spool)?;
    let worktree = fs::canonicalize(worktree)?;
    let spool = fs::canonicalize(spool)?;
    let workspace = WorkspaceSpec {
        node_id: NodeRef::new("round16-node")?,
        root_id: contract.root_id.clone(),
        project_id: contract.project_id.clone(),
        workspace: worktree.clone(),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    }
    .validate()?;
    assert!(workspace.owns(Path::new("src/range.rs")));
    assert!(!workspace.owns(Path::new("../outside/command")));

    let cli = CodexCliObservation::from_help(
        "round16-node",
        PathBuf::from("/usr/bin/codex"),
        SUPPORTED_CLI_VERSION,
        "--json --color --ignore-user-config --strict-config --ephemeral --model --sandbox --cd",
        "--no-daemon --ask-for-approval",
        Some(10),
    )?;
    let options = CodexExecutionOptions {
        effort: CodexEffort::Medium,
        sandbox: CodexSandbox::WorkspaceWrite,
    };
    let prepared = prepare_launch(
        &contract,
        admission(&contract, &model, &cli, &worktree, &spool)?,
        &model,
        options,
        &cli,
        "round16 bounded task capsule",
        Some(19),
    )?;
    let prepared_replay = prepare_launch(
        &contract,
        admission(&contract, &model, &cli, &worktree, &spool)?,
        &model,
        options,
        &cli,
        "round16 bounded task capsule",
        Some(19),
    )?;
    assert_eq!(prepared.tuple_digest(), prepared_replay.tuple_digest());
    assert!(!prepared.tuple().native_execution_qualified());
    assert!(prepared.requires_stdin_eof());
    assert!(matches!(
        prepare_launch(
            &contract,
            admission(&contract, &model, &cli, &worktree, &spool)?,
            &model,
            options,
            &cli,
            "round16 bounded task capsule",
            Some(20),
        ),
        Err(CodexError::Clock)
    ));

    // P10: bind a reservation to the exact execution envelope, preserve its
    // liability through stop and rejected release evidence, then reconcile and
    // replay the successful no-usage cutover without double settlement.
    let scope = ReservationScope {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new("round16-attempt")?,
        owner: PrincipalId::new("round16-owner")?,
        incarnation: "round16-worker-boot".into(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract)?,
        authority_ref: contract.authority_ref.clone(),
        resource_claims_digest: content_digest(&contract.owned_paths)?,
        account_id: AccountId::new("round16-account")?,
        unit: CurrencyUnit::new("USD", 2)?,
    };
    let mut ledger = ReservationLedger::new();
    ledger.limits.insert(
        "round16-account".into(),
        ScopeLimits {
            unit: CurrencyUnit::new("USD", 2)?,
            account: Some(MinorUnits::new(100)),
            project: Some(MinorUnits::new(100)),
            root: Some(MinorUnits::new(100)),
        },
    );
    let reserve_event = ReservationEvent::Reserve {
        event_id: "round16-reserve".into(),
        reservation_id: "round16-reservation".into(),
        scope: scope.clone(),
        correlation_id: prepared.tuple_digest().into(),
        amount: MinorUnits::new(40),
    };
    ledger = reduce_reservation(&ledger, reserve_event.clone())?;
    assert_eq!(
        reduce_reservation(&ledger, reserve_event)?,
        ledger,
        "exact reserve replay is idempotent"
    );
    let stop_event = ReservationEvent::RequestStop {
        event_id: "round16-stop".into(),
        reservation_id: "round16-reservation".into(),
        binding: ledger
            .reservations
            .get("round16-reservation")
            .ok_or("reservation missing after cutover admission")?
            .binding(),
        reason: StopReason::Cancellation,
    };
    let stopped = reduce_reservation(&ledger, stop_event)?;
    assert_eq!(
        stopped.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(40))
    );
    let forged = FinalEvidence::ConfirmedNoUsage {
        evidence_ref: "round16-forged-receipt".into(),
        reconciliation: ReconciliationEvidence {
            scope_digest: "sha256:wrong".into(),
            resource_claims_digest: scope.resource_claims_digest.clone(),
            usage_complete: Some(true),
            effects_reconciled: Some(true),
            resource_claims_reconciled: Some(true),
        },
    };
    assert!(
        settle_usage(
            &stopped,
            "round16-forged-settlement",
            "round16-reservation",
            MinorUnits::new(0),
            Some(forged),
        )
        .is_err()
    );
    assert_eq!(
        stopped.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(40))
    );

    let evidence = FinalEvidence::ConfirmedNoUsage {
        evidence_ref: "round16-reconciled-no-usage".into(),
        reconciliation: ReconciliationEvidence {
            scope_digest: content_digest(&scope)?,
            resource_claims_digest: scope.resource_claims_digest.clone(),
            usage_complete: Some(true),
            effects_reconciled: Some(true),
            resource_claims_reconciled: Some(true),
        },
    };
    let settled = settle_usage(
        &stopped,
        "round16-settlement",
        "round16-reservation",
        MinorUnits::new(0),
        Some(evidence.clone()),
    )?;
    let final_binding = stopped
        .reservations
        .get("round16-reservation")
        .ok_or("reservation missing before release")?
        .binding();
    let replayed_settlement = reduce_reservation(
        &settled,
        ReservationEvent::Settle {
            event_id: "round16-settlement".into(),
            reservation_id: "round16-reservation".into(),
            binding: final_binding,
            amount: MinorUnits::new(0),
            final_evidence: Some(evidence),
        },
    )?;
    assert_eq!(replayed_settlement, settled);
    assert_eq!(
        settled.unresolved_liabilities.get(&scope),
        Some(&MinorUnits::new(0))
    );
    assert_eq!(settled.settled.get(&scope), Some(&MinorUnits::new(0)));
    Ok(())
}
