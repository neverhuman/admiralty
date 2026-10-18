#[test]
fn p03_p09_p16_round12_item5_redteam_audit_gate() {
    use admiralty_core::{
        AcceptanceCriterion, CheckSpec, DomainEvent, ProjectId, RootId, SourceIdentity,
        TaskContract, TaskId,
    };
    use admiralty_runtime::workspace::registry::{Custody, NodeRef, WorkspaceSpec};
    use admiralty_store::{Store, outbox::OutboxIntent};
    use serde_json::json;
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    let temp = tempfile::tempdir().expect("isolated work-order fixture");
    let workspace = fs::canonicalize(temp.path()).expect("canonical workspace");
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700))
        .expect("private workspace permissions");
    fs::create_dir(workspace.join("src")).expect("source directory");
    fs::create_dir(workspace.join(".git")).expect("metadata directory");
    fs::write(workspace.join("src/lib.rs"), "// work order fixture\n").expect("source file");
    fs::write(workspace.join(".git/config"), "[core]\n").expect("metadata file");

    // P03: a work order must carry bounded ownership, an explicit outcome, named checks,
    // and a criterion linked to those checks before it can become a submitted task.
    let contract = TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round13-project").expect("project ID"),
        root_id: RootId::new("round13-root").expect("root ID"),
        task_id: TaskId::new("round13-task").expect("task ID"),
        revision: 1,
        title: "Prepare release evidence".into(),
        objective: "Produce scoped, independently reviewable evidence".into(),
        non_goals: vec!["Do not claim release or execute publication".into()],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![".git".into()],
        read_first: vec!["README.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "evidence-bound".into(),
            text: "Evidence remains tied to the task scope and named check".into(),
            check_ids: vec!["release-audit".into()],
        }],
        checks: vec![CheckSpec {
            id: "release-audit".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 45,
        }],
        authority_ref: "fixture:round13-work-order".into(),
        max_attempts: 1,
        simulated: false,
    };
    contract.validate().expect("complete work order validates");

    let mut unsafe_scope = contract.clone();
    unsafe_scope.owned_paths = vec!["../secrets".into()];
    assert!(
        unsafe_scope.validate().is_err(),
        "work order cannot escape its repo"
    );
    let mut unbound_criterion = contract.clone();
    unbound_criterion.criteria[0].check_ids = vec!["missing-check".into()];
    assert!(
        unbound_criterion.validate().is_err(),
        "acceptance criteria must reference a declared check"
    );

    // P09: registration records bounded custody and does not adopt Git metadata.
    let registered = WorkspaceSpec {
        node_id: NodeRef::new("round13-node").expect("node ID"),
        root_id: contract.root_id.clone(),
        project_id: contract.project_id.clone(),
        workspace: workspace.clone(),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    }
    .validate()
    .expect("scoped workspace validates");
    assert_eq!(registered.custody, Custody::RegisteredOnly);
    assert!(registered.owns(std::path::Path::new("src/lib.rs")));
    assert!(!registered.owns(std::path::Path::new(".git/config")));

    // P16: release-shaped JSON cannot promote its own status into authenticated evidence.
    for forged in [
        br#"{"release_granted":true}"#.as_slice(),
        br#"{"accepted":true,"execution_granted":true}"#.as_slice(),
        br#"{"work_order_complete":true,"release_status":"released"}"#.as_slice(),
    ] {
        assert!(
            admiralty_runtime::audits::decode_evidence(forged).is_err(),
            "release decisions must come from validated, scoped audit evidence"
        );
    }

    // P03 durability: exact replay is idempotent; changed evidence intent cannot rewrite it.
    let event = DomainEvent::TaskSubmitted { contract };
    let source = SourceIdentity {
        producer_id: "round13-captain".into(),
        incarnation: "fixture-1".into(),
        sequence: "1".into(),
    };
    let intent = OutboxIntent {
        idempotency_key: "round13-release-evidence".into(),
        payload: json!({"operation":"record-evidence-reference","task":"round13-task"}),
        not_before: 13,
    };
    let store = Store::open(temp.path().join("work-order.sqlite")).expect("open durable store");
    let first = store
        .append_with_intent(
            source.clone(),
            event.clone(),
            "2026-10-18T00:00:00Z".into(),
            intent.clone(),
        )
        .expect("atomic work-order and evidence intent");
    assert!(!first.duplicate);
    let replay = store
        .append_with_intent(
            source.clone(),
            event.clone(),
            "2026-10-18T00:00:00Z".into(),
            intent,
        )
        .expect("exact work-order replay");
    assert!(replay.duplicate);
    assert_eq!(replay.event_seq, first.event_seq);

    let changed = OutboxIntent {
        idempotency_key: "round13-release-evidence".into(),
        payload: json!({"operation":"claim-release"}),
        not_before: 13,
    };
    assert!(
        store
            .append_with_intent(source, event, "2026-10-18T00:00:00Z".into(), changed,)
            .is_err()
    );
    let snapshot = store.snapshot().expect("durable work-order projection");
    assert_eq!(snapshot.cursor.event_seq, first.event_seq);
    assert_eq!(snapshot.projection.tasks.len(), 1);
}
