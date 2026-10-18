use admiralty_core::coordination::questions::{QuestionEvent, QuestionStatus};
use admiralty_core::{
    AcceptanceCriterion, AttemptId, CheckSpec, PrincipalId, ProjectId, RootId, TaskContract,
    TaskId, content_digest,
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
};
use admiralty_store::questions::{
    JournalScope, append as append_question, initialize as initialize_questions,
    reconstruct as reconstruct_question,
};
use rusqlite::Connection;
use std::collections::BTreeSet;

fn fixture() -> (TaskContract, ModelSpec) {
    let contract = TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("project").expect("project fixture"),
        root_id: RootId::new("root").expect("root fixture"),
        task_id: TaskId::new("task").expect("task fixture"),
        revision: 2,
        title: "Scoped worktree lease".into(),
        objective: "Prepare a bounded Codex launch under a live lease".into(),
        non_goals: vec![],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec!["secrets".into()],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "C1".into(),
            text: "The exact launch remains bound to its lease".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "fixture-check".into(),
            args: vec![],
            cwd: ".".into(),
            timeout_secs: 1,
        }],
        authority_ref: "authority:fixture".into(),
        max_attempts: 1,
        simulated: false,
    };
    let model = ModelSpec {
        id: "registry-codex".into(),
        label: "Selected Codex".into(),
        provider: Provider::CodexCli,
        provider_model: "native-model".into(),
        tier: 0,
        enabled: true,
        node_id: Some("node".into()),
        endpoint: None,
        credential_file_ref: None,
        max_concurrency: 1,
        capabilities: BTreeSet::from(["code".into()]),
        task_max_attempts: 1,
        escalate_to: None,
    };
    (contract, model)
}

#[test]
fn p05_p15_round8_p08_worktree_lease_qualification() {
    let (contract, model) = fixture();
    let base = std::env::temp_dir().join("codex-worktree-lease-qualification");
    let cli = CodexCliObservation::from_help(
        "node",
        "/usr/bin/codex".into(),
        SUPPORTED_CLI_VERSION,
        "--json --color --ignore-user-config --strict-config --ephemeral --model --sandbox --cd",
        "--no-daemon --ask-for-approval",
        Some(10),
    )
    .expect("CLI help fixture parses");
    let admission = || CodexAdmission {
        scope: SupervisionScope {
            project_id: contract.project_id.clone(),
            root_id: contract.root_id.clone(),
            task_id: contract.task_id.clone(),
            attempt_id: AttemptId::new("attempt").expect("attempt fixture"),
            owner: PrincipalId::new("captain").expect("captain fixture"),
            incarnation: "incarnation".into(),
            authority_ref: contract.authority_ref.clone(),
            contract_digest: content_digest(&contract).expect("contract digest"),
            revision: contract.revision,
        },
        attempt_ordinal: 1,
        registry_revision: "17".into(),
        selected_model_digest: content_digest(&model).expect("model digest"),
        cli_observation_digest: cli.fingerprint().into(),
        node_id: "node".into(),
        working_dir: base.join("worktree"),
        spool_root: base.join("spool"),
        lease_ref: "lease:worktree-7".into(),
        resource_permit_ref: "permit:7".into(),
        custody_ref: "custody:7".into(),
        auth_context_ref: "auth:node".into(),
        admitted_tick: 10,
        expires_tick: 20,
    };
    let options = CodexExecutionOptions {
        effort: CodexEffort::Medium,
        sandbox: CodexSandbox::WorkspaceWrite,
    };

    let prepared = prepare_launch(
        &contract,
        admission(),
        &model,
        options,
        &cli,
        "deterministic task capsule",
        Some(19),
    )
    .expect("launch may be prepared while the worktree lease is live");
    assert_eq!(prepared.tuple().lease_ref, "lease:worktree-7");
    assert!(!prepared.tuple().native_execution_qualified());
    assert_eq!(prepared.tuple().expires_tick, "20");

    assert!(matches!(
        prepare_launch(
            &contract,
            admission(),
            &model,
            options,
            &cli,
            "deterministic task capsule",
            Some(20),
        ),
        Err(CodexError::Clock)
    ));
}

fn question_scope(id: &str) -> JournalScope {
    JournalScope {
        root_id: RootId::new("root-recovery").expect("valid deterministic fixture"),
        question: admiralty_core::coordination::questions::QuestionScope {
            question_id: admiralty_core::MessageId::new(id).expect("valid deterministic fixture"),
            project_id: ProjectId::new("project-recovery").expect("valid deterministic fixture"),
            task_id: TaskId::new("task-recovery").expect("valid deterministic fixture"),
        },
        authority_ref: "grant:task-recovery:r3".into(),
    }
}

fn question_open(scope: &JournalScope, event_id: &str, deadline_utc: Option<u64>) -> QuestionEvent {
    QuestionEvent::Open {
        event_id: admiralty_core::MessageId::new(event_id).expect("valid deterministic fixture"),
        scope: scope.question.clone(),
        authority_ref: scope.authority_ref.clone(),
        asker: admiralty_core::PrincipalId::new("asker").expect("valid deterministic fixture"),
        recipient: admiralty_core::PrincipalId::new("worker").expect("valid deterministic fixture"),
        allowed_recipients: vec![
            admiralty_core::PrincipalId::new("worker").expect("valid deterministic fixture"),
            admiralty_core::PrincipalId::new("reviewer").expect("valid deterministic fixture"),
        ],
        requirement: "Resolve the scoped question with retained evidence".into(),
        question: "Which artifact revision is authoritative?".into(),
        options: vec!["r1".into(), "r2".into()],
        deadline_utc,
    }
}

#[test]
fn p05_p15_round9_item3_question_routing_recovery_qualification() {
    let directory = tempfile::tempdir().expect("private question journal directory");
    let database = directory.path().join("questions.sqlite3");
    let scope = question_scope("question-route");
    let open = question_open(&scope, "event-open", None);
    let route = QuestionEvent::Route {
        event_id: admiralty_core::MessageId::new("event-route")
            .expect("valid deterministic fixture"),
        scope: scope.question.clone(),
        expected_revision: 1,
        authority_ref: scope.authority_ref.clone(),
        actor: admiralty_core::PrincipalId::new("worker").expect("valid deterministic fixture"),
        recipient: admiralty_core::PrincipalId::new("reviewer")
            .expect("valid deterministic fixture"),
    };

    let mut connection = Connection::open(&database).expect("open durable question journal");
    initialize_questions(&connection).expect("initialize deterministic question schema");
    let opened = append_question(
        &mut connection,
        &scope,
        &admiralty_core::MessageId::new("command-open").expect("valid deterministic fixture"),
        &open,
    )
    .expect("open question in its exact root/project/task/authority scope");
    assert_eq!(opened.revision, "1");
    assert!(!opened.duplicate);

    let before_route = reconstruct_question(&connection, &scope)
        .expect("valid deterministic fixture")
        .expect("opened question reconstructs");
    let unauthorized = QuestionEvent::Route {
        event_id: admiralty_core::MessageId::new("event-unauthorized")
            .expect("valid deterministic fixture"),
        scope: scope.question.clone(),
        expected_revision: 1,
        authority_ref: scope.authority_ref.clone(),
        actor: admiralty_core::PrincipalId::new("intruder").expect("valid deterministic fixture"),
        recipient: admiralty_core::PrincipalId::new("reviewer")
            .expect("valid deterministic fixture"),
    };
    assert!(
        append_question(
            &mut connection,
            &scope,
            &admiralty_core::MessageId::new("command-unauthorized")
                .expect("valid deterministic fixture"),
            &unauthorized,
        )
        .is_err()
    );
    assert!(
        append_question(
            &mut connection,
            &scope,
            &admiralty_core::MessageId::new("command-stale").expect("valid deterministic fixture"),
            &QuestionEvent::Route {
                event_id: admiralty_core::MessageId::new("event-stale")
                    .expect("valid deterministic fixture"),
                scope: scope.question.clone(),
                expected_revision: 0,
                authority_ref: scope.authority_ref.clone(),
                actor: admiralty_core::PrincipalId::new("worker")
                    .expect("valid deterministic fixture"),
                recipient: admiralty_core::PrincipalId::new("reviewer")
                    .expect("valid deterministic fixture"),
            },
        )
        .is_err()
    );
    assert_eq!(
        reconstruct_question(&connection, &scope)
            .expect("valid deterministic fixture")
            .expect("valid deterministic fixture"),
        before_route,
        "rejected routing commands leave journal and projection unchanged"
    );

    let routed = append_question(
        &mut connection,
        &scope,
        &admiralty_core::MessageId::new("command-route").expect("valid deterministic fixture"),
        &route,
    )
    .expect("allowlisted current recipient can route to reviewer");
    assert_eq!(routed.revision, "2");
    assert_eq!(
        append_question(
            &mut connection,
            &scope,
            &admiralty_core::MessageId::new("command-route").expect("valid deterministic fixture"),
            &route,
        )
        .expect("valid deterministic fixture"),
        routed,
        "exact command retry returns the retained receipt"
    );
    let replay = append_question(
        &mut connection,
        &scope,
        &admiralty_core::MessageId::new("command-route-replay")
            .expect("valid deterministic fixture"),
        &route,
    )
    .expect("event replay under a new command is idempotent");
    assert!(replay.duplicate);
    assert_eq!(replay.revision, "2");
    assert!(matches!(
        append_question(
            &mut connection,
            &scope,
            &admiralty_core::MessageId::new("command-route").expect("valid deterministic fixture"),
            &QuestionEvent::Route {
                event_id: admiralty_core::MessageId::new("event-route")
                    .expect("valid deterministic fixture"),
                scope: scope.question.clone(),
                expected_revision: 1,
                authority_ref: scope.authority_ref.clone(),
                actor: admiralty_core::PrincipalId::new("worker")
                    .expect("valid deterministic fixture"),
                recipient: admiralty_core::PrincipalId::new("worker")
                    .expect("valid deterministic fixture"),
            },
        ),
        Err(admiralty_store::questions::QuestionStoreError::IdentityCollision)
    ));
    drop(connection);

    let mut connection =
        Connection::open(&database).expect("reopen question journal after restart");
    let restored = reconstruct_question(&connection, &scope)
        .expect("valid deterministic fixture")
        .expect("routed state rebuilds from durable event history");
    assert_eq!(restored.state.revision, 2);
    assert_eq!(restored.state.status, QuestionStatus::Routed);
    assert_eq!(restored.state.recipient.as_str(), "reviewer");
    assert!(
        append_question(
            &mut connection,
            &scope,
            &admiralty_core::MessageId::new("command-route-cycle")
                .expect("valid deterministic fixture"),
            &QuestionEvent::Route {
                event_id: admiralty_core::MessageId::new("event-route-cycle")
                    .expect("valid deterministic fixture"),
                scope: scope.question.clone(),
                expected_revision: 2,
                authority_ref: scope.authority_ref.clone(),
                actor: admiralty_core::PrincipalId::new("reviewer")
                    .expect("valid deterministic fixture"),
                recipient: admiralty_core::PrincipalId::new("worker")
                    .expect("valid deterministic fixture"),
            },
        )
        .is_err(),
        "a visited recipient cannot be routed back to after restart"
    );

    let expiry_scope = question_scope("question-expiry");
    let expiring = question_open(&expiry_scope, "event-expiry-open", Some(100));
    append_question(
        &mut connection,
        &expiry_scope,
        &admiralty_core::MessageId::new("command-expiry-open")
            .expect("valid deterministic fixture"),
        &expiring,
    )
    .expect("open second scoped question with deterministic deadline");
    let deadline = QuestionEvent::EvaluateDeadline {
        event_id: admiralty_core::MessageId::new("event-expiry-eval")
            .expect("valid deterministic fixture"),
        scope: expiry_scope.question.clone(),
        expected_revision: 1,
        authority_ref: expiry_scope.authority_ref.clone(),
        actor: admiralty_core::PrincipalId::new("asker").expect("valid deterministic fixture"),
        observed_utc: Some(101),
    };
    let escalation = append_question(
        &mut connection,
        &expiry_scope,
        &admiralty_core::MessageId::new("command-expiry-eval")
            .expect("valid deterministic fixture"),
        &deadline,
    )
    .expect("strictly expired deadline creates a durable escalation proposal");
    let escalation_ref = escalation
        .escalation_ref
        .expect("first deadline transition persists escalation reference");
    let expiry_replay = append_question(
        &mut connection,
        &expiry_scope,
        &admiralty_core::MessageId::new("command-expiry-replay")
            .expect("valid deterministic fixture"),
        &deadline,
    )
    .expect("deadline event replay is idempotent");
    assert!(expiry_replay.duplicate);
    assert_eq!(
        expiry_replay.escalation_ref.as_deref(),
        Some(escalation_ref.as_str())
    );
    assert!(matches!(
        append_question(
            &mut connection,
            &expiry_scope,
            &admiralty_core::MessageId::new("command-answer-expired")
                .expect("valid deterministic fixture"),
            &QuestionEvent::Answer {
                event_id: admiralty_core::MessageId::new("event-answer-expired")
                    .expect("valid deterministic fixture"),
                scope: expiry_scope.question.clone(),
                expected_revision: 2,
                authority_ref: expiry_scope.authority_ref.clone(),
                actor: admiralty_core::PrincipalId::new("worker")
                    .expect("valid deterministic fixture"),
                answer: "r2".into(),
            },
        ),
        Err(admiralty_store::questions::QuestionStoreError::Reducer(_))
    ));
    assert_eq!(
        reconstruct_question(&connection, &expiry_scope)
            .expect("valid deterministic fixture")
            .expect("valid deterministic fixture")
            .escalation_ref,
        Some(escalation_ref),
        "recovery retains the one recorded escalation reference"
    );
}

#[test]
fn p05_p15_round10_item3_workspace_gate_audit() {
    use admiralty_core::verification::parse_receipt;
    use admiralty_store::{
        artifacts::ArtifactStore,
        custody::{
            self, AcquirePinRequest, AssetBinding, ClaimRetirementRequest, CustodyObservation,
            RegisterAssetRequest, ReleasePinRequest,
        },
    };
    use std::{fs, os::unix::fs::PermissionsExt};

    let fixture = tempfile::tempdir().expect("private workspace gate fixture");
    fs::set_permissions(fixture.path(), fs::Permissions::from_mode(0o700))
        .expect("restrict fixture directory");
    let artifact_root = fixture.path().join("objects");
    fs::create_dir(&artifact_root).expect("create artifact object root");
    fs::set_permissions(&artifact_root, fs::Permissions::from_mode(0o700))
        .expect("restrict artifact object root");
    let bytes = b"candidate evidence bound to the durable review question";
    let artifacts = ArtifactStore::open(&artifact_root, 1024)
        .expect("open content-addressed P04 artifact store");
    let artifact = artifacts
        .publish_bytes(bytes)
        .expect("publish immutable candidate evidence");

    let binding = AssetBinding {
        project_id: ProjectId::new("project-recovery").unwrap(),
        root_id: RootId::new("root-recovery").unwrap(),
        task_id: TaskId::new("task-recovery").unwrap(),
        asset_id: "candidate-evidence".into(),
        owner_id: PrincipalId::new("captain").unwrap(),
        sha256: artifact.sha256.clone(),
        generation: "1".into(),
    };
    let observation = |revision: &str| {
        let observed_at = 10 + revision.parse::<u64>().unwrap();
        CustodyObservation {
            binding: binding.clone(),
            asset_revision: revision.into(),
            clock_id: "qualification-clock".into(),
            observed_at: observed_at.to_string(),
            decision_at: (observed_at + 1).to_string(),
            max_age: "10".into(),
            provenance: "fixture:round10-workspace-gate".into(),
            registered: Some(revision != "0"),
            foreign: Some(false),
            dirty: Some(false),
            held: Some(false),
        }
    };
    let database = fixture.path().join("workspace-gate.sqlite3");
    let mut connection = Connection::open(&database).expect("open shared durable journal");
    initialize_questions(&connection).expect("initialize P05 question journal");
    custody::initialize(&connection).expect("initialize P04 custody journal");
    custody::register_asset(
        &mut connection,
        RegisterAssetRequest {
            command_id: "register-candidate-evidence".into(),
            binding: binding.clone(),
            expected_revision: "0".into(),
            observation: observation("0"),
        },
    )
    .expect("bind the exact published artifact digest to scoped custody");
    let pin = custody::acquire_pin(
        &mut connection,
        AcquirePinRequest {
            command_id: "pin-candidate-evidence".into(),
            binding: binding.clone(),
            pin_id: "workspace-review".into(),
            expected_revision: "1".into(),
            observation: observation("1"),
        },
    )
    .expect("hold candidate evidence while its review question is open");
    assert_eq!(pin.revision, "2");
    let pin_id = pin.reference_id.clone().expect("durable custody pin ID");

    let scope = question_scope("workspace-gate-review");
    let open = QuestionEvent::Open {
        event_id: admiralty_core::MessageId::new("gate-open").unwrap(),
        scope: scope.question.clone(),
        authority_ref: scope.authority_ref.clone(),
        asker: PrincipalId::new("captain").unwrap(),
        recipient: PrincipalId::new("worker").unwrap(),
        allowed_recipients: vec![
            PrincipalId::new("worker").unwrap(),
            PrincipalId::new("reviewer").unwrap(),
        ],
        requirement: format!("Audit candidate artifact sha256:{}", artifact.sha256),
        question: "Does this exact candidate evidence satisfy the workspace gate?".into(),
        options: vec!["review".into(), "hold".into()],
        deadline_utc: Some(100),
    };
    append_question(
        &mut connection,
        &scope,
        &admiralty_core::MessageId::new("gate-open-command").unwrap(),
        &open,
    )
    .expect("record scoped audit question");
    let route = QuestionEvent::Route {
        event_id: admiralty_core::MessageId::new("gate-route").unwrap(),
        scope: scope.question.clone(),
        expected_revision: 1,
        authority_ref: scope.authority_ref.clone(),
        actor: PrincipalId::new("worker").unwrap(),
        recipient: PrincipalId::new("reviewer").unwrap(),
    };
    append_question(
        &mut connection,
        &scope,
        &admiralty_core::MessageId::new("gate-route-command").unwrap(),
        &route,
    )
    .expect("route review to an explicitly allowed independent reviewer");

    assert!(matches!(
        custody::claim_retirement(
            &mut connection,
            ClaimRetirementRequest {
                command_id: "retire-before-review".into(),
                binding: binding.clone(),
                claim_id: "premature-retirement".into(),
                expected_revision: "2".into(),
                observation: observation("2"),
            },
        ),
        Err(custody::CustodyError::ActivePins)
    ));
    assert_eq!(
        artifacts.read_verified(&artifact).unwrap(),
        bytes,
        "a held review artifact remains byte-identical while retirement is blocked"
    );

    let forged_acceptance = br#"{"schema_version":1,"receipt_id":"forged","producer":"independent_verifier","verifier":"reviewer","inputs":{"project_id":"project-recovery","root_id":"root-recovery","task_id":"task-recovery","attempt_id":"attempt-1","candidate_ref":"candidate:sha256","contract_revision":1,"contract_digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","verification_policy_digest":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","source_commit_sha":"1111111111111111111111111111111111111111","source_tree_sha":"2222222222222222222222222222222222222222","source_artifact_digest":"sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","protected_oracle_digest":"sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},"checks":[],"negative_checks":[],"acceptance_granted":true}"#;
    assert!(
        parse_receipt(forged_acceptance).is_err(),
        "P15 receipt parsing rejects a self-asserted acceptance field"
    );
    append_question(
        &mut connection,
        &scope,
        &admiralty_core::MessageId::new("gate-answer-command").unwrap(),
        &QuestionEvent::Answer {
            event_id: admiralty_core::MessageId::new("gate-answer").unwrap(),
            scope: scope.question.clone(),
            expected_revision: 2,
            authority_ref: scope.authority_ref.clone(),
            actor: PrincipalId::new("reviewer").unwrap(),
            answer: "hold: forged acceptance evidence is not trusted".into(),
        },
    )
    .expect("reviewer records a scoped answer without granting task acceptance");
    assert_eq!(
        reconstruct_question(&connection, &scope)
            .unwrap()
            .unwrap()
            .state
            .status,
        QuestionStatus::Answered {
            answer: "hold: forged acceptance evidence is not trusted".into()
        }
    );
    custody::release_pin(
        &mut connection,
        ReleasePinRequest {
            command_id: "release-after-review".into(),
            binding: binding.clone(),
            pin_id,
            expected_revision: "2".into(),
            observation: observation("2"),
        },
    )
    .expect("release review pin only after a durable answer");
    custody::claim_retirement(
        &mut connection,
        ClaimRetirementRequest {
            command_id: "retire-after-review".into(),
            binding: binding.clone(),
            claim_id: "retirement-after-review".into(),
            expected_revision: "3".into(),
            observation: observation("3"),
        },
    )
    .expect("retirement claim is available after pin release");
    drop(connection);
    let connection = Connection::open(&database).expect("recover shared journals after restart");
    assert_eq!(
        reconstruct_question(&connection, &scope)
            .unwrap()
            .unwrap()
            .state
            .status,
        QuestionStatus::Answered {
            answer: "hold: forged acceptance evidence is not trusted".into()
        },
        "the durable answer remains after the workspace gate database reopens"
    );
    assert_eq!(
        artifacts.read_verified(&artifact).unwrap(),
        bytes,
        "custody retirement records a claim and never mutates artifact bytes"
    );
}

#[test]
fn p02_p05_p15_round11_item4_final_validation_gate() {
    use admiralty_core::verification::parse_receipt;

    let (contract, _) = fixture();
    let contract_digest = content_digest(&contract).expect("canonical P02 contract digest");
    let contract_json = serde_json::to_vec(&contract).expect("serialize frozen task contract");
    let decoded: TaskContract = serde_json::from_slice(&contract_json)
        .expect("P02 versioned contract round-trips through its public JSON shape");
    assert_eq!(content_digest(&decoded).unwrap(), contract_digest);
    let mut unknown_field: serde_json::Value = serde_json::from_slice(&contract_json).unwrap();
    unknown_field["future_required_semantics"] = serde_json::json!("unknown");
    assert!(
        serde_json::from_value::<TaskContract>(unknown_field).is_err(),
        "closed P02 contract rejects fields with unknown required semantics"
    );

    let directory = tempfile::tempdir().expect("private validation journal directory");
    let database = directory.path().join("validation.sqlite3");
    let mut connection = Connection::open(&database).expect("open durable validation journal");
    initialize_questions(&connection).expect("initialize P05 durable question schema");
    let scope = JournalScope {
        root_id: contract.root_id.clone(),
        question: admiralty_core::coordination::questions::QuestionScope {
            question_id: admiralty_core::MessageId::new("round11-validation-question").unwrap(),
            project_id: contract.project_id.clone(),
            task_id: contract.task_id.clone(),
        },
        authority_ref: contract.authority_ref.clone(),
    };
    let opening = QuestionEvent::Open {
        event_id: admiralty_core::MessageId::new("round11-open-event").unwrap(),
        scope: scope.question.clone(),
        authority_ref: scope.authority_ref.clone(),
        asker: PrincipalId::new("captain").unwrap(),
        recipient: PrincipalId::new("worker").unwrap(),
        allowed_recipients: vec![
            PrincipalId::new("worker").unwrap(),
            PrincipalId::new("reviewer").unwrap(),
        ],
        requirement: format!("Review contract digest {contract_digest}"),
        question: "Does the candidate satisfy the exact contract checks?".into(),
        options: vec!["review".into(), "hold".into()],
        deadline_utc: None,
    };
    let mut foreign_scope = scope.clone();
    foreign_scope.question.task_id = TaskId::new("foreign-task").unwrap();
    assert!(matches!(
        append_question(
            &mut connection,
            &foreign_scope,
            &admiralty_core::MessageId::new("round11-foreign-command").unwrap(),
            &opening,
        ),
        Err(admiralty_store::questions::QuestionStoreError::ScopeMismatch)
    ));
    append_question(
        &mut connection,
        &scope,
        &admiralty_core::MessageId::new("round11-open-command").unwrap(),
        &opening,
    )
    .expect("P05 opens question bound to exact P02 contract scope");
    append_question(
        &mut connection,
        &scope,
        &admiralty_core::MessageId::new("round11-route-command").unwrap(),
        &QuestionEvent::Route {
            event_id: admiralty_core::MessageId::new("round11-route-event").unwrap(),
            scope: scope.question.clone(),
            expected_revision: 1,
            authority_ref: scope.authority_ref.clone(),
            actor: PrincipalId::new("worker").unwrap(),
            recipient: PrincipalId::new("reviewer").unwrap(),
        },
    )
    .expect("current recipient routes only to explicitly allowed reviewer");
    append_question(
        &mut connection,
        &scope,
        &admiralty_core::MessageId::new("round11-answer-command").unwrap(),
        &QuestionEvent::Answer {
            event_id: admiralty_core::MessageId::new("round11-answer-event").unwrap(),
            scope: scope.question.clone(),
            expected_revision: 2,
            authority_ref: scope.authority_ref.clone(),
            actor: PrincipalId::new("reviewer").unwrap(),
            answer: "hold pending independent verification".into(),
        },
    )
    .expect("only routed reviewer records the final scoped question answer");
    drop(connection);
    let connection = Connection::open(&database).expect("reopen durable validation journal");
    let recovered = reconstruct_question(&connection, &scope)
        .unwrap()
        .expect("P05 question state reconstructs after restart");
    assert_eq!(recovered.state.revision, 3);
    assert_eq!(
        recovered.state.status,
        QuestionStatus::Answered {
            answer: "hold pending independent verification".into()
        }
    );

    let forged_receipt = format!(
        r#"{{"schema_version":1,"receipt_id":"round11-forged","producer":"provider_claim","verifier":"reviewer","inputs":{{"project_id":"{}","root_id":"{}","task_id":"{}","attempt_id":"attempt-1","candidate_ref":"candidate:round11","contract_revision":{},"contract_digest":"{}","verification_policy_digest":"sha256:{}","source_commit_sha":"{}","source_tree_sha":"{}","source_artifact_digest":"sha256:{}","protected_oracle_digest":"sha256:{}"}},"checks":[],"negative_checks":[],"acceptance_granted":true}}"#,
        contract.project_id,
        contract.root_id,
        contract.task_id,
        contract.revision,
        contract_digest,
        "b".repeat(64),
        "1".repeat(40),
        "2".repeat(40),
        "c".repeat(64),
        "d".repeat(64),
    );
    assert!(
        parse_receipt(forged_receipt.as_bytes()).is_err(),
        "P15 rejects provider self-claims even when the forged receipt names the exact P02 scope"
    );
}

#[test]
fn p02_p05_p15_round12_item5_redteam_audit_gate() {
    use admiralty_core::verification::{
        CheckResult, ReceiptProducer, RequiredNegativeCheck, VerificationInputs,
        VerificationPolicy, VerificationReceipt, parse_receipt, validate_receipt,
    };

    // P02 rejects both ambiguous duplicate fields and forward fields whose
    // required semantics this reader cannot interpret.
    let (contract, _) = fixture();
    let contract_json = serde_json::to_vec(&contract).expect("serialize P02 contract");
    let contract_text = String::from_utf8(contract_json.clone()).expect("contract JSON is UTF-8");
    let duplicate_version = contract_text.replacen(
        "\"schema_version\":1",
        "\"schema_version\":1,\"schema_version\":1",
        1,
    );
    assert_ne!(
        duplicate_version, contract_text,
        "fixture contains schema version"
    );
    assert!(serde_json::from_str::<TaskContract>(&duplicate_version).is_err());
    let mut future_contract: serde_json::Value = serde_json::from_slice(&contract_json).unwrap();
    future_contract["unreviewed_authority_override"] = serde_json::json!(true);
    assert!(serde_json::from_value::<TaskContract>(future_contract).is_err());

    // A well-shaped portable receipt parses, but its digest/evidence claims
    // never confer authority and malformed or injected trust fields fail shut.
    let protected_oracle_digest = format!("sha256:{}", "d".repeat(64));
    let policy = VerificationPolicy {
        schema_version: 1,
        verifier: PrincipalId::new("reviewer").unwrap(),
        protected_oracle_digest: protected_oracle_digest.clone(),
        required_negative_checks: vec![RequiredNegativeCheck {
            check: CheckSpec {
                id: "negative".into(),
                program: "fixture-negative".into(),
                args: vec![],
                cwd: ".".into(),
                timeout_secs: 1,
            },
            expected_exit_code: 1,
            fixture_digest: format!("sha256:{}", "9".repeat(64)),
        }],
    };
    let inputs = VerificationInputs {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new("attempt-1").unwrap(),
        candidate_ref: "candidate:redteam".into(),
        contract_revision: contract.revision,
        contract_digest: content_digest(&contract).unwrap(),
        verification_policy_digest: content_digest(&policy).unwrap(),
        source_commit_sha: "1".repeat(40),
        source_tree_sha: "2".repeat(40),
        source_artifact_digest: format!("sha256:{}", "c".repeat(64)),
        protected_oracle_digest,
    };
    let receipt = VerificationReceipt {
        schema_version: 1,
        receipt_id: "redteam-receipt".into(),
        producer: ReceiptProducer::IndependentVerifier,
        verifier: PrincipalId::new("reviewer").unwrap(),
        inputs: inputs.clone(),
        checks: contract
            .checks
            .iter()
            .cloned()
            .map(|check| CheckResult {
                check,
                exit_code: Some(0),
                stdout_digest: format!("sha256:{}", "e".repeat(64)),
                stderr_digest: format!("sha256:{}", "f".repeat(64)),
            })
            .collect(),
        negative_checks: policy
            .required_negative_checks
            .iter()
            .map(|negative| CheckResult {
                check: negative.check.clone(),
                exit_code: Some(1),
                stdout_digest: format!("sha256:{}", "1".repeat(64)),
                stderr_digest: format!("sha256:{}", "2".repeat(64)),
            })
            .collect(),
    };
    let receipt_bytes =
        serde_json::to_vec(&receipt).expect("serialize deterministic receipt fixture");
    assert!(
        parse_receipt(&receipt_bytes).is_ok(),
        "well-shaped receipt fixture is parseable"
    );
    let validated = validate_receipt(&contract, &inputs, &policy, &receipt)
        .expect("independent receipt matches exact pinned contract and policy");
    assert!(validated.all_checks_passed());
    assert!(!validated.acceptance_granted());
    assert!(!validated.verifier_authenticated());

    let mut provider_claim = receipt.clone();
    provider_claim.producer = ReceiptProducer::ProviderClaim;
    assert!(parse_receipt(&serde_json::to_vec(&provider_claim).unwrap()).is_ok());
    assert!(validate_receipt(&contract, &inputs, &policy, &provider_claim).is_err());
    let mut injected_trust = serde_json::to_value(&receipt).unwrap();
    injected_trust["acceptance_granted"] = serde_json::json!(true);
    assert!(parse_receipt(&serde_json::to_vec(&injected_trust).unwrap()).is_err());
    let mut malformed_digest = serde_json::to_value(&receipt).unwrap();
    malformed_digest["inputs"]["source_artifact_digest"] = serde_json::json!("sha256:short");
    assert!(parse_receipt(&serde_json::to_vec(&malformed_digest).unwrap()).is_err());
    let receipt_text = String::from_utf8(receipt_bytes).unwrap();
    let duplicate_receipt_id = receipt_text.replacen(
        "\"receipt_id\":\"redteam-receipt\"",
        "\"receipt_id\":\"redteam-receipt\",\"receipt_id\":\"attacker\"",
        1,
    );
    assert_ne!(duplicate_receipt_id, receipt_text);
    assert!(parse_receipt(duplicate_receipt_id.as_bytes()).is_err());

    // P05 reconstructs only from integrity-bound events and projection. A
    // valid opening followed by payload tampering must not yield trusted state.
    let directory = tempfile::tempdir().expect("private red-team journal directory");
    let mut connection =
        Connection::open(directory.path().join("questions.sqlite3")).expect("open P05 journal");
    initialize_questions(&connection).expect("initialize P05 journal");
    let question_scope = admiralty_core::coordination::questions::QuestionScope {
        question_id: admiralty_core::MessageId::new("round12-redteam-question").unwrap(),
        project_id: contract.project_id.clone(),
        task_id: contract.task_id.clone(),
    };
    let scope = JournalScope {
        root_id: contract.root_id.clone(),
        question: question_scope.clone(),
        authority_ref: contract.authority_ref.clone(),
    };
    let opening = QuestionEvent::Open {
        event_id: admiralty_core::MessageId::new("round12-redteam-open-event").unwrap(),
        scope: question_scope,
        authority_ref: scope.authority_ref.clone(),
        asker: PrincipalId::new("captain").unwrap(),
        recipient: PrincipalId::new("worker").unwrap(),
        allowed_recipients: vec![
            PrincipalId::new("worker").unwrap(),
            PrincipalId::new("reviewer").unwrap(),
        ],
        requirement: "Verify P02 contract and P15 evidence independently".into(),
        question: "Does the evidence satisfy the pinned contract?".into(),
        options: vec!["hold".into(), "review".into()],
        deadline_utc: None,
    };
    append_question(
        &mut connection,
        &scope,
        &admiralty_core::MessageId::new("round12-redteam-open-command").unwrap(),
        &opening,
    )
    .expect("append valid scoped opening");
    connection
        .execute(
            "UPDATE durable_question_events SET event_json='{}' WHERE question_id=?1",
            [scope.question.question_id.as_str()],
        )
        .expect("tamper durable P05 event payload");
    assert!(reconstruct_question(&connection, &scope).is_err());
}

#[test]
fn p02_p05_p15_round13_wo_intake_multi_paragraph_preview() {
    use admiralty_core::verification::{
        CheckResult, ReceiptProducer, RequiredNegativeCheck, VerificationInputs,
        VerificationPolicy, VerificationReceipt, parse_receipt, validate_receipt,
    };
    use admiralty_core::workplans::{Workplan, preview_workplan, preview_workplan_with_existing};

    let (mut contract, _) = fixture();
    contract.objective = "Preserve the original request as one work order.\n\nKeep each paragraph intact; previewing must not split it into tasks.".into();
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round13-work-order".into(),
        revision: 1,
        project_id: contract.project_id.clone(),
        tasks: vec![contract.clone()],
    };
    let source = serde_json::to_vec(&plan).expect("serialize immutable work-order preview input");
    let preview = preview_workplan(&source).expect("P02 bounded multi-paragraph preview");
    let replay = preview_workplan_with_existing(&source, &preview.task_digests)
        .expect("exact work-order preview replay");
    assert_eq!(
        preview, replay,
        "preview and source identity are deterministic"
    );
    assert_eq!(preview.task_order, [contract.task_id.clone()]);
    assert_eq!(preview.source_digest, replay.source_digest);
    assert_eq!(
        preview.task_digests[&contract.task_id],
        content_digest(&contract).unwrap()
    );
    let restored: Workplan = serde_json::from_slice(&source).unwrap();
    assert_eq!(
        restored.tasks.len(),
        1,
        "paragraph boundaries do not multiply work orders"
    );
    assert_eq!(restored.tasks[0].objective, contract.objective);
    let mut forged_source: serde_json::Value = serde_json::from_slice(&source).unwrap();
    forged_source["tasks"][0]["accepted"] = serde_json::json!(true);
    assert!(preview_workplan(&serde_json::to_vec(&forged_source).unwrap()).is_err());

    // P05 retains request review under exact work-order scope and recovers it
    // from its durable event log after a restart.
    let directory = tempfile::tempdir().expect("private work-order journal");
    let database = directory.path().join("work-order.sqlite3");
    let mut connection = Connection::open(&database).expect("open journal");
    initialize_questions(&connection).expect("initialize journal");
    let question_scope = admiralty_core::coordination::questions::QuestionScope {
        question_id: admiralty_core::MessageId::new("round13-work-order-review").unwrap(),
        project_id: contract.project_id.clone(),
        task_id: contract.task_id.clone(),
    };
    let journal_scope = JournalScope {
        root_id: contract.root_id.clone(),
        question: question_scope.clone(),
        authority_ref: contract.authority_ref.clone(),
    };
    let requirement = format!(
        "Review work-order digest {}",
        preview.task_digests[&contract.task_id]
    );
    let open = QuestionEvent::Open {
        event_id: admiralty_core::MessageId::new("round13-open-event").unwrap(),
        scope: question_scope.clone(),
        authority_ref: journal_scope.authority_ref.clone(),
        asker: PrincipalId::new("captain").unwrap(),
        recipient: PrincipalId::new("worker").unwrap(),
        allowed_recipients: vec![
            PrincipalId::new("worker").unwrap(),
            PrincipalId::new("reviewer").unwrap(),
        ],
        requirement: requirement.clone(),
        question: contract.objective.clone(),
        options: vec!["hold".into(), "review".into()],
        deadline_utc: None,
    };
    append_question(
        &mut connection,
        &journal_scope,
        &admiralty_core::MessageId::new("round13-open-command").unwrap(),
        &open,
    )
    .expect("record question in exact work-order scope");
    append_question(
        &mut connection,
        &journal_scope,
        &admiralty_core::MessageId::new("round13-route-command").unwrap(),
        &QuestionEvent::Route {
            event_id: admiralty_core::MessageId::new("round13-route-event").unwrap(),
            scope: question_scope.clone(),
            expected_revision: 1,
            authority_ref: journal_scope.authority_ref.clone(),
            actor: PrincipalId::new("worker").unwrap(),
            recipient: PrincipalId::new("reviewer").unwrap(),
        },
    )
    .expect("route only to allowed reviewer");
    drop(connection);
    let connection = Connection::open(&database).expect("reopen work-order journal");
    let recovered = reconstruct_question(&connection, &journal_scope)
        .unwrap()
        .expect("reconstruct durable review");
    assert_eq!(recovered.state.revision, 2);
    assert_eq!(recovered.state.requirement, requirement);
    assert_eq!(recovered.state.question, contract.objective);

    // P15 binds release evidence to the previewed immutable task and pinned
    // verifier policy, while a valid receipt remains non-authorizing evidence.
    let oracle_digest = format!("sha256:{}", "d".repeat(64));
    let policy = VerificationPolicy {
        schema_version: 1,
        verifier: PrincipalId::new("reviewer").unwrap(),
        protected_oracle_digest: oracle_digest.clone(),
        required_negative_checks: vec![RequiredNegativeCheck {
            check: CheckSpec {
                id: "negative".into(),
                program: "fixture-negative".into(),
                args: vec![],
                cwd: ".".into(),
                timeout_secs: 1,
            },
            expected_exit_code: 1,
            fixture_digest: format!("sha256:{}", "9".repeat(64)),
        }],
    };
    let inputs = VerificationInputs {
        project_id: contract.project_id.clone(),
        root_id: contract.root_id.clone(),
        task_id: contract.task_id.clone(),
        attempt_id: AttemptId::new("round13-attempt").unwrap(),
        candidate_ref: "candidate:round13".into(),
        contract_revision: contract.revision,
        contract_digest: preview.task_digests[&contract.task_id].clone(),
        verification_policy_digest: content_digest(&policy).unwrap(),
        source_commit_sha: "1".repeat(40),
        source_tree_sha: "2".repeat(40),
        source_artifact_digest: format!("sha256:{}", "c".repeat(64)),
        protected_oracle_digest: oracle_digest,
    };
    let evidence = VerificationReceipt {
        schema_version: 1,
        receipt_id: "round13-release-evidence".into(),
        producer: ReceiptProducer::IndependentVerifier,
        verifier: PrincipalId::new("reviewer").unwrap(),
        inputs: inputs.clone(),
        checks: contract
            .checks
            .iter()
            .cloned()
            .map(|check| CheckResult {
                check,
                exit_code: Some(0),
                stdout_digest: format!("sha256:{}", "e".repeat(64)),
                stderr_digest: format!("sha256:{}", "f".repeat(64)),
            })
            .collect(),
        negative_checks: policy
            .required_negative_checks
            .iter()
            .map(|negative| CheckResult {
                check: negative.check.clone(),
                exit_code: Some(1),
                stdout_digest: format!("sha256:{}", "1".repeat(64)),
                stderr_digest: format!("sha256:{}", "2".repeat(64)),
            })
            .collect(),
    };
    assert!(parse_receipt(&serde_json::to_vec(&evidence).unwrap()).is_ok());
    let validated = validate_receipt(&contract, &inputs, &policy, &evidence)
        .expect("receipt matches exact work-order and release policy");
    assert!(validated.all_checks_passed());
    assert!(!validated.acceptance_granted());
    assert!(!validated.execution_qualified());
    assert!(!validated.evidence_custody_verified());
    let mut forged_release_claim = serde_json::to_value(evidence).unwrap();
    forged_release_claim["release_granted"] = serde_json::json!(true);
    assert!(parse_receipt(&serde_json::to_vec(&forged_release_claim).unwrap()).is_err());
}
