use admiralty_core::interfaces::{
    CompatibilityEvidence, DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId,
    InterfaceScope, RecipientEpoch, reduce_interface_for,
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::session::{InputKind, LaunchSpec, SessionHost};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

fn principal(value: &str) -> PrincipalId {
    PrincipalId::new(value).expect("fixture principal")
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct GateAuditReceipt {
    schema_version: u32,
    run_id: String,
    input_epoch: u64,
    output_offset: u64,
    artifact_digest: String,
    interface_id: String,
    interface_revision: u64,
    recipient_epoch: String,
    acknowledgment_event_id: String,
}

#[cfg(unix)]
#[test]
fn p07_p17_round10_item3_workspace_gate_audit() {
    let temp = tempfile::tempdir().expect("session workspace fixture");
    let spool = temp.path().join("session-spool");
    let workspace = fs::canonicalize(temp.path()).expect("canonical fixture workspace");
    let host = SessionHost::new();
    host.launch(LaunchSpec {
        run_id: "round10-gate".into(),
        program: PathBuf::from("/bin/cat"),
        args: Vec::new(),
        cwd: workspace,
        spool_dir: spool,
    })
    .expect("start deterministic local session child");

    let artifact = b"workspace gate evidence: candidate bytes\n";
    let input = host
        .write_input(
            "round10-gate",
            InputKind::CompareAndAdvance { expected_epoch: 0 },
            artifact,
        )
        .expect("write candidate bytes once at epoch zero");
    assert_eq!(input.input_epoch, 1);
    assert!(
        host.write_input(
            "round10-gate",
            InputKind::CompareAndAdvance { expected_epoch: 0 },
            artifact,
        )
        .is_err(),
        "stale automation cannot duplicate workspace input"
    );
    host.close_input_if_epoch("round10-gate", 1)
        .expect("close after exact input epoch");
    assert!(
        host.wait("round10-gate")
            .expect("wait for owned child")
            .success()
    );
    let captured = host
        .tail("round10-gate", 0, 1024)
        .expect("read captured workspace output");
    let bytes = captured
        .chunks
        .iter()
        .flat_map(|chunk| chunk.bytes.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(
        bytes, artifact,
        "gate audits the exact bytes captured by P07"
    );

    let scope = InterfaceScope {
        project_id: ProjectId::new("round10-project").expect("project ID"),
        root_id: RootId::new("round10-root").expect("root ID"),
        task_id: TaskId::new("round10-task").expect("task ID"),
        authority_ref: "grant:round10".into(),
    };
    let worker = principal("round10-worker");
    let captain = principal("round10-captain");
    let owner = principal("round10-owner");
    let contract = InterfaceContract {
        interface_id: InterfaceId::new("workspace-gate").expect("interface ID"),
        scope: scope.clone(),
        producer: worker.clone(),
        consumers: vec![captain.clone()],
        revision: DecimalU64::new(1),
        artifact_digest: digest(&bytes),
        schema_digest: digest(b"workspace-gate-schema-v1"),
    };
    let publish = InterfaceEvent::Publish {
        event_id: MessageId::new("round10-gate-publish").expect("publish event ID"),
        contract: contract.clone(),
        actor: worker.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "captain-round10-epoch".into(),
        }],
        expected_state_revision: None,
    };
    let published = reduce_interface_for(&None, &publish, &scope, &worker, &owner)
        .expect("publish scoped workspace gate contract");
    let make_ack = |event_id: &str, artifact_digest: String| InterfaceEvent::Acknowledge {
        event_id: MessageId::new(event_id).expect("ack event ID"),
        interface_id: contract.interface_id.clone(),
        scope: scope.clone(),
        actor: captain.clone(),
        recipient_epoch: "captain-round10-epoch".into(),
        interface_revision: DecimalU64::new(1),
        artifact_digest,
        schema_digest: contract.schema_digest.clone(),
        expected_state_revision: DecimalU64::new(1),
        authority_ref: scope.authority_ref.clone(),
    };
    let tampered = make_ack("round10-tampered-ack", digest(b"different bytes"));
    assert!(
        reduce_interface_for(
            &Some(published.state.clone()),
            &tampered,
            &scope,
            &captain,
            &owner,
        )
        .is_err(),
        "captain gate rejects an acknowledgment for altered workspace bytes"
    );

    let ack = make_ack("round10-captain-ack", contract.artifact_digest.clone());
    let acknowledged = reduce_interface_for(&Some(published.state), &ack, &scope, &captain, &owner)
        .expect("captain acknowledges exact captured workspace artifact");
    assert!(matches!(
        acknowledged.state.evidence.get(&captain),
        Some(CompatibilityEvidence::ProvisionalAck { .. })
    ));
    let replay = reduce_interface_for(
        &Some(acknowledged.state.clone()),
        &ack,
        &scope,
        &captain,
        &owner,
    )
    .expect("exact captain ACK replay");
    assert!(replay.replayed);
    assert_eq!(replay.state, acknowledged.state);
}

#[cfg(unix)]
#[test]
fn p07_p17_round11_item4_final_validation_gate() {
    let temp = tempfile::tempdir().expect("private validation workspace");
    let workspace = fs::canonicalize(temp.path()).expect("canonical workspace path");
    let host = SessionHost::new();
    host.launch(LaunchSpec {
        run_id: "round11-final-gate".into(),
        program: PathBuf::from("/bin/cat"),
        args: Vec::new(),
        cwd: workspace.clone(),
        spool_dir: workspace.join("session-spool"),
    })
    .expect("launch owned session process");

    let candidate = b"round11 immutable workspace candidate\n";
    let delivered = host
        .write_input(
            "round11-final-gate",
            InputKind::CompareAndAdvance { expected_epoch: 0 },
            candidate,
        )
        .expect("deliver one input under epoch zero");
    assert_eq!(delivered.input_epoch, 1);
    assert_eq!(delivered.bytes_written, candidate.len());
    assert!(
        host.write_input(
            "round11-final-gate",
            InputKind::CompareAndAdvance { expected_epoch: 0 },
            candidate,
        )
        .is_err(),
        "replayed stale input is refused"
    );
    assert!(
        host.close_input_if_epoch("round11-final-gate", 0).is_err(),
        "stale epoch cannot close the session input"
    );
    host.close_input_if_epoch("round11-final-gate", 1)
        .expect("close current input epoch");
    assert!(
        host.wait("round11-final-gate")
            .expect("wait for owned session process")
            .success()
    );
    let status = host
        .status("round11-final-gate")
        .expect("observe completed session");
    assert_eq!(status.input_epoch, 2);
    assert!(status.input_closed);
    assert_eq!(status.final_output_offset, Some(candidate.len() as u64));
    let tail = host
        .tail("round11-final-gate", 0, 4096)
        .expect("read full captured output");
    let captured = tail
        .chunks
        .iter()
        .flat_map(|chunk| chunk.bytes.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(captured, candidate, "session output preserves exact bytes");
    assert_eq!(tail.next_offset, candidate.len() as u64);

    let scope = InterfaceScope {
        project_id: ProjectId::new("round11-project").expect("project ID"),
        root_id: RootId::new("round11-root").expect("root ID"),
        task_id: TaskId::new("round11-task").expect("task ID"),
        authority_ref: "grant:round11".into(),
    };
    let worker = principal("round11-worker");
    let captain = principal("round11-captain");
    let owner = principal("round11-owner");
    let first_contract = InterfaceContract {
        interface_id: InterfaceId::new("final-validation-gate").expect("interface ID"),
        scope: scope.clone(),
        producer: worker.clone(),
        consumers: vec![captain.clone()],
        revision: DecimalU64::new(1),
        artifact_digest: digest(&captured),
        schema_digest: digest(b"final-gate-schema-v1"),
    };
    let publish = InterfaceEvent::Publish {
        event_id: MessageId::new("round11-publish-1").expect("publish event ID"),
        contract: first_contract.clone(),
        actor: worker.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "captain-epoch-11".into(),
        }],
        expected_state_revision: None,
    };
    let published = reduce_interface_for(&None, &publish, &scope, &worker, &owner)
        .expect("publish exact session artifact under authenticated scope");

    let acknowledgment = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round11-ack-1").expect("ack event ID"),
        interface_id: first_contract.interface_id.clone(),
        scope: scope.clone(),
        actor: captain.clone(),
        recipient_epoch: "captain-epoch-11".into(),
        interface_revision: DecimalU64::new(1),
        artifact_digest: first_contract.artifact_digest.clone(),
        schema_digest: first_contract.schema_digest.clone(),
        expected_state_revision: DecimalU64::new(1),
        authority_ref: scope.authority_ref.clone(),
    };
    let mut foreign_scope = scope.clone();
    foreign_scope.task_id = TaskId::new("foreign-task").expect("foreign task ID");
    assert!(
        reduce_interface_for(
            &Some(published.state.clone()),
            &acknowledgment,
            &foreign_scope,
            &captain,
            &owner,
        )
        .is_err(),
        "cross-task interface ACK is rejected"
    );
    assert!(
        reduce_interface_for(
            &Some(published.state.clone()),
            &acknowledgment,
            &scope,
            &principal("forged-captain"),
            &owner,
        )
        .is_err(),
        "ACK cannot be attributed to another peer"
    );

    let acknowledged = reduce_interface_for(
        &Some(published.state),
        &acknowledgment,
        &scope,
        &captain,
        &owner,
    )
    .expect("captain ACKs the exact candidate artifact");
    assert!(matches!(
        acknowledged.state.evidence.get(&captain),
        Some(CompatibilityEvidence::ProvisionalAck { .. })
    ));
    let exact_replay = reduce_interface_for(
        &Some(acknowledged.state.clone()),
        &acknowledgment,
        &scope,
        &captain,
        &owner,
    )
    .expect("exact ACK replay is idempotent");
    assert!(exact_replay.replayed);
    assert_eq!(exact_replay.state, acknowledged.state);

    let next_contract = InterfaceContract {
        revision: DecimalU64::new(2),
        artifact_digest: digest(b"round11 revised candidate\n"),
        schema_digest: digest(b"final-gate-schema-v2"),
        ..first_contract
    };
    let revised = InterfaceEvent::Publish {
        event_id: MessageId::new("round11-publish-2").expect("revision event ID"),
        contract: next_contract,
        actor: worker.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "captain-epoch-12".into(),
        }],
        expected_state_revision: Some(DecimalU64::new(2)),
    };
    let invalidated =
        reduce_interface_for(&Some(acknowledged.state), &revised, &scope, &worker, &owner)
            .expect("new contract revision proposes invalidation");
    assert_eq!(invalidated.invalidations.len(), 1);
    assert_eq!(invalidated.invalidations[0].consumer, captain);
    assert_eq!(
        invalidated.invalidations[0].previous_revision,
        DecimalU64::new(1)
    );
    assert_eq!(
        invalidated.invalidations[0].new_revision,
        DecimalU64::new(2)
    );
}

#[cfg(unix)]
#[test]
fn p07_p17_round12_item5_redteam_audit_gate() {
    let temp = tempfile::tempdir().expect("red-team session workspace");
    let workspace = fs::canonicalize(temp.path()).expect("canonical workspace");
    let host = SessionHost::new();
    host.launch(LaunchSpec {
        run_id: "round12-redteam".into(),
        program: PathBuf::from("/bin/cat"),
        args: Vec::new(),
        cwd: workspace.clone(),
        spool_dir: workspace.join("spool"),
    })
    .expect("launch local fixture process");
    let payload = b"audited candidate payload\n";
    assert_eq!(
        host.write_input(
            "round12-redteam",
            InputKind::CompareAndAdvance { expected_epoch: 0 },
            payload,
        )
        .expect("first epoch input")
        .input_epoch,
        1
    );
    assert!(
        host.write_input(
            "round12-redteam",
            InputKind::CompareAndAdvance { expected_epoch: 0 },
            payload,
        )
        .is_err(),
        "replayed input with a stale epoch must fail closed"
    );
    host.close_input_if_epoch("round12-redteam", 1)
        .expect("close at current epoch");
    assert!(
        host.wait("round12-redteam")
            .expect("owned process exit")
            .success()
    );
    let tail = host
        .tail("round12-redteam", 0, 1024)
        .expect("session journal tail");
    let output = tail
        .chunks
        .iter()
        .flat_map(|chunk| chunk.bytes.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(output, payload);
    let artifact_digest = digest(&output);

    let scope = InterfaceScope {
        project_id: ProjectId::new("round12-project").expect("project ID"),
        root_id: RootId::new("round12-root").expect("root ID"),
        task_id: TaskId::new("round12-task").expect("task ID"),
        authority_ref: "grant:round12".into(),
    };
    let producer = principal("round12-worker");
    let recipient = principal("round12-captain");
    let owner = principal("round12-owner");
    let contract = InterfaceContract {
        interface_id: InterfaceId::new("redteam-interface").expect("interface ID"),
        scope: scope.clone(),
        producer: producer.clone(),
        consumers: vec![recipient.clone()],
        revision: DecimalU64::new(1),
        artifact_digest: artifact_digest.clone(),
        schema_digest: digest(b"redteam-schema-v1"),
    };
    let publish = InterfaceEvent::Publish {
        event_id: MessageId::new("round12-publish").expect("publish ID"),
        contract: contract.clone(),
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: recipient.clone(),
            epoch: "captain-epoch-12".into(),
        }],
        expected_state_revision: None,
    };
    let published = reduce_interface_for(&None, &publish, &scope, &producer, &owner)
        .expect("publish session-bound artifact");
    let ack = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round12-ack").expect("ack ID"),
        interface_id: contract.interface_id.clone(),
        scope: scope.clone(),
        actor: recipient.clone(),
        recipient_epoch: "captain-epoch-12".into(),
        interface_revision: DecimalU64::new(1),
        artifact_digest: artifact_digest.clone(),
        schema_digest: contract.schema_digest.clone(),
        expected_state_revision: DecimalU64::new(1),
        authority_ref: scope.authority_ref.clone(),
    };

    let mut forged_scope = scope.clone();
    forged_scope.project_id = ProjectId::new("foreign-project").expect("foreign project");
    assert!(
        reduce_interface_for(
            &Some(published.state.clone()),
            &ack,
            &forged_scope,
            &recipient,
            &owner,
        )
        .is_err(),
        "foreign project cannot consume a scoped artifact"
    );
    assert!(
        reduce_interface_for(
            &Some(published.state.clone()),
            &ack,
            &scope,
            &principal("forged-peer"),
            &owner,
        )
        .is_err(),
        "peer identity is authenticated independently of event fields"
    );
    let mut tampered_ack = ack.clone();
    if let InterfaceEvent::Acknowledge {
        artifact_digest, ..
    } = &mut tampered_ack
    {
        *artifact_digest = digest(b"altered candidate");
    }
    assert!(
        reduce_interface_for(
            &Some(published.state.clone()),
            &tampered_ack,
            &scope,
            &recipient,
            &owner,
        )
        .is_err(),
        "changed artifact digest cannot pass the interface gate"
    );

    let accepted = reduce_interface_for(&Some(published.state), &ack, &scope, &recipient, &owner)
        .expect("authorized matching ACK");
    let collision = reduce_interface_for(
        &Some(accepted.state.clone()),
        &tampered_ack,
        &scope,
        &recipient,
        &owner,
    );
    assert!(collision.is_err(), "ACK event identity cannot be rebound");

    let receipt = GateAuditReceipt {
        schema_version: 1,
        run_id: "round12-redteam".into(),
        input_epoch: host
            .status("round12-redteam")
            .expect("session status")
            .input_epoch,
        output_offset: tail.next_offset,
        artifact_digest,
        interface_id: "redteam-interface".into(),
        interface_revision: accepted.state.revision.get(),
        recipient_epoch: "captain-epoch-12".into(),
        acknowledgment_event_id: "round12-ack".into(),
    };
    let receipt_bytes = serde_json::to_vec(&receipt).expect("serialize exact audit receipt");
    let receipt_digest = digest(&receipt_bytes);
    let decoded: GateAuditReceipt =
        serde_json::from_slice(&receipt_bytes).expect("strict receipt round trip");
    assert_eq!(decoded, receipt);
    let mut altered = decoded.clone();
    altered.output_offset += 1;
    assert_ne!(
        digest(&serde_json::to_vec(&altered).expect("serialize alteration")),
        receipt_digest
    );
    let mut unknown_field: serde_json::Value =
        serde_json::from_slice(&receipt_bytes).expect("receipt JSON value");
    unknown_field["authority_granted"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<GateAuditReceipt>(unknown_field).is_err());
}
