#![cfg(unix)]

use admiralty_core::{AcceptanceCriterion, CheckSpec, ProjectId, RootId, TaskContract, TaskId};
use admiralty_runtime::{
    session::{CaptureState, LaunchSpec, SessionError, SessionHost},
    workspace::registry::{Custody, NodeRef, WorkspaceSpec},
};
use std::{error::Error, fs, path::PathBuf};

fn launch_spec(root: &std::path::Path, run_id: &str, output: &str) -> LaunchSpec {
    LaunchSpec {
        run_id: run_id.into(),
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".into(), format!("printf '%s' '{output}'")],
        cwd: root.to_path_buf(),
        spool_dir: root.join(format!("spool-{run_id}")),
    }
}

fn run_once(output: &str) -> Result<(Vec<u8>, u64), Box<dyn Error>> {
    let fixture = tempfile::tempdir()?;
    let host = SessionHost::new();
    host.launch(launch_spec(fixture.path(), "replay", output))?;
    let exit = host.wait("replay")?;
    assert!(exit.success());
    let status = host.status("replay")?;
    assert_eq!(status.capture_state, CaptureState::Complete);
    let tail = host.tail("replay", 0, 1024)?;
    Ok((
        tail.chunks
            .into_iter()
            .flat_map(|chunk| chunk.bytes)
            .collect(),
        tail.next_offset,
    ))
}

fn work_order(project: &str, task: &str) -> Result<TaskContract, Box<dyn Error>> {
    Ok(TaskContract {
        schema_version: 1,
        project_id: ProjectId::new(project)?,
        root_id: RootId::new("round14-root")?,
        task_id: TaskId::new(task)?,
        revision: 1,
        title: "Round fourteen isolation check".into(),
        objective: "Validate scoped release evidence".into(),
        non_goals: vec!["Do not grant execution or acceptance".into()],
        owned_paths: vec!["owned".into()],
        forbidden_paths: vec!["private".into()],
        read_first: vec!["README.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "replayable".into(),
            text: "Named check evidence is scoped to this work order".into(),
            check_ids: vec!["audit".into()],
        }],
        checks: vec![CheckSpec {
            id: "audit".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: format!("fixture:{project}:{task}"),
        max_attempts: 1,
        simulated: false,
    })
}

#[test]
fn codex_round14_p09_p16_workspace_lifecycle_and_session_isolation() -> Result<(), Box<dyn Error>> {
    let workspace = tempfile::tempdir()?;
    fs::create_dir(workspace.path().join("owned"))?;
    fs::write(workspace.path().join("owned/input.txt"), "registered")?;

    // P03: work orders validate independently; unbound checks and traversal fail closed.
    let first_order = work_order("round14-project-a", "round14-task-a")?;
    let second_order = work_order("round14-project-b", "round14-task-b")?;
    first_order.validate()?;
    second_order.validate()?;
    assert_ne!(first_order.project_id, second_order.project_id);
    let mut escaped_order = first_order.clone();
    escaped_order.owned_paths = vec!["../outside".into()];
    assert!(escaped_order.validate().is_err());
    let mut unbound_order = second_order;
    unbound_order.criteria[0].check_ids = vec!["undeclared".into()];
    assert!(unbound_order.validate().is_err());

    // P09 registration is repeatable, read-only, and limited to its declared scope.
    let spec = WorkspaceSpec {
        node_id: NodeRef::new("round14-node")?,
        root_id: RootId::new("round14-root")?,
        project_id: ProjectId::new("round14-project")?,
        workspace: fs::canonicalize(workspace.path())?,
        owned_paths: vec![PathBuf::from("owned")],
        forbidden_paths: vec![PathBuf::from("private")],
    };
    let first_registration = spec.validate()?;
    let replayed_registration = spec.validate()?;
    assert_eq!(first_registration, replayed_registration);
    assert_eq!(first_registration.custody, Custody::RegisteredOnly);
    assert!(first_registration.owns(std::path::Path::new("owned/input.txt")));
    assert!(!first_registration.owns(std::path::Path::new("private/token")));
    assert!(!first_registration.owns(std::path::Path::new("../outside")));
    assert_eq!(
        fs::read(workspace.path().join("owned/input.txt"))?,
        b"registered"
    );

    // A symlink cannot extend the registered workspace ownership boundary.
    let outside = tempfile::tempdir()?;
    fs::write(outside.path().join("secret"), "outside")?;
    std::os::unix::fs::symlink(
        outside.path().join("secret"),
        workspace.path().join("owned/escape"),
    )?;
    let mut symlink_spec = spec.clone();
    symlink_spec.owned_paths = vec![PathBuf::from("owned/escape")];
    assert!(symlink_spec.validate().is_err());

    // P16 session outputs are isolated by run ID and replay identically on a fresh host.
    let host = SessionHost::new();
    host.launch(launch_spec(workspace.path(), "alpha", "alpha-output"))?;
    host.launch(launch_spec(workspace.path(), "beta", "beta-output"))?;
    assert!(matches!(
        host.launch(launch_spec(workspace.path(), "alpha", "replacement")),
        Err(SessionError::DuplicateRun(_))
    ));
    assert!(matches!(
        host.status("unknown"),
        Err(SessionError::UnknownRun(_))
    ));
    assert!(host.wait("alpha")?.success());
    assert!(host.wait("beta")?.success());

    let alpha = host.tail("alpha", 0, 1024)?;
    let beta = host.tail("beta", 0, 1024)?;
    let alpha_offset = alpha.next_offset;
    let beta_offset = beta.next_offset;
    let bytes = |tail: admiralty_runtime::session::Tail| {
        tail.chunks
            .into_iter()
            .flat_map(|chunk| chunk.bytes)
            .collect::<Vec<_>>()
    };
    assert_eq!(bytes(alpha), b"alpha-output");
    assert_eq!(bytes(beta), b"beta-output");
    assert_eq!(alpha_offset, b"alpha-output".len() as u64);
    assert_eq!(beta_offset, b"beta-output".len() as u64);
    assert!(host.tail("alpha", alpha_offset, 1024)?.chunks.is_empty());
    assert!(host.tail("beta", beta_offset, 1024)?.chunks.is_empty());
    assert_eq!(run_once("stable-output")?, run_once("stable-output")?);

    // Invalid launch identities fail before creating a spool or running a process.
    let invalid = LaunchSpec {
        run_id: "invalid".into(),
        program: PathBuf::from("sh"),
        args: vec![],
        cwd: workspace.path().to_path_buf(),
        spool_dir: workspace.path().join("invalid-spool"),
    };
    assert!(matches!(
        host.launch(invalid),
        Err(SessionError::InvalidSpec(_))
    ));
    assert!(!workspace.path().join("invalid-spool").exists());
    let invalid_run_id = LaunchSpec {
        run_id: "../escape".into(),
        program: PathBuf::from("/bin/true"),
        args: vec![],
        cwd: workspace.path().to_path_buf(),
        spool_dir: workspace.path().join("invalid-run-id-spool"),
    };
    assert!(matches!(
        host.launch(invalid_run_id),
        Err(SessionError::InvalidSpec(_))
    ));
    assert!(!workspace.path().join("invalid-run-id-spool").exists());
    Ok(())
}
