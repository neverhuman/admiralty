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
