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
