use admiralty_core::interfaces::{
    DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId, InterfaceScope, RecipientEpoch,
    decode_event, reduce_interface_for,
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::session::{InputKind, LaunchSpec, SessionHost};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{error::Error, fs, path::PathBuf};

fn principal(value: &str) -> Result<PrincipalId, Box<dyn Error>> {
    Ok(PrincipalId::new(value)?)
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReleaseAudit {
    schema_version: u32,
    artifact_digest: String,
    interface_id: String,
    interface_revision: u64,
    schema_digest: String,
    acknowledgment_event_id: String,
}

#[cfg(unix)]
#[test]
fn codex_round17_p07_p17() -> Result<(), Box<dyn Error>> {
    let temp = tempfile::tempdir()?;
    let host = SessionHost::new();
    host.launch(LaunchSpec {
        run_id: "round17-release-audit".into(),
        program: PathBuf::from("/bin/cat"),
        args: Vec::new(),
        cwd: fs::canonicalize(temp.path())?,
        spool_dir: temp.path().join("session-spool"),
    })?;
    let artifact = b"P07 candidate bytes bound to P17 release evidence\n";
    let accepted_input = host.write_input(
        "round17-release-audit",
        InputKind::CompareAndAdvance { expected_epoch: 0 },
        artifact,
    )?;
    assert_eq!(accepted_input.input_epoch, 1);
    assert!(
        host.write_input(
            "round17-release-audit",
            InputKind::CompareAndAdvance { expected_epoch: 0 },
            b"substituted candidate",
        )
        .is_err()
    );
    host.close_input_if_epoch("round17-release-audit", 1)?;
    assert!(host.wait("round17-release-audit")?.success());
    let tail = host.tail("round17-release-audit", 0, 4096)?;
    let captured = tail
        .chunks
        .iter()
        .flat_map(|chunk| chunk.bytes.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(captured, artifact);

    let scope = InterfaceScope {
        project_id: ProjectId::new("round17-project")?,
        root_id: RootId::new("round17-root")?,
        task_id: TaskId::new("round17-task")?,
        authority_ref: "grant:round17".into(),
    };
    let producer = principal("round17-producer")?;
    let captain = principal("round17-captain")?;
    let owner = principal("round17-owner")?;
    let interface_id = InterfaceId::new("round17-release-interface")?;
    let contract = InterfaceContract {
        interface_id: interface_id.clone(),
        scope: scope.clone(),
        producer: producer.clone(),
        consumers: vec![captain.clone()],
        revision: DecimalU64::new(1),
        artifact_digest: digest(&captured),
        schema_digest: digest(b"release-audit-schema-v1"),
    };
    let publish = InterfaceEvent::Publish {
        event_id: MessageId::new("round17-publish")?,
        contract: contract.clone(),
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "round17-captain-epoch".into(),
        }],
        expected_state_revision: None,
    };
    let encoded_publish = serde_json::to_vec(&publish)?;
    let decoded_by_serde: InterfaceEvent = serde_json::from_slice(&encoded_publish)?;
    let decoded_by_api = decode_event(&encoded_publish)?;
    assert_eq!(decoded_by_serde, decoded_by_api);
    let published = reduce_interface_for(&None, &decoded_by_api, &scope, &producer, &owner)?;
    let replay = reduce_interface_for(
        &Some(published.state.clone()),
        &decoded_by_api,
        &scope,
        &producer,
        &owner,
    )?;
    assert!(replay.replayed);
    assert_eq!(replay.state, published.state);

    let ack = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round17-captain-ack")?,
        interface_id: interface_id.clone(),
        scope: scope.clone(),
        actor: captain.clone(),
        recipient_epoch: "round17-captain-epoch".into(),
        interface_revision: DecimalU64::new(1),
        artifact_digest: contract.artifact_digest.clone(),
        schema_digest: contract.schema_digest.clone(),
        expected_state_revision: published.state.revision.clone(),
        authority_ref: scope.authority_ref.clone(),
    };
    let acknowledged = reduce_interface_for(
        &Some(published.state.clone()),
        &ack,
        &scope,
        &captain,
        &owner,
    )?;
    let audit = ReleaseAudit {
        schema_version: 1,
        artifact_digest: contract.artifact_digest.clone(),
        interface_id: interface_id.as_str().to_owned(),
        interface_revision: contract.revision.get(),
        schema_digest: contract.schema_digest.clone(),
        acknowledgment_event_id: "round17-captain-ack".into(),
    };
    let audit_bytes = serde_json::to_vec(&audit)?;
    assert_eq!(serde_json::from_slice::<ReleaseAudit>(&audit_bytes)?, audit);
    let mut forged_receipt: serde_json::Value = serde_json::from_slice(&audit_bytes)?;
    forged_receipt["acceptance_granted"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<ReleaseAudit>(forged_receipt).is_err());

    // A new release revision invalidates old captain evidence; an ID collision is contained.
    let contract_v2 = InterfaceContract {
        revision: DecimalU64::new(2),
        artifact_digest: digest(b"round17 replacement artifact"),
        schema_digest: digest(b"release-audit-schema-v2"),
        ..contract
    };
    let publish_v2 = InterfaceEvent::Publish {
        event_id: MessageId::new("round17-publish-v2")?,
        contract: contract_v2,
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "round17-captain-epoch-v2".into(),
        }],
        expected_state_revision: Some(acknowledged.state.revision.clone()),
    };
    let cutover = reduce_interface_for(
        &Some(acknowledged.state.clone()),
        &publish_v2,
        &scope,
        &producer,
        &owner,
    )?;
    assert_eq!(cutover.invalidations.len(), 1);
    assert!(cutover.state.evidence.is_empty());

    let mut collided_ack = ack.clone();
    if let InterfaceEvent::Acknowledge {
        artifact_digest, ..
    } = &mut collided_ack
    {
        *artifact_digest = digest(b"tampered candidate");
    }
    assert!(
        reduce_interface_for(
            &Some(acknowledged.state),
            &collided_ack,
            &scope,
            &captain,
            &owner,
        )
        .is_err(),
        "reused acknowledgment identity with altered evidence fails closed"
    );

    let foreign_scope = InterfaceScope {
        task_id: TaskId::new("foreign-task")?,
        ..scope.clone()
    };
    assert!(
        reduce_interface_for(&Some(cutover.state), &ack, &foreign_scope, &captain, &owner,)
            .is_err()
    );
    Ok(())
}
