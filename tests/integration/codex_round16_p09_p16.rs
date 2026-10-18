#![cfg(unix)]

use admiralty_core::{
    AcceptanceCriterion, CheckSpec, DomainEvent, ProjectId, RootId, SourceIdentity, TaskContract,
    TaskId,
};
use admiralty_runtime::{
    session::{CaptureState, LaunchSpec, SessionError, SessionHost},
    workspace::registry::{Custody, NodeRef, WorkspaceSpec},
};
use admiralty_store::{Store, outbox::OutboxIntent};
use serde_json::json;
use std::{error::Error, fs, os::unix::fs::PermissionsExt, path::PathBuf};

fn task_contract() -> Result<TaskContract, Box<dyn Error>> {
    Ok(TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round16-project")?,
        root_id: RootId::new("round16-root")?,
        task_id: TaskId::new("round16-task")?,
        revision: 1,
        title: "Cut over deterministic release smoke".into(),
        objective: "Record scoped work and contain session faults".into(),
        non_goals: vec!["Do not grant release or execution authority".into()],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![".git".into()],
        read_first: vec!["README.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "smoke-evidence".into(),
            text: "The deterministic smoke check completes within this scope".into(),
            check_ids: vec!["smoke".into()],
        }],
        checks: vec![CheckSpec {
            id: "smoke".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "fixture:round16-cutover".into(),
        max_attempts: 1,
        simulated: false,
    })
}

fn launch(root: &std::path::Path, run_id: &str, command: &str) -> LaunchSpec {
    LaunchSpec {
        run_id: run_id.into(),
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".into(), command.into()],
        cwd: root.to_path_buf(),
        spool_dir: root.join(format!("spool-{run_id}")),
    }
}

#[test]
fn p03_p09_p16_round16_end_to_end_cutover_fault_containment() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let workspace = fs::canonicalize(root.path())?;
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700))?;
    fs::create_dir(workspace.join("src"))?;
    fs::create_dir(workspace.join(".git"))?;
    fs::write(workspace.join("src/lib.rs"), "// cutover fixture\n")?;

    // P03: exact cutover replay is idempotent; altered intent cannot replace evidence.
    let contract = task_contract()?;
    contract.validate()?;
    let mut invalid = contract.clone();
    invalid.owned_paths = vec!["../outside".into()];
    assert!(invalid.validate().is_err());
    let event = DomainEvent::TaskSubmitted { contract };
    let source = SourceIdentity {
        producer_id: "round16-captain".into(),
        incarnation: "cutover-1".into(),
        sequence: "1".into(),
    };
    let intent = OutboxIntent {
        idempotency_key: "round16-release-smoke".into(),
        payload: json!({"operation":"record-smoke-evidence","task":"round16-task"}),
        not_before: 16,
    };
    let store = Store::open(workspace.join("cutover.sqlite"))?;
    let committed = store.append_with_intent(
        source.clone(),
        event.clone(),
        "2026-10-18T00:00:00Z".into(),
        intent.clone(),
    )?;
    let replay = store.append_with_intent(
        source.clone(),
        event.clone(),
        "2026-10-18T00:00:00Z".into(),
        intent,
    )?;
    assert!(!committed.duplicate);
    assert!(replay.duplicate);
    assert_eq!(replay.event_seq, committed.event_seq);
    let changed_intent = OutboxIntent {
        idempotency_key: "round16-release-smoke".into(),
        payload: json!({"operation":"grant-release"}),
        not_before: 16,
    };
    assert!(
        store
            .append_with_intent(source, event, "2026-10-18T00:00:00Z".into(), changed_intent,)
            .is_err()
    );
    let snapshot = store.snapshot()?;
    assert_eq!(snapshot.cursor.event_seq, committed.event_seq);
    assert_eq!(snapshot.projection.tasks.len(), 1);

    // P09: valid custody stays registered-only; traversal and symlink roots fail closed.
    let registration = WorkspaceSpec {
        node_id: NodeRef::new("round16-node")?,
        root_id: RootId::new("round16-root")?,
        project_id: ProjectId::new("round16-project")?,
        workspace: workspace.clone(),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    };
    let validated = registration.validate()?;
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(std::path::Path::new("src/lib.rs")));
    assert!(!validated.owns(std::path::Path::new(".git/config")));
    assert!(!validated.owns(std::path::Path::new("../outside")));
    let mut traversal = registration.clone();
    traversal.owned_paths = vec![PathBuf::from("../outside")];
    assert!(traversal.validate().is_err());
    let linked_root = workspace.join("linked-root");
    std::os::unix::fs::symlink(&workspace, &linked_root)?;
    let mut symlink_root = registration;
    symlink_root.workspace = linked_root;
    assert!(symlink_root.validate().is_err());

    // P16: failure in one child is recorded as failure and cannot contaminate its peer.
    let host = SessionHost::new();
    host.launch(launch(&workspace, "healthy", "printf healthy"))?;
    host.launch(launch(&workspace, "faulted", "printf fault; exit 23"))?;
    assert!(matches!(
        host.launch(launch(&workspace, "healthy", "printf replacement")),
        Err(SessionError::DuplicateRun(_))
    ));
    let healthy_exit = host.wait("healthy")?;
    let faulted_exit = host.wait("faulted")?;
    assert!(healthy_exit.success());
    assert!(!faulted_exit.success());
    let healthy = host.status("healthy")?;
    let faulted = host.status("faulted")?;
    assert_eq!(healthy.capture_state, CaptureState::Complete);
    assert_eq!(faulted.capture_state, CaptureState::Complete);
    let healthy_bytes: Vec<u8> = host
        .tail("healthy", 0, 1024)?
        .chunks
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    let faulted_bytes: Vec<u8> = host
        .tail("faulted", 0, 1024)?
        .chunks
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    assert_eq!(healthy_bytes, b"healthy");
    assert_eq!(faulted_bytes, b"fault");
    assert!(matches!(
        host.status("missing"),
        Err(SessionError::UnknownRun(_))
    ));
    Ok(())
}
