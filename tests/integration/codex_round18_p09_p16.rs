#![cfg(unix)]

use admiralty_core::{
    AcceptanceCriterion, CheckSpec, DomainEvent, ProjectId, RootId, SourceIdentity, TaskContract,
    TaskId,
};
use admiralty_runtime::{
    session::{CaptureState, LaunchSpec, SessionHost},
    workspace::registry::{Custody, NodeRef, WorkspaceSpec},
};
use admiralty_store::{Store, outbox::OutboxIntent};
use serde_json::{Value, json};
use std::{error::Error, fs, os::unix::fs::PermissionsExt, path::PathBuf};

const TASK_CONTRACT_SCHEMA: &str = include_str!("../../schemas/task-contract.schema.json");

fn contract() -> Result<TaskContract, Box<dyn Error>> {
    Ok(TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("round18-project")?,
        root_id: RootId::new("round18-root")?,
        task_id: TaskId::new("round18-task")?,
        revision: 1,
        title: "Release bundle smoke".into(),
        objective: "Bind release bundle to validated scope and replayable evidence".into(),
        non_goals: vec!["Do not grant acceptance or execution".into()],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![".git".into()],
        read_first: vec!["README.md".into()],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "release-smoke".into(),
            text: "The release smoke check is present and schema-bound".into(),
            check_ids: vec!["smoke".into()],
        }],
        checks: vec![CheckSpec {
            id: "smoke".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "fixture:round18-release-bundle".into(),
        max_attempts: 1,
        simulated: false,
    })
}

fn launch(root: &std::path::Path) -> LaunchSpec {
    LaunchSpec {
        run_id: "round18-smoke".into(),
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".into(), "printf release-smoke".into()],
        cwd: root.to_path_buf(),
        spool_dir: root.join("spool-round18-smoke"),
    }
}

#[test]
fn p03_p09_p16_round18_release_bundle_schema_parity() -> Result<(), Box<dyn Error>> {
    // Schema parity: generated contract schema must equal the checked-in release schema.
    let generated_schema = serde_json::to_value(schemars::schema_for!(TaskContract))?;
    let checked_in_schema: Value = serde_json::from_str(TASK_CONTRACT_SCHEMA)?;
    assert_eq!(generated_schema, checked_in_schema);

    let temporary = tempfile::tempdir()?;
    let workspace = fs::canonicalize(temporary.path())?;
    fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700))?;
    fs::create_dir(workspace.join("src"))?;
    fs::create_dir(workspace.join(".git"))?;
    fs::write(workspace.join("src/lib.rs"), "// release bundle fixture\n")?;

    // P03: the work order and event survive typed schema round-trip before persistence.
    let work_order = contract()?;
    work_order.validate()?;
    let contract_json = serde_json::to_value(&work_order)?;
    let decoded_contract: TaskContract = serde_json::from_value(contract_json.clone())?;
    assert_eq!(decoded_contract, work_order);
    let event = DomainEvent::TaskSubmitted {
        contract: decoded_contract,
    };
    let event_json = serde_json::to_value(&event)?;
    let decoded_event: DomainEvent = serde_json::from_value(event_json.clone())?;
    assert_eq!(decoded_event, event);

    let registration = WorkspaceSpec {
        node_id: NodeRef::new("round18-node")?,
        root_id: work_order.root_id.clone(),
        project_id: work_order.project_id.clone(),
        workspace: workspace.clone(),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    };
    let validated = registration.validate()?;
    assert_eq!(validated.custody, Custody::RegisteredOnly);
    assert!(validated.owns(std::path::Path::new("src/lib.rs")));
    assert!(!validated.owns(std::path::Path::new(".git/config")));

    // P16: session capture produces bounded bundle evidence with no authority grants.
    let host = SessionHost::new();
    host.launch(launch(&workspace))?;
    assert!(host.wait("round18-smoke")?.success());
    let status = host.status("round18-smoke")?;
    assert_eq!(status.capture_state, CaptureState::Complete);
    let tail = host.tail("round18-smoke", 0, 1024)?;
    let captured: Vec<u8> = tail
        .chunks
        .into_iter()
        .flat_map(|chunk| chunk.bytes)
        .collect();
    assert_eq!(captured, b"release-smoke");
    assert!(host.status("missing").is_err());

    let source = SourceIdentity {
        producer_id: "round18-release-auditor".into(),
        incarnation: "bundle-1".into(),
        sequence: "1".into(),
    };
    let intent = OutboxIntent {
        idempotency_key: "round18-release-bundle".into(),
        payload: json!({"task":"round18-task","workspace":validated.node_id.as_str()}),
        not_before: 18,
    };
    let store = Store::open(workspace.join("release.sqlite"))?;
    let recorded = store.append_with_intent(
        source.clone(),
        event.clone(),
        "2026-10-18T00:00:00Z".into(),
        intent.clone(),
    )?;
    let replay = store.append_with_intent(source, event, "2026-10-18T00:00:00Z".into(), intent)?;
    assert!(!recorded.duplicate);
    assert!(replay.duplicate);
    assert_eq!(replay.event_seq, recorded.event_seq);

    use sha2::Digest;
    let release_bundle = json!({
        "schema_version": 1,
        "task_contract": contract_json,
        "domain_event": event_json,
        "workspace": {
            "node_id": validated.node_id.as_str(),
            "project_id": validated.project_id,
            "root_id": validated.root_id,
            "custody": "registered_only",
            "owned_paths": validated.owned_paths,
            "forbidden_paths": validated.forbidden_paths,
        },
        "session": {
            "run_id": "round18-smoke",
            "capture_state": "complete",
            "output_offset": tail.next_offset.to_string(),
            "sha256": format!("{:x}", sha2::Sha256::digest(&captured)),
        },
        "event_seq": recorded.event_seq.to_string(),
        "acceptance_granted": false,
        "execution_granted": false,
        "release_granted": false,
    });
    let encoded_bundle = serde_json::to_vec(&release_bundle)?;
    let decoded_bundle: Value = serde_json::from_slice(&encoded_bundle)?;
    assert_eq!(decoded_bundle, release_bundle);
    assert_eq!(decoded_bundle["task_contract"]["schema_version"], 1);
    assert_eq!(decoded_bundle["workspace"]["custody"], "registered_only");
    assert_eq!(decoded_bundle["session"]["output_offset"], "13");
    assert_eq!(decoded_bundle["acceptance_granted"], false);
    assert_eq!(decoded_bundle["execution_granted"], false);
    assert_eq!(decoded_bundle["release_granted"], false);

    let mut schema_mismatch = release_bundle;
    schema_mismatch["task_contract"]["schema_version"] = 2.into();
    let mismatched: TaskContract =
        serde_json::from_value(schema_mismatch["task_contract"].clone())?;
    assert!(mismatched.validate().is_err());
    Ok(())
}
