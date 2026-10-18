use admiralty_core::interfaces::{
    CompatibilityEvidence, DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId,
    InterfaceScope, RecipientEpoch, reduce_interface_for,
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, RootId, TaskId};

fn principal(value: &str) -> PrincipalId {
    PrincipalId::new(value).expect("fixture principal")
}

fn scope() -> InterfaceScope {
    InterfaceScope {
        project_id: ProjectId::new("project").expect("fixture project"),
        root_id: RootId::new("root").expect("fixture root"),
        task_id: TaskId::new("producer-task").expect("fixture task"),
        authority_ref: "grant-1".into(),
    }
}

fn digest(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

fn publish(event_id: &str, revision: u64, expected_state_revision: Option<u64>) -> InterfaceEvent {
    let consumer = principal("consumer");
    InterfaceEvent::Publish {
        event_id: MessageId::new(event_id).expect("fixture event"),
        contract: InterfaceContract {
            interface_id: InterfaceId::new("api-v1").expect("fixture interface"),
            scope: scope(),
            producer: principal("producer"),
            consumers: vec![consumer.clone()],
            revision: DecimalU64::new(revision),
            artifact_digest: digest(if revision == 1 { 'a' } else { 'c' }),
            schema_digest: digest(if revision == 1 { 'b' } else { 'd' }),
        },
        actor: principal("producer"),
        authority_owner: principal("owner"),
        recipient_epochs: vec![RecipientEpoch {
            recipient: consumer,
            epoch: if revision == 1 { "epoch-1" } else { "epoch-2" }.into(),
        }],
        expected_state_revision: expected_state_revision.map(DecimalU64::new),
    }
}

fn acknowledge() -> InterfaceEvent {
    InterfaceEvent::Acknowledge {
        event_id: MessageId::new("ack-1").expect("fixture event"),
        interface_id: InterfaceId::new("api-v1").expect("fixture interface"),
        scope: scope(),
        actor: principal("consumer"),
        recipient_epoch: "epoch-1".into(),
        interface_revision: DecimalU64::new(1),
        artifact_digest: digest('a'),
        schema_digest: digest('b'),
        expected_state_revision: DecimalU64::new(1),
        authority_ref: "grant-1".into(),
    }
}

#[test]
fn consumer_ack_is_provisional_and_new_revision_proposes_invalidation() {
    let scope = scope();
    let owner = principal("owner");
    let producer = principal("producer");
    let consumer = principal("consumer");
    let initial = reduce_interface_for(
        &None,
        &publish("publish-1", 1, None),
        &scope,
        &producer,
        &owner,
    )
    .expect("publish interface contract");
    let ack_event = acknowledge();
    let acknowledged =
        reduce_interface_for(&Some(initial.state), &ack_event, &scope, &consumer, &owner)
            .expect("consumer acknowledgment");
    assert!(matches!(
        acknowledged.state.evidence.get(&consumer),
        Some(CompatibilityEvidence::ProvisionalAck { .. })
    ));
    assert!(acknowledged.invalidations.is_empty());

    let replay = reduce_interface_for(
        &Some(acknowledged.state.clone()),
        &ack_event,
        &scope,
        &consumer,
        &owner,
    )
    .expect("idempotent acknowledgment replay");
    assert!(replay.replayed);
    assert_eq!(replay.state, acknowledged.state);

    let revised = reduce_interface_for(
        &Some(acknowledged.state),
        &publish("publish-2", 2, Some(2)),
        &scope,
        &producer,
        &owner,
    )
    .expect("publish revised contract");
    assert_eq!(revised.invalidations.len(), 1);
    assert_eq!(revised.invalidations[0].consumer, consumer);
    assert_eq!(
        revised.invalidations[0].previous_revision,
        DecimalU64::new(1)
    );
    assert_eq!(revised.invalidations[0].new_revision, DecimalU64::new(2));
}
