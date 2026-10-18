#![cfg(unix)]

use admiralty_core::{ProjectId, RootId};
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

#[test]
fn codex_round14_p09_p16_workspace_lifecycle_and_session_isolation() -> Result<(), Box<dyn Error>> {
    let workspace = tempfile::tempdir()?;
    fs::create_dir(workspace.path().join("owned"))?;
    fs::write(workspace.path().join("owned/input.txt"), "registered")?;

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
    let bytes = |tail: admiralty_runtime::session::Tail| {
        tail.chunks
            .into_iter()
            .flat_map(|chunk| chunk.bytes)
            .collect::<Vec<_>>()
    };
    assert_eq!(bytes(alpha), b"alpha-output");
    assert_eq!(bytes(beta), b"beta-output");
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
    Ok(())
}
