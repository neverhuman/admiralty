use admiralty_core::interfaces::{
    CompatibilityEvidence, DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId,
    InterfaceScope, RecipientEpoch, reduce_interface_for,
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::workspace::registry::{Custody, NodeRef, WorkspaceSpec};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

fn principal(value: &str) -> PrincipalId {
    PrincipalId::new(value).expect("fixture principal")
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

#[test]
fn p07_p17_round8_p09_captain_interfaces_qualification() {
    let temp = tempfile::tempdir().expect("private workspace fixture");
    let workspace = temp.path().join("workspace");
    fs::create_dir(&workspace).expect("workspace directory");
    fs::create_dir(workspace.join("src")).expect("owned source directory");
    let artifact = b"pub fn interface_v1() -> u32 { 7 }\n";
    fs::write(workspace.join("src/lib.rs"), artifact).expect("fixture artifact");

    let registered = WorkspaceSpec {
        node_id: NodeRef::new("node-local").expect("node reference"),
        root_id: RootId::new("round8-root").expect("root ID"),
        project_id: ProjectId::new("round8-project").expect("project ID"),
        workspace: fs::canonicalize(&workspace).expect("canonical workspace"),
        owned_paths: vec![PathBuf::from("src")],
        forbidden_paths: vec![PathBuf::from(".git")],
    }
    .validate()
    .expect("P09 registered workspace validates");
    assert_eq!(registered.custody, Custody::RegisteredOnly);
    assert!(registered.owns(&PathBuf::from("src/lib.rs")));
    let artifact_digest = digest(&fs::read(workspace.join("src/lib.rs")).expect("artifact bytes"));

    let scope = InterfaceScope {
        project_id: ProjectId::new("round8-project").expect("project ID"),
        root_id: RootId::new("round8-root").expect("root ID"),
        task_id: TaskId::new("round8-task").expect("task ID"),
        authority_ref: "grant:round8".into(),
    };
    let producer = principal("worker");
    let captain = principal("captain");
    let owner = principal("authority-owner");
    let contract = InterfaceContract {
        interface_id: InterfaceId::new("captain-api").expect("interface ID"),
        scope: scope.clone(),
        producer: producer.clone(),
        consumers: vec![captain.clone()],
        revision: DecimalU64::new(1),
        artifact_digest: artifact_digest.clone(),
        schema_digest: digest(b"schema-v1"),
    };
    let publish = InterfaceEvent::Publish {
        event_id: MessageId::new("round8-publish-1").expect("event ID"),
        contract: contract.clone(),
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "captain-epoch-4".into(),
        }],
        expected_state_revision: None,
    };
    let published = reduce_interface_for(&None, &publish, &scope, &producer, &owner)
        .expect("producer publishes scoped artifact contract");

    let ack = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round8-captain-ack").expect("event ID"),
        interface_id: contract.interface_id.clone(),
        scope: scope.clone(),
        actor: captain.clone(),
        recipient_epoch: "captain-epoch-4".into(),
        interface_revision: DecimalU64::new(1),
        artifact_digest: artifact_digest.clone(),
        schema_digest: contract.schema_digest.clone(),
        expected_state_revision: DecimalU64::new(1),
        authority_ref: scope.authority_ref.clone(),
    };
    let mut wrong_scope = scope.clone();
    wrong_scope.task_id = TaskId::new("foreign-task").expect("foreign task ID");
    assert!(
        reduce_interface_for(
            &Some(published.state.clone()),
            &ack,
            &wrong_scope,
            &captain,
            &owner,
        )
        .is_err(),
        "captain acknowledgment must be bound to the exact task scope"
    );
    assert!(
        reduce_interface_for(
            &Some(published.state.clone()),
            &ack,
            &scope,
            &principal("untrusted-peer"),
            &owner,
        )
        .is_err(),
        "a peer cannot acknowledge as the captain"
    );

    let acknowledged = reduce_interface_for(&Some(published.state), &ack, &scope, &captain, &owner)
        .expect("authenticated captain acknowledges exact artifact");
    assert_eq!(
        acknowledged.state.evidence.get(&captain),
        Some(&CompatibilityEvidence::ProvisionalAck {
            artifact_digest,
            schema_digest: contract.schema_digest,
        })
    );

    let revised_contract = InterfaceContract {
        revision: DecimalU64::new(2),
        artifact_digest: digest(b"pub fn interface_v2() -> u32 { 8 }\n"),
        schema_digest: digest(b"schema-v2"),
        ..contract
    };
    let revised = InterfaceEvent::Publish {
        event_id: MessageId::new("round8-publish-2").expect("event ID"),
        contract: revised_contract,
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "captain-epoch-5".into(),
        }],
        expected_state_revision: Some(DecimalU64::new(2)),
    };
    let invalidated = reduce_interface_for(
        &Some(acknowledged.state),
        &revised,
        &scope,
        &producer,
        &owner,
    )
    .expect("new artifact contract records dependency invalidation");
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
