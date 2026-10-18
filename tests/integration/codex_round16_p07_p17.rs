use admiralty_core::interfaces::{
    DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId, InterfaceScope, RecipientEpoch,
    decode_event, reduce_interface_for,
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::session::{InputKind, LaunchSpec, SessionHost};
use sha2::{Digest, Sha256};
use std::{error::Error, fs, path::PathBuf};

fn principal(value: &str) -> Result<PrincipalId, Box<dyn Error>> {
    Ok(PrincipalId::new(value)?)
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[cfg(unix)]
#[test]
fn codex_round16_p07_p17() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let workspace = fs::canonicalize(temp.path())?;
    let host = SessionHost::new();
    host.launch(LaunchSpec {
        run_id: "round16-cutover".into(),
        program: PathBuf::from("/bin/cat"),
        args: Vec::new(),
        cwd: workspace,
        spool_dir: temp.path().join("session-spool"),
    })?;

    let artifact = b"round16 release candidate: interface schema v1\n";
    let input = host.write_input(
        "round16-cutover",
        InputKind::CompareAndAdvance { expected_epoch: 0 },
        artifact,
    )?;
    assert_eq!(input.input_epoch, 1);
    assert!(
        host.write_input(
            "round16-cutover",
            InputKind::CompareAndAdvance { expected_epoch: 0 },
            artifact,
        )
        .is_err(),
        "stale session epoch must not duplicate the artifact"
    );
    host.close_input_if_epoch("round16-cutover", 1)?;
    assert!(host.wait("round16-cutover")?.success());
    let captured = host.tail("round16-cutover", 0, 4096)?;
    let captured_bytes = captured
        .chunks
        .iter()
        .flat_map(|chunk| chunk.bytes.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(
        captured_bytes, artifact,
        "P07 capture binds exact release bytes"
    );

    let scope = InterfaceScope {
        project_id: ProjectId::new("round16-project")?,
        root_id: RootId::new("round16-root")?,
        task_id: TaskId::new("round16-task")?,
        authority_ref: "grant:round16".into(),
    };
    let producer = principal("round16-producer")?;
    let captain = principal("round16-captain")?;
    let owner = principal("round16-owner")?;
    let interface_id = InterfaceId::new("round16-release-interface")?;
    let contract_v1 = InterfaceContract {
        interface_id: interface_id.clone(),
        scope: scope.clone(),
        producer: producer.clone(),
        consumers: vec![captain.clone()],
        revision: DecimalU64::new(1),
        artifact_digest: digest(&captured_bytes),
        schema_digest: digest(b"captain-interface-schema-v1"),
    };
    let publish_v1 = InterfaceEvent::Publish {
        event_id: MessageId::new("round16-publish-v1")?,
        contract: contract_v1.clone(),
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "captain-epoch-v1".into(),
        }],
        expected_state_revision: None,
    };

    let wire = serde_json::to_vec(&publish_v1)?;
    let serde_event: InterfaceEvent = serde_json::from_slice(&wire)?;
    let api_event = decode_event(&wire)?;
    assert_eq!(serde_event, api_event, "schema and serde APIs must agree");
    let state_v1 = reduce_interface_for(&None, &api_event, &scope, &producer, &owner)?.state;
    let replay_v1 = reduce_interface_for(
        &Some(state_v1.clone()),
        &api_event,
        &scope,
        &producer,
        &owner,
    )?;
    assert!(replay_v1.replayed);
    assert_eq!(replay_v1.state, state_v1);

    let ack_v1 = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round16-ack-v1")?,
        interface_id: interface_id.clone(),
        scope: scope.clone(),
        actor: captain.clone(),
        recipient_epoch: "captain-epoch-v1".into(),
        interface_revision: DecimalU64::new(1),
        artifact_digest: contract_v1.artifact_digest.clone(),
        schema_digest: contract_v1.schema_digest.clone(),
        expected_state_revision: state_v1.revision.clone(),
        authority_ref: scope.authority_ref.clone(),
    };
    let acknowledged_v1 =
        reduce_interface_for(&Some(state_v1.clone()), &ack_v1, &scope, &captain, &owner)?;

    // Cut over to v2; the reducer invalidates the old provisional acknowledgment.
    let contract_v2 = InterfaceContract {
        revision: DecimalU64::new(2),
        artifact_digest: digest(b"round16 release candidate v2"),
        schema_digest: digest(b"captain-interface-schema-v2"),
        ..contract_v1
    };
    let publish_v2 = InterfaceEvent::Publish {
        event_id: MessageId::new("round16-publish-v2")?,
        contract: contract_v2.clone(),
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "captain-epoch-v2".into(),
        }],
        expected_state_revision: Some(acknowledged_v1.state.revision.clone()),
    };
    let cutover = reduce_interface_for(
        &Some(acknowledged_v1.state.clone()),
        &publish_v2,
        &scope,
        &producer,
        &owner,
    )?;
    assert_eq!(cutover.invalidations.len(), 1);
    assert_eq!(cutover.invalidations[0].consumer, captain);
    assert!(cutover.state.evidence.is_empty());

    let stale_ack = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round16-stale-ack-v1")?,
        interface_id: interface_id.clone(),
        scope: scope.clone(),
        actor: principal("round16-captain")?,
        recipient_epoch: "captain-epoch-v1".into(),
        interface_revision: DecimalU64::new(1),
        artifact_digest: digest(artifact),
        schema_digest: digest(b"captain-interface-schema-v1"),
        expected_state_revision: cutover.state.revision.clone(),
        authority_ref: scope.authority_ref.clone(),
    };
    assert!(
        reduce_interface_for(
            &Some(cutover.state.clone()),
            &stale_ack,
            &scope,
            &captain,
            &owner,
        )
        .is_err(),
        "stale schema evidence cannot cross the cutover"
    );

    let mut forged: serde_json::Value = serde_json::from_slice(&serde_json::to_vec(&publish_v2)?)?;
    forged["acceptance_granted"] = serde_json::Value::Bool(true);
    assert!(decode_event(&serde_json::to_vec(&forged)?).is_err());

    let mut foreign_scope = scope.clone();
    foreign_scope.task_id = TaskId::new("foreign-task")?;
    assert!(
        reduce_interface_for(
            &Some(cutover.state),
            &ack_v1,
            &foreign_scope,
            &captain,
            &owner,
        )
        .is_err(),
        "foreign scope cannot reuse prior evidence"
    );
    Ok(())
}
