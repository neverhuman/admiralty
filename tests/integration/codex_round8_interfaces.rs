use admiralty_core::interfaces::{
    CompatibilityEvidence, DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId,
    InterfaceScope, RecipientEpoch, reduce_interface_for,
};
use admiralty_core::{
    AttemptId,
    conflicts::{CandidateChange, MAX_CANDIDATES, analyze_conflicts},
    placement::{
        BudgetRequirement, Capacity, DecimalU64 as PlacementDecimal, Known, NodeId, NodeOffer,
        Observation, PlacementPolicy, PlacementRequest, propose_placement,
    },
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::{
    providers::claude,
    recovery::{
        self, ArtifactRecord, BackupManifest, Compatibility, DecimalU64 as RecoveryDecimal,
        Reconciliation, RecoveryObservation, RecoveryScope, SessionCustody,
    },
    reservations::{
        AccountId, CurrencyUnit, MinorUnits, ReservationError, ReservationEvent, ReservationLedger,
        ReservationScope, ScopeLimits, reduce_reservation, reserve_budget,
    },
    session::{InputKind, SessionHost},
    workspace::registry::{Custody, NodeRef, WorkspaceSpec},
};
use admiralty_store::artifacts::ArtifactStore;
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{fs, path::PathBuf};

fn principal(value: &str) -> PrincipalId {
    PrincipalId::new(value).expect("fixture principal")
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn fixture_digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

#[test]
fn p07_p17_round8_p09_captain_interfaces_qualification() {
    let temp = tempfile::tempdir().expect("private workspace fixture");
    let workspace = temp.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace directory");
    fs::create_dir(workspace.join("src")).expect("owned source directory");
    let artifact = b"pub fn interface_v1() -> u32 { 7 }\n";
    fs::write(workspace.join("src/lib.rs"), artifact).expect("fixture artifact");

    let registered = WorkspaceSpec {
        node_id: NodeRef::new("node-local").expect("node reference"),
        root_id: RootId::new("round8-root").expect("root ID"),
        project_id: ProjectId::new("round8-project").expect("project ID"),
        workspace: fs::canonicalize(&workspace).expect("canonical workspace"),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    }
    .validate()
    .expect("P09 registered workspace validates");
    assert_eq!(registered.custody, Custody::RegisteredOnly);
    assert!(registered.owns(&PathBuf::from("src/lib.rs")));
    let artifact_digest = digest(&fs::read(workspace.join("src/lib.rs")).expect("artifact bytes"));

    let scope = InterfaceScope {
        project_id: ProjectId::new("round8-project").expect("project ID"),
        root_id: RootId::new("round8-root").expect("root ID"),
        task_id: TaskId::new("round8-task").expect("task ID"),
        authority_ref: "grant:round8".into(),
    };
    let producer = principal("worker");
    let captain = principal("captain");
    let owner = principal("authority-owner");
    let contract = InterfaceContract {
        interface_id: InterfaceId::new("captain-api").expect("interface ID"),
        scope: scope.clone(),
        producer: producer.clone(),
        consumers: vec![captain.clone()],
        revision: DecimalU64::new(1),
        artifact_digest: artifact_digest.clone(),
        schema_digest: digest(b"schema-v1"),
    };
    let publish = InterfaceEvent::Publish {
        event_id: MessageId::new("round8-publish-1").expect("event ID"),
        contract: contract.clone(),
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "captain-epoch-4".into(),
        }],
        expected_state_revision: None,
    };
    let published = reduce_interface_for(&None, &publish, &scope, &producer, &owner)
        .expect("producer publishes scoped artifact contract");

    let ack = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round8-captain-ack").expect("event ID"),
        interface_id: contract.interface_id.clone(),
        scope: scope.clone(),
        actor: captain.clone(),
        recipient_epoch: "captain-epoch-4".into(),
        interface_revision: DecimalU64::new(1),
        artifact_digest: artifact_digest.clone(),
        schema_digest: contract.schema_digest.clone(),
        expected_state_revision: DecimalU64::new(1),
        authority_ref: scope.authority_ref.clone(),
    };
    let mut wrong_scope = scope.clone();
    wrong_scope.task_id = TaskId::new("foreign-task").expect("foreign task ID");
    assert!(
        reduce_interface_for(
            &Some(published.state.clone()),
            &ack,
            &wrong_scope,
            &captain,
            &owner,
        )
        .is_err(),
        "captain acknowledgment must be bound to the exact task scope"
    );
    assert!(
        reduce_interface_for(
            &Some(published.state.clone()),
            &ack,
            &scope,
            &principal("untrusted-peer"),
            &owner,
        )
        .is_err(),
        "a peer cannot acknowledge as the captain"
    );

    let acknowledged = reduce_interface_for(&Some(published.state), &ack, &scope, &captain, &owner)
        .expect("authenticated captain acknowledges exact artifact");
    assert_eq!(
        acknowledged.state.evidence.get(&captain),
        Some(&CompatibilityEvidence::ProvisionalAck {
            artifact_digest,
            schema_digest: contract.schema_digest,
        })
    );

    let revised_contract = InterfaceContract {
        revision: DecimalU64::new(2),
        artifact_digest: digest(b"pub fn interface_v2() -> u32 { 8 }\n"),
        schema_digest: digest(b"schema-v2"),
        ..contract
    };
    let revised = InterfaceEvent::Publish {
        event_id: MessageId::new("round8-publish-2").expect("event ID"),
        contract: revised_contract,
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "captain-epoch-5".into(),
        }],
        expected_state_revision: Some(DecimalU64::new(2)),
    };
    let invalidated = reduce_interface_for(
        &Some(acknowledged.state),
        &revised,
        &scope,
        &producer,
        &owner,
    )
    .expect("new artifact contract records dependency invalidation");
    assert_eq!(invalidated.invalidations.len(), 1);
    assert_eq!(invalidated.invalidations[0].consumer, captain);
    assert_eq!(
        invalidated.invalidations[0].previous_revision,
        DecimalU64::new(1)
    );
    assert_eq!(
        invalidated.invalidations[0].new_revision,
        DecimalU64::new(2)
    );
}

fn budget_placement_offer(budget_remaining: u64) -> NodeOffer {
    NodeOffer {
        node_id: NodeId::new("budget-node").expect("node ID"),
        boot_id: "boot-budget".into(),
        authority_generation: PlacementDecimal::new(1),
        visible_scopes: Observation {
            value: vec!["budget-project".into()],
            observed_at: PlacementDecimal::new(10),
        },
        capabilities: Observation {
            value: vec!["cpu".into()],
            observed_at: PlacementDecimal::new(10),
        },
        account_pool: Observation {
            value: "pool".into(),
            observed_at: PlacementDecimal::new(10),
        },
        capacity: Observation {
            value: Known::Known {
                value: Capacity {
                    slots: PlacementDecimal::new(1),
                    budget_remaining: PlacementDecimal::new(budget_remaining),
                },
            },
            observed_at: PlacementDecimal::new(10),
        },
        current_pressure: Observation {
            value: Known::Known {
                value: PlacementDecimal::new(0),
            },
            observed_at: PlacementDecimal::new(10),
        },
        spool_free: Observation {
            value: Known::Known {
                value: PlacementDecimal::new(10),
            },
            observed_at: PlacementDecimal::new(10),
        },
        existing_active_roots: Vec::new(),
        controller_connected: true,
        locality_refs: Vec::new(),
        continuation_permit: None,
        reconciliation_required: false,
        ownership_observed_at: Some(PlacementDecimal::new(10)),
        ownership_revision: Some(PlacementDecimal::new(1)),
    }
}

#[test]
fn p07_p17_round9_item3_budget_enforcer_exhaustion_qualification() {
    // P04: artifact publication must reject a payload above its configured quota.
    let temp = tempfile::tempdir().expect("bounded fixture root");
    let artifact_root = temp.path().join("artifact-objects");
    fs::create_dir(&artifact_root).expect("artifact root");
    #[cfg(unix)]
    fs::set_permissions(&artifact_root, fs::Permissions::from_mode(0o700))
        .expect("private artifact root");
    let artifacts = ArtifactStore::open(&artifact_root, 8).expect("small artifact quota");
    assert!(
        artifacts.publish_bytes(b"nine-byte").is_err(),
        "oversized artifact cannot consume custody storage"
    );

    // P07 and P21: oversized terminal input and provider prompt frames fail before launch.
    let session_host = SessionHost::new();
    assert!(
        session_host
            .write_input("missing-run", InputKind::Human, &vec![b'x'; 4097])
            .is_err()
    );
    assert!(claude::ClaudeLaunch::input_frame(&"x".repeat(4096)).is_err());

    // P18: conflict analysis refuses the first candidate above its bounded generation.
    let make_candidate = |n: usize| CandidateChange {
        project_id: ProjectId::new("budget-project").expect("project ID"),
        root_id: RootId::new("budget-root").expect("root ID"),
        task_id: TaskId::new(format!("task-{n}")).expect("task ID"),
        candidate_id: format!("candidate-{n}"),
        base_sha: fixture_digest('a'),
        head_sha: fixture_digest('b'),
        tree_sha: fixture_digest('c'),
        ancestry_digest: Some(fixture_digest('d')),
        files: Vec::new(),
        constraints: None,
    };
    let too_many = (0..=MAX_CANDIDATES).map(make_candidate).collect::<Vec<_>>();
    assert!(analyze_conflicts(&too_many, "budget-policy").is_err());

    // P24: a fresh node with insufficient remaining budget cannot receive a proposal.
    let placement_request = PlacementRequest {
        project: ProjectId::new("budget-project").expect("project ID"),
        root: RootId::new("budget-root").expect("root ID"),
        task: TaskId::new("budget-task").expect("task ID"),
        contract_digest: fixture_digest('e'),
        authority_ref: "grant:budget".into(),
        required_capabilities: vec!["cpu".into()],
        locality_refs: Vec::new(),
        budget_requirement: BudgetRequirement {
            account_pool: "pool".into(),
            amount: PlacementDecimal::new(5),
        },
    };
    let placement_policy = PlacementPolicy {
        max_observation_age: PlacementDecimal::new(10),
        maximum_pressure: PlacementDecimal::new(5),
        minimum_spool_reserve: PlacementDecimal::new(1),
        disconnected_continuation_only: true,
        ownership_revision: Some(PlacementDecimal::new(1)),
    };
    let proposal = propose_placement(
        &placement_request,
        &[budget_placement_offer(4)],
        &placement_policy,
        10,
    )
    .expect("budget shortage is a rejected proposal");
    assert!(proposal.candidates.is_empty());
    assert!(!proposal.execution_granted);

    // P25: unknown session custody blocks recovery resume even with matching snapshot facts.
    let scope = RecoveryScope {
        project_id: ProjectId::new("budget-project").expect("project ID"),
        root_id: RootId::new("budget-root").expect("root ID"),
        task_id: TaskId::new("budget-task").expect("task ID"),
        store_id: "store-one".into(),
        attempt_id: AttemptId::new("attempt-one").expect("attempt ID"),
        owner: principal("owner"),
        revision: 1,
        contract_digest: fixture_digest('f'),
        executor_incarnation: "executor-old".into(),
    };
    let mut manifest = BackupManifest {
        scope: scope.clone(),
        schema_version: 1,
        snapshot_watermark: RecoveryDecimal::new(10),
        snapshot_digest: fixture_digest('c'),
        snapshot_size: RecoveryDecimal::new(8),
        recovery_generation: RecoveryDecimal::new(2),
        artifact_closure: Vec::<ArtifactRecord>::new(),
        active_bundle_digests: vec![fixture_digest('d')],
        manifest_digest: String::new(),
    };
    manifest.manifest_digest = recovery::manifest_digest(&manifest).expect("manifest digest");
    let observation = RecoveryObservation {
        scope: scope.clone(),
        manifest_digest: manifest.manifest_digest.clone(),
        snapshot_digest: manifest.snapshot_digest.clone(),
        snapshot_size: manifest.snapshot_size.clone(),
        snapshot_watermark: manifest.snapshot_watermark.clone(),
        backup_generation: manifest.recovery_generation.clone(),
        current_generation: RecoveryDecimal::new(3),
        old_executor_incarnation: "executor-old".into(),
        observed_bundle_digests: manifest.active_bundle_digests.clone(),
        available_artifacts: Vec::new(),
        compatibility: Compatibility::Compatible,
        unresolved_effects: Reconciliation::Clear,
        session_custody: SessionCustody::Unknown,
    };
    let recovery = recovery::plan_recovery(&manifest, &observation, None)
        .expect("unknown custody becomes a blocker");
    assert!(!recovery.resume_proposed);
    assert!(
        recovery
            .blockers
            .iter()
            .any(|blocker| matches!(blocker, recovery::Blocker::UnknownSessionCustody))
    );

    // P26: reservation exhaustion keeps the account liability and rejects overspend.
    let account = AccountId::new("acct-budget").expect("account ID");
    let unit = CurrencyUnit::new("USD", 2).expect("currency unit");
    let ledger = reduce_reservation(
        &ReservationLedger::new(),
        ReservationEvent::SetLimits {
            event_id: "set-budget-limit".into(),
            account_id: account.clone(),
            limits: ScopeLimits {
                unit: unit.clone(),
                account: Some(MinorUnits::new(10)),
                project: Some(MinorUnits::new(10)),
                root: Some(MinorUnits::new(10)),
            },
        },
    )
    .expect("set scoped account budget");
    let reservation_scope = ReservationScope {
        project_id: ProjectId::new("budget-project").expect("project ID"),
        root_id: RootId::new("budget-root").expect("root ID"),
        task_id: TaskId::new("budget-task").expect("task ID"),
        attempt_id: AttemptId::new("attempt-budget").expect("attempt ID"),
        owner: principal("owner"),
        incarnation: "boot-budget".into(),
        contract_revision: 1,
        contract_digest: fixture_digest('9'),
        authority_ref: "grant:budget".into(),
        resource_claims_digest: fixture_digest('0'),
        account_id: account,
        unit,
    };
    let reserved = reserve_budget(
        &ledger,
        "reserve-first",
        "reservation-first",
        reservation_scope.clone(),
        "correlation-first",
        MinorUnits::new(10),
    )
    .expect("reserve exact account limit");
    assert!(matches!(
        reserve_budget(
            &reserved,
            "reserve-second",
            "reservation-second",
            reservation_scope,
            "correlation-second",
            MinorUnits::new(1),
        ),
        Err(ReservationError::LimitExceeded("account"))
    ));
}
