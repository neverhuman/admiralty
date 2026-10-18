#![allow(clippy::unwrap_used, clippy::expect_used)]
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

fn make_contract() -> Result<TaskContract, Box<dyn Error>> {
    Ok(TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round21-project")?,
        root_id: RootId::new("round21-root")?,
        task_id: TaskId::new("round21-task")?,
        revision: 1,
        title: "Hermetic release qualification".into(),
        objective: "Keep release evidence bound to one portable workspace scope".into(),
        non_goals: vec!["Do not grant release, acceptance, or execution".into()],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![".git".into()],
        read_first: vec!["README.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "portable-check".into(),
            text: "The named check remains part of the portable work order".into(),
            check_ids: vec!["ci".into()],
        }],
        checks: vec![CheckSpec {
            id: "ci".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "fixture:round21-hermetic-ci".into(),
        max_attempts: 1,
        simulated: false,
    })
}

fn launch(workspace: &std::path::Path, id: &str, command: &str) -> LaunchSpec {
    LaunchSpec {
        run_id: id.into(),
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".into(), command.into()],
        cwd: workspace.to_path_buf(),
        spool_dir: workspace.join(format!("session-{id}")),
    }
}

#[test]
fn p03_p09_p16_round21_hermetic_ci_cross_packet_release_qualification() -> Result<(), Box<dyn Error>>
{
    // All mutable fixtures are rooted in a private temporary directory; no host path is assumed.
    let temporary = tempfile::tempdir()?;
    let workspace = fs::canonicalize(temporary.path())?;
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700))?;
    fs::create_dir(workspace.join("src"))?;
    fs::create_dir(workspace.join(".git"))?;
    fs::write(workspace.join("src/lib.rs"), "// hermetic fixture\n")?;

    // P03/P09: serialized task scope and registered workspace retain matching identities.
    let contract = make_contract()?;
    contract.validate()?;
    let encoded = serde_json::to_vec(&contract)?;
    let decoded: TaskContract = serde_json::from_slice(&encoded)?;
    assert_eq!(decoded, contract);
    let registration = WorkspaceSpec {
        node_id: NodeRef::new("round21-node")?,
        root_id: decoded.root_id.clone(),
        project_id: decoded.project_id.clone(),
        workspace: workspace.clone(),
        owned_paths: decoded.owned_paths.iter().map(PathBuf::from).collect(),
        forbidden_paths: decoded.forbidden_paths.iter().map(PathBuf::from).collect(),
    };
    let validated = registration.validate()?;
    assert_eq!(validated.root_id, decoded.root_id);
    assert_eq!(validated.project_id, decoded.project_id);
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(std::path::Path::new("src/lib.rs")));
    assert!(!validated.owns(std::path::Path::new(".git/config")));
    assert!(!validated.owns(std::path::Path::new("../escape")));
    let mut foreign_scope = registration.clone();
    foreign_scope.project_id = ProjectId::new("round21-foreign-project")?;
    assert_ne!(foreign_scope.validate()?.project_id, decoded.project_id);

    // Replay of the release event is stable; changed evidence cannot replace it.
    let event = DomainEvent::TaskSubmitted { contract: decoded };
    let source = SourceIdentity {
        producer_id: "round21-ci-auditor".into(),
        incarnation: "portable-run-1".into(),
        sequence: "1".into(),
    };
    let intent = OutboxIntent {
        idempotency_key: "round21-release-check".into(),
        payload: json!({"task":"round21-task","workspace":validated.node_id.as_str()}),
        not_before: 21,
    };
    let store = Store::open(workspace.join("release.sqlite"))?;
    let first = store.append_with_intent(
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
    assert!(!first.duplicate);
    assert!(replay.duplicate);
    assert_eq!(first.event_seq, replay.event_seq);
    assert!(
        store
            .append_with_intent(
                source,
                event,
                "2026-10-18T00:00:00Z".into(),
                OutboxIntent {
                    idempotency_key: "round21-release-check".into(),
                    payload: json!({"release_granted":true}),
                    not_before: 21,
                },
            )
            .is_err()
    );
    assert_eq!(store.snapshot()?.projection.tasks.len(), 1);

    // P16 session failure is contained and cannot change a healthy peer's capture.
    let host = SessionHost::new();
    host.launch(launch(&workspace, "healthy", "printf portable"))?;
    host.launch(launch(&workspace, "failed", "printf isolated; exit 13"))?;
    assert!(matches!(
        host.launch(launch(&workspace, "healthy", "printf overwrite")),
        Err(SessionError::DuplicateRun(_))
    ));
    assert!(host.wait("healthy")?.success());
    assert!(!host.wait("failed")?.success());
    assert_eq!(
        host.status("healthy")?.capture_state,
        CaptureState::Complete
    );
    assert_eq!(host.status("failed")?.capture_state, CaptureState::Complete);
    let healthy: Vec<u8> = host
        .tail("healthy", 0, 1024)?
        .chunks
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    let failed: Vec<u8> = host
        .tail("failed", 0, 1024)?
        .chunks
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    assert_eq!(healthy, b"portable");
    assert_eq!(failed, b"isolated");
    assert!(matches!(
        host.status("unknown"),
        Err(SessionError::UnknownRun(_))
    ));
    Ok(())
}
