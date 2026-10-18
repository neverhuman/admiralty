#![cfg(unix)]

use admiralty_core::{AcceptanceCriterion, CheckSpec, ProjectId, RootId, TaskContract, TaskId};
use admiralty_runtime::{
    session::{CaptureState, InputKind, LaunchSpec, SessionError, SessionHost},
    workspace::registry::{Custody, NodeRef, WorkspaceSpec},
};
use std::{error::Error, fs, path::PathBuf};

fn contract(step: usize) -> Result<TaskContract, Box<dyn Error>> {
    let suffix = step.to_string();
    Ok(TaskContract {
        schema_version: 1,
        project_id: ProjectId::new(format!("round15-project-{suffix}"))?,
        root_id: RootId::new("round15-root")?,
        task_id: TaskId::new(format!("round15-task-{suffix}"))?,
        revision: 1,
        title: format!("Release smoke step {suffix}"),
        objective: "Complete one bounded deterministic lifecycle step".into(),
        non_goals: vec!["Do not grant release or execution authority".into()],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![".git".into()],
        read_first: vec!["README.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "smoke-output".into(),
            text: "The named smoke check completes with the expected output".into(),
            check_ids: vec!["smoke".into()],
        }],
        checks: vec![CheckSpec {
            id: "smoke".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: format!("fixture:round15:{suffix}"),
        max_attempts: 1,
        simulated: false,
    })
}

fn launch(root: &std::path::Path, id: &str, step: usize) -> LaunchSpec {
    let label = format!("step-{step}");
    LaunchSpec {
        run_id: id.into(),
        program: PathBuf::from("/bin/sh"),
        args: vec![
            "-c".into(),
            format!(
                "printf 'ready:{label};'; IFS= read -r value; printf 'done:{label}:%s' \"$value\""
            ),
        ],
        cwd: root.to_path_buf(),
        spool_dir: root.join(format!("spool-{id}")),
    }
}

#[test]
fn p03_p09_p16_round15_post_item4_release_smoke_multistep_stress() -> Result<(), Box<dyn Error>> {
    let workspace = tempfile::tempdir()?;
    fs::create_dir(workspace.path().join("src"))?;
    fs::write(
        workspace.path().join("src/lib.rs"),
        "// release smoke fixture\n",
    )?;

    // P03: independent work-order identities validate; malformed scope/check bindings reject.
    for step in 0..3 {
        let mut task = contract(step)?;
        task.validate()?;
        task.revision = step as u32 + 1;
        task.validate()?;
    }
    let mut unsafe_task = contract(9)?;
    unsafe_task.owned_paths = vec!["src/../../outside".into()];
    assert!(unsafe_task.validate().is_err());
    let mut unbound_task = contract(10)?;
    unbound_task.criteria[0].check_ids = vec!["absent-check".into()];
    assert!(unbound_task.validate().is_err());

    // P09: repeated registration is stable and never expands custody to Git metadata.
    let spec = WorkspaceSpec {
        node_id: NodeRef::new("round15-node")?,
        root_id: RootId::new("round15-root")?,
        project_id: ProjectId::new("round15-project")?,
        workspace: fs::canonicalize(workspace.path())?,
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    };
    let registered = spec.validate()?;
    assert_eq!(registered, spec.validate()?);
    assert_eq!(registered.custody, Custody::RegisteredOnly);
    assert!(registered.owns(std::path::Path::new("src/lib.rs")));
    assert!(!registered.owns(std::path::Path::new(".git/config")));
    assert!(!registered.owns(std::path::Path::new("../outside")));
    let symlink_root = workspace.path().join("workspace-link");
    std::os::unix::fs::symlink(workspace.path(), &symlink_root)?;
    let mut symlink_registration = spec.clone();
    symlink_registration.workspace = symlink_root;
    assert!(symlink_registration.validate().is_err());

    // P16: three sessions advance independently through input, EOF, exit, and replay.
    let host = SessionHost::new();
    for step in 0..3 {
        host.launch(launch(workspace.path(), &format!("run-{step}"), step))?;
    }
    for step in 0..3 {
        let id = format!("run-{step}");
        let input = format!("payload-{step}\n");
        let receipt = host.write_input(
            &id,
            InputKind::CompareAndAdvance { expected_epoch: 0 },
            input.as_bytes(),
        )?;
        assert_eq!(receipt.input_epoch, 1);
        assert!(matches!(
            host.write_input(
                &id,
                InputKind::CompareAndAdvance { expected_epoch: 0 },
                input.as_bytes()
            ),
            Err(SessionError::StaleInput {
                expected: 0,
                current: 1
            })
        ));
        assert_eq!(host.close_input_if_epoch(&id, 1)?.input_epoch, 2);
        assert!(host.wait(&id)?.success());
        let status = host.status(&id)?;
        assert_eq!(status.capture_state, CaptureState::Complete);
        assert_eq!(status.input_epoch, 2);
        let tail = host.tail(&id, 0, 1024)?;
        let output: Vec<u8> = tail
            .chunks
            .into_iter()
            .flat_map(|chunk| chunk.bytes)
            .collect();
        assert_eq!(
            output,
            format!("ready:step-{step};done:step-{step}:payload-{step}").into_bytes()
        );
        assert!(host.tail(&id, tail.next_offset, 1024)?.chunks.is_empty());
    }
    assert!(matches!(
        host.status("missing-run"),
        Err(SessionError::UnknownRun(_))
    ));
    assert!(matches!(
        host.close_input("missing-run"),
        Err(SessionError::UnknownRun(_))
    ));
    Ok(())
}
