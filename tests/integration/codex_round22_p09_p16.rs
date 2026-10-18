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

fn release_contract() -> Result<TaskContract, Box<dyn Error>> {
    Ok(TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round22-project")?,
        root_id: RootId::new("round22-root")?,
        task_id: TaskId::new("round22-task")?,
        revision: 1,
        title: "Canary release package smoke".into(),
        objective: "Preserve scope and session isolation through a canary package run".into(),
        non_goals: vec!["Do not grant acceptance or publication".into()],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![".git".into()],
        read_first: vec!["README.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "canary-smoke".into(),
            text: "The named canary check is bound to this task".into(),
            check_ids: vec!["package-smoke".into()],
        }],
        checks: vec![CheckSpec {
            id: "package-smoke".into(),
            program: "python3".into(),
            args: vec!["scripts/release/package.py".into(), "--self-test".into()],
            cwd: ".".into(),
            timeout_secs: 60,
        }],
        authority_ref: "fixture:round22-canary-smoke".into(),
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
        spool_dir: root.join(format!("session-{run_id}")),
    }
}

#[test]
fn p03_p09_p16_round22_canary_smoke_release_package_qualification() -> Result<(), Box<dyn Error>> {
    let fixture = tempfile::tempdir()?;
    let workspace = fs::canonicalize(fixture.path())?;
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700))?;
    fs::create_dir(workspace.join("src"))?;
    fs::create_dir(workspace.join(".git"))?;
    fs::write(workspace.join("src/lib.rs"), "// round22 canary fixture\n")?;

    // P03: work-order identity and exact evidence replay survive the release handoff.
    let contract = release_contract()?;
    contract.validate()?;
    let mut escaped = contract.clone();
    escaped.owned_paths = vec!["../outside".into()];
    assert!(escaped.validate().is_err());
    let event = DomainEvent::TaskSubmitted {
        contract: contract.clone(),
    };
    let source = SourceIdentity {
        producer_id: "round22-release-canary".into(),
        incarnation: "canary-1".into(),
        sequence: "1".into(),
    };
    let intent = OutboxIntent {
        idempotency_key: "round22-package-smoke".into(),
        payload: json!({"task":"round22-task","operation":"record-canary-evidence"}),
        not_before: 22,
    };
    let store = Store::open(workspace.join("canary.sqlite"))?;
    let initial = store.append_with_intent(
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
    assert!(!initial.duplicate);
    assert!(replay.duplicate);
    assert_eq!(initial.event_seq, replay.event_seq);
    assert!(
        store
            .append_with_intent(
                source,
                event,
                "2026-10-18T00:00:00Z".into(),
                OutboxIntent {
                    idempotency_key: "round22-package-smoke".into(),
                    payload: json!({"release_granted":true}),
                    not_before: 22,
                },
            )
            .is_err()
    );
    assert_eq!(store.snapshot()?.projection.tasks.len(), 1);

    // P09: registration is read-only, bounded and rejects symlink escapes.
    let registration = WorkspaceSpec {
        node_id: NodeRef::new("round22-node")?,
        root_id: contract.root_id.clone(),
        project_id: contract.project_id.clone(),
        workspace: workspace.clone(),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    };
    let registered = registration.validate()?;
    assert_eq!(registered.custody, Custody::RegisteredOnly);
    assert!(registered.owns(std::path::Path::new("src/lib.rs")));
    assert!(!registered.owns(std::path::Path::new(".git/config")));
    assert!(!registered.owns(std::path::Path::new("../outside")));
    let link = workspace.join("root-link");
    std::os::unix::fs::symlink(&workspace, &link)?;
    let mut symlink_scope = registration;
    symlink_scope.workspace = link;
    assert!(symlink_scope.validate().is_err());

    // P16: the failed canary process remains isolated from the successful peer.
    let host = SessionHost::new();
    host.launch(launch(&workspace, "package-ok", "printf package-ok"))?;
    host.launch(launch(
        &workspace,
        "package-fail",
        "printf canary-fault; exit 21",
    ))?;
    assert!(matches!(
        host.launch(launch(&workspace, "package-ok", "printf replacement")),
        Err(SessionError::DuplicateRun(_))
    ));
    assert!(host.wait("package-ok")?.success());
    assert!(!host.wait("package-fail")?.success());
    assert_eq!(
        host.status("package-ok")?.capture_state,
        CaptureState::Complete
    );
    assert_eq!(
        host.status("package-fail")?.capture_state,
        CaptureState::Complete
    );
    let successful_output: Vec<u8> = host
        .tail("package-ok", 0, 1024)?
        .chunks
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    let failed_output: Vec<u8> = host
        .tail("package-fail", 0, 1024)?
        .chunks
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    assert_eq!(successful_output, b"package-ok");
    assert_eq!(failed_output, b"canary-fault");
    assert!(matches!(
        host.status("not-launched"),
        Err(SessionError::UnknownRun(_))
    ));
    assert!(admiralty_runtime::audits::decode_evidence(br#"{"release_granted":true}"#).is_err());
    Ok(())
}
