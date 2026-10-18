#[test]
fn p03_p09_p16_round11_item4_final_validation_gate() {
    use admiralty_core::{
        AcceptanceCriterion, CheckSpec, DomainEvent, ProjectId, RootId, SourceIdentity,
        TaskContract, TaskId,
    };
    use admiralty_runtime::workspace::registry::{Custody, NodeRef, WorkspaceSpec};
    use admiralty_store::{Store, outbox::OutboxIntent};
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::{fs, path::PathBuf};

    let temp = tempfile::tempdir().expect("isolated validation workspace");
    let workspace = fs::canonicalize(temp.path()).expect("canonical validation workspace");
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700))
        .expect("private validation workspace permissions");
    fs::create_dir(workspace.join("src")).expect("source directory");
    fs::write(workspace.join("src/lib.rs"), "// deterministic fixture\n").expect("source fixture");

    // P09 scope is exact, read-only registration; excluded metadata stays outside it.
    let validated = WorkspaceSpec {
        node_id: NodeRef::new("round11-node").expect("node ID"),
        root_id: RootId::new("round11-root").expect("root ID"),
        project_id: ProjectId::new("round11-project").expect("project ID"),
        workspace: workspace.clone(),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    }
    .validate()
    .expect("workspace validates");
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(std::path::Path::new("src/lib.rs")));
    assert!(!validated.owns(std::path::Path::new(".git/config")));

    // P16 rejects audit payloads attempting to declare their own acceptance.
    assert!(
        admiralty_runtime::audits::decode_evidence(
            br#"{"acceptance_granted":true,"execution_granted":true}"#
        )
        .is_err()
    );

    // P03 stores the domain event and its immutable effect intent atomically.
    let contract = TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round11-project").expect("project ID"),
        root_id: RootId::new("round11-root").expect("root ID"),
        task_id: TaskId::new("round11-task").expect("task ID"),
        revision: 1,
        title: "Round 11 validation".into(),
        objective: "Preserve exact scoped event and intent identity".into(),
        non_goals: vec!["Do not execute the effect".into()],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![".git".into()],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "scope-preserved".into(),
            text: "Workspace scope remains bounded".into(),
            check_ids: vec!["unit".into()],
        }],
        checks: vec![CheckSpec {
            id: "unit".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "fixture:round11".into(),
        max_attempts: 1,
        simulated: false,
    };
    let event = DomainEvent::TaskSubmitted { contract };
    let intent = OutboxIntent {
        idempotency_key: "round11-effect".into(),
        payload: json!({"operation":"record-only","task":"round11-task"}),
        not_before: 10,
    };
    let store = Store::open(temp.path().join("events.sqlite")).expect("open durable store");
    let source = SourceIdentity {
        producer_id: "round11-captain".into(),
        incarnation: "fixture-1".into(),
        sequence: "1".into(),
    };
    let first = store
        .append_with_intent(
            source.clone(),
            event.clone(),
            "2026-01-01T00:00:00Z".into(),
            intent.clone(),
        )
        .expect("atomic event and intent append");
    assert_eq!(first.event_seq, "1");
    assert!(!first.duplicate);
    let replay = store
        .append_with_intent(
            source.clone(),
            event.clone(),
            "2026-01-01T00:00:00Z".into(),
            intent,
        )
        .expect("exact source and effect replay");
    assert!(replay.duplicate);
    assert_eq!(replay.event_seq, first.event_seq);
    assert_eq!(store.events_after(0, 10).expect("durable events").len(), 1);

    let changed_intent = OutboxIntent {
        idempotency_key: "round11-effect".into(),
        payload: json!({"operation":"changed"}),
        not_before: 10,
    };
    assert!(
        store
            .append_with_intent(source, event, "2026-01-01T00:00:00Z".into(), changed_intent)
            .is_err()
    );
    let snapshot = store.snapshot().expect("stable post-collision projection");
    assert_eq!(snapshot.cursor.event_seq, "1");
    assert_eq!(snapshot.projection.tasks.len(), 1);
}
