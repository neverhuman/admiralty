use admiralty_core::interfaces::{
    DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId, InterfaceScope, RecipientEpoch,
    decode_event, reduce_interface_for,
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, RootId, TaskId};
use std::error::Error;

fn principal(value: &str) -> Result<PrincipalId, Box<dyn Error>> {
    Ok(PrincipalId::new(value)?)
}

fn scope() -> Result<InterfaceScope, Box<dyn Error>> {
    Ok(InterfaceScope {
        project_id: ProjectId::new("round14-project")?,
        root_id: RootId::new("round14-root")?,
        task_id: TaskId::new("round14-task")?,
        authority_ref: "grant:round14".into(),
    })
}

#[test]
fn codex_round14_p07_p17() -> Result<(), Box<dyn Error>> {
    let expected_scope = scope()?;
    let producer = principal("round14-worker")?;
    let captain = principal("round14-captain")?;
    let owner = principal("round14-owner")?;
    let contract = InterfaceContract {
        interface_id: InterfaceId::new("round14-captain-api")?,
        scope: expected_scope.clone(),
        producer: producer.clone(),
        consumers: vec![captain.clone()],
        revision: DecimalU64::new(1),
        artifact_digest: format!("sha256:{}", "a".repeat(64)),
        schema_digest: format!("sha256:{}", "b".repeat(64)),
    };
    let publish = InterfaceEvent::Publish {
        event_id: MessageId::new("round14-publish")?,
        contract: contract.clone(),
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "captain-epoch-14".into(),
        }],
        expected_state_revision: None,
    };

    // The JSON schema contract is stable, strict, and shared with the decode API.
    let encoded = serde_json::to_vec(&publish)?;
    let decoded = decode_event(&encoded)?;
    assert_eq!(decoded, publish);
    let mut unknown_field: serde_json::Value = serde_json::from_slice(&encoded)?;
    unknown_field["authority_granted"] = serde_json::Value::Bool(true);
    let forged_bytes = serde_json::to_vec(&unknown_field)?;
    assert!(decode_event(&forged_bytes).is_err());
    assert!(InterfaceId::new("../foreign").is_err());

    let published = reduce_interface_for(&None, &decoded, &expected_scope, &producer, &owner)?;
    let replay = reduce_interface_for(
        &Some(published.state.clone()),
        &decoded,
        &expected_scope,
        &producer,
        &owner,
    )?;
    assert!(replay.replayed);
    assert_eq!(replay.state, published.state);

    let acknowledge = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round14-ack")?,
        interface_id: contract.interface_id.clone(),
        scope: expected_scope.clone(),
        actor: captain.clone(),
        recipient_epoch: "captain-epoch-14".into(),
        interface_revision: DecimalU64::new(1),
        artifact_digest: contract.artifact_digest.clone(),
        schema_digest: contract.schema_digest.clone(),
        expected_state_revision: published.state.revision.clone(),
        authority_ref: expected_scope.authority_ref.clone(),
    };
    let acknowledged = reduce_interface_for(
        &Some(published.state.clone()),
        &acknowledge,
        &expected_scope,
        &captain,
        &owner,
    )?;
    let replayed_ack = reduce_interface_for(
        &Some(acknowledged.state.clone()),
        &acknowledge,
        &expected_scope,
        &captain,
        &owner,
    )?;
    assert!(replayed_ack.replayed);
    assert_eq!(replayed_ack.state, acknowledged.state);

    let mut foreign_scope = expected_scope;
    foreign_scope.task_id = TaskId::new("foreign-task")?;
    assert!(
        reduce_interface_for(
            &Some(published.state.clone()),
            &acknowledge,
            &foreign_scope,
            &captain,
            &owner,
        )
        .is_err()
    );
    let mut substituted_artifact = acknowledge.clone();
    if let InterfaceEvent::Acknowledge {
        artifact_digest, ..
    } = &mut substituted_artifact
    {
        *artifact_digest = format!("sha256:{}", "c".repeat(64));
    }
    assert!(
        reduce_interface_for(
            &Some(published.state),
            &substituted_artifact,
            &scope()?,
            &captain,
            &owner,
        )
        .is_err()
    );
    Ok(())
}
