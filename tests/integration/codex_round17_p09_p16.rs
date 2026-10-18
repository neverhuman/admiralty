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
        project_id: ProjectId::new("round17-project")?,
        root_id: RootId::new("round17-root")?,
        task_id: TaskId::new("round17-task")?,
        revision: 1,
        title: "Cross-packet release audit".into(),
        objective: "Bind release evidence to one task and workspace scope".into(),
        non_goals: vec!["Do not authorize release or execution".into()],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![".git".into()],
        read_first: vec!["README.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "audit-check".into(),
            text: "The declared audit check is the only criterion evidence source".into(),
            check_ids: vec!["audit".into()],
        }],
        checks: vec![CheckSpec {
            id: "audit".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "fixture:round17-release-audit".into(),
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
fn p03_p09_p16_round17_post_item5_release_audit_cross_packet_invariants()
-> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let workspace = fs::canonicalize(temporary.path())?;
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700))?;
    fs::create_dir(workspace.join("src"))?;
    fs::create_dir(workspace.join(".git"))?;
    fs::write(workspace.join("src/lib.rs"), "// cross-packet fixture\n")?;

    // P03/P09: contract ownership and workspace custody share the same project/root binding.
    let work_order = contract()?;
    work_order.validate()?;
    let registration = WorkspaceSpec {
        node_id: NodeRef::new("round17-node")?,
        root_id: work_order.root_id.clone(),
        project_id: work_order.project_id.clone(),
        workspace: workspace.clone(),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    };
    let validated = registration.validate()?;
    assert_eq!(validated.root_id, work_order.root_id);
    assert_eq!(validated.project_id, work_order.project_id);
    assert_eq!(
        validated.owned_paths,
        work_order
            .owned_paths
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        validated.forbidden_paths,
        work_order
            .forbidden_paths
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    );
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(std::path::Path::new("src/lib.rs")));
    assert!(!validated.owns(std::path::Path::new(".git/config")));

    let mut crossed_registration = registration.clone();
    crossed_registration.project_id = ProjectId::new("foreign-project")?;
    let crossed = crossed_registration.validate()?;
    assert_ne!(crossed.project_id, work_order.project_id);
    assert!(!crossed.owns(std::path::Path::new("../outside")));
    let root_link = workspace.join("root-link");
    std::os::unix::fs::symlink(&workspace, &root_link)?;
    crossed_registration.workspace = root_link;
    assert!(crossed_registration.validate().is_err());

    // P03: source and outbox replay retain one exact work-order event.
    let event = DomainEvent::TaskSubmitted {
        contract: work_order,
    };
    let source = SourceIdentity {
        producer_id: "round17-auditor".into(),
        incarnation: "audit-1".into(),
        sequence: "1".into(),
    };
    let intent = OutboxIntent {
        idempotency_key: "round17-audit-evidence".into(),
        payload: json!({"task":"round17-task","project":"round17-project"}),
        not_before: 17,
    };
    let store = Store::open(workspace.join("audit.sqlite"))?;
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
    assert_eq!(replay.event_seq, first.event_seq);
    let collision = OutboxIntent {
        idempotency_key: "round17-audit-evidence".into(),
        payload: json!({"release_granted":true}),
        not_before: 17,
    };
    assert!(
        store
            .append_with_intent(source, event, "2026-10-18T00:00:00Z".into(), collision,)
            .is_err()
    );
    assert_eq!(store.snapshot()?.projection.tasks.len(), 1);

    // P16: audit claims and child failure remain local to their own evidence/session.
    for forged in [
        br#"{"release_granted":true}"#.as_slice(),
        br#"{"accepted":true,"execution_granted":true}"#.as_slice(),
    ] {
        assert!(admiralty_runtime::audits::decode_evidence(forged).is_err());
    }
    let host = SessionHost::new();
    host.launch(launch(&workspace, "audit-ok", "printf audit-ok"))?;
    host.launch(launch(
        &workspace,
        "audit-fail",
        "printf contained; exit 19",
    ))?;
    assert!(matches!(
        host.launch(launch(&workspace, "audit-ok", "printf forged")),
        Err(SessionError::DuplicateRun(_))
    ));
    assert!(host.wait("audit-ok")?.success());
    assert!(!host.wait("audit-fail")?.success());
    assert_eq!(
        host.status("audit-ok")?.capture_state,
        CaptureState::Complete
    );
    assert_eq!(
        host.status("audit-fail")?.capture_state,
        CaptureState::Complete
    );
    let ok_output: Vec<u8> = host
        .tail("audit-ok", 0, 1024)?
        .chunks
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    let failed_output: Vec<u8> = host
        .tail("audit-fail", 0, 1024)?
        .chunks
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    assert_eq!(ok_output, b"audit-ok");
    assert_eq!(failed_output, b"contained");
    assert!(matches!(
        host.status("not-registered"),
        Err(SessionError::UnknownRun(_))
    ));
    Ok(())
}
