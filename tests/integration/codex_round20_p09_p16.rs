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

fn contract() -> Result<TaskContract, Box<dyn Error>> {
    Ok(TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round20-project")?,
        root_id: RootId::new("round20-root")?,
        task_id: TaskId::new("round20-task")?,
        revision: 1,
        title: "Post-PR-57 release qualification".into(),
        objective: "Bind release evidence to one work order and isolated workspace".into(),
        non_goals: vec!["Do not grant release, acceptance, or execution".into()],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![".git".into()],
        read_first: vec!["README.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "release-check".into(),
            text: "Named release check evidence remains replayable and scoped".into(),
            check_ids: vec!["qualification".into()],
        }],
        checks: vec![CheckSpec {
            id: "qualification".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "fixture:round20-post-pr57".into(),
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
fn p03_p09_p16_round20_post_pr57_release_qualification() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let workspace = fs::canonicalize(temporary.path())?;
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700))?;
    fs::create_dir(workspace.join("src"))?;
    fs::create_dir(workspace.join(".git"))?;
    fs::write(workspace.join("src/lib.rs"), "// qualification fixture\n")?;

    // P03: reject unbounded task scope and preserve exact append/replay identity.
    let work_order = contract()?;
    work_order.validate()?;
    let mut escaped = work_order.clone();
    escaped.owned_paths = vec!["../outside".into()];
    assert!(escaped.validate().is_err());
    let event = DomainEvent::TaskSubmitted {
        contract: work_order.clone(),
    };
    let source = SourceIdentity {
        producer_id: "round20-auditor".into(),
        incarnation: "post-pr57".into(),
        sequence: "1".into(),
    };
    let intent = OutboxIntent {
        idempotency_key: "round20-release-evidence".into(),
        payload: json!({"task":"round20-task","check":"qualification"}),
        not_before: 20,
    };
    let store = Store::open(workspace.join("qualification.sqlite"))?;
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
    let altered = OutboxIntent {
        idempotency_key: "round20-release-evidence".into(),
        payload: json!({"release_granted":true}),
        not_before: 20,
    };
    assert!(
        store
            .append_with_intent(source, event, "2026-10-18T00:00:00Z".into(), altered,)
            .is_err()
    );
    assert_eq!(store.snapshot()?.projection.tasks.len(), 1);

    // P09: custody and path ownership stay pinned to the work-order identity.
    let registration = WorkspaceSpec {
        node_id: NodeRef::new("round20-node")?,
        root_id: work_order.root_id.clone(),
        project_id: work_order.project_id.clone(),
        workspace: workspace.clone(),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    };
    let validated = registration.validate()?;
    assert_eq!(validated.root_id, work_order.root_id);
    assert_eq!(validated.project_id, work_order.project_id);
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(std::path::Path::new("src/lib.rs")));
    assert!(!validated.owns(std::path::Path::new(".git/config")));
    assert!(!validated.owns(std::path::Path::new("../outside")));
    let link = workspace.join("workspace-link");
    std::os::unix::fs::symlink(&workspace, &link)?;
    let mut symlink_registration = registration;
    symlink_registration.workspace = link;
    assert!(symlink_registration.validate().is_err());

    // P16: one failed child cannot change its peer's result or output journal.
    let host = SessionHost::new();
    host.launch(launch(&workspace, "qualified", "printf qualified"))?;
    host.launch(launch(
        &workspace,
        "contained-fault",
        "printf fault; exit 17",
    ))?;
    assert!(matches!(
        host.launch(launch(&workspace, "qualified", "printf replacement")),
        Err(SessionError::DuplicateRun(_))
    ));
    assert!(host.wait("qualified")?.success());
    assert!(!host.wait("contained-fault")?.success());
    assert_eq!(
        host.status("qualified")?.capture_state,
        CaptureState::Complete
    );
    assert_eq!(
        host.status("contained-fault")?.capture_state,
        CaptureState::Complete
    );
    let qualified: Vec<u8> = host
        .tail("qualified", 0, 1024)?
        .chunks
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    let faulted: Vec<u8> = host
        .tail("contained-fault", 0, 1024)?
        .chunks
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    assert_eq!(qualified, b"qualified");
    assert_eq!(faulted, b"fault");
    assert!(matches!(
        host.status("unregistered"),
        Err(SessionError::UnknownRun(_))
    ));
    Ok(())
}
