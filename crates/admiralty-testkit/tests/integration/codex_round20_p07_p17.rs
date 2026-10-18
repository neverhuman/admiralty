#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::interfaces::{
    DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId, InterfaceScope, RecipientEpoch,
    decode_event, reduce_interface_for,
};
use admiralty_core::workplans::{Workplan, WorkplanError, preview_workplan};
use admiralty_core::{
    AcceptanceCriterion, CheckSpec, MessageId, PrincipalId, ProjectId, RootId, TaskContract, TaskId,
};
use sha2::{Digest, Sha256};
use std::error::Error;

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn task(
    task_id: &str,
    project_id: ProjectId,
    dependencies: &[&str],
) -> Result<TaskContract, Box<dyn Error>> {
    Ok(TaskContract {
        schema_version: 1,
        project_id,
        root_id: RootId::new("round20-root")?,
        task_id: TaskId::new(task_id)?,
        revision: 1,
        title: format!("Post PR 57 {task_id}"),
        objective: "Bind release evidence to a deterministic work order".into(),
        non_goals: vec![],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![],
        read_first: vec![],
        dependencies: dependencies
            .iter()
            .map(|dependency| TaskId::new(*dependency))
            .collect::<Result<Vec<_>, _>>()?,
        criteria: vec![AcceptanceCriterion {
            id: "release".into(),
            text: "The exact contract and interface evidence are replayable".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "grant:round20".into(),
        max_attempts: 1,
        simulated: true,
    })
}

fn principal(value: &str) -> Result<PrincipalId, Box<dyn Error>> {
    Ok(PrincipalId::new(value)?)
}

#[test]
fn p07_p17_round20_post_pr57_release_qualification() -> Result<(), Box<dyn Error>> {
    let project = ProjectId::new("round20-project")?;
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round20-release".into(),
        revision: 2,
        project_id: project.clone(),
        tasks: vec![
            task("release", project.clone(), &["build"])?,
            task("build", project.clone(), &[])?,
        ],
    };
    let plan_bytes = serde_json::to_vec(&plan)?;
    let first_preview = preview_workplan(&plan_bytes)?;
    let replay_preview = preview_workplan(&plan_bytes)?;
    assert_eq!(first_preview, replay_preview);
    assert_eq!(
        first_preview.task_order,
        [TaskId::new("build")?, TaskId::new("release")?]
    );

    let mut forged_plan: serde_json::Value = serde_json::from_slice(&plan_bytes)?;
    forged_plan["tasks"][0]["accepted"] = serde_json::Value::Bool(true);
    assert!(matches!(
        preview_workplan(&serde_json::to_vec(&forged_plan)?),
        Err(WorkplanError::Json(_))
    ));

    let artifact = b"round20 candidate bound to exact release task\n";
    let scope = InterfaceScope {
        project_id: project,
        root_id: RootId::new("round20-root")?,
        task_id: TaskId::new("release")?,
        authority_ref: "grant:round20".into(),
    };
    let producer = principal("round20-worker")?;
    let captain = principal("round20-captain")?;
    let owner = principal("round20-owner")?;
    let contract = InterfaceContract {
        interface_id: InterfaceId::new("round20-release-api")?,
        scope: scope.clone(),
        producer: producer.clone(),
        consumers: vec![captain.clone()],
        revision: DecimalU64::new(1),
        artifact_digest: digest(artifact),
        schema_digest: digest(b"round20-schema-v1"),
    };
    let publish = InterfaceEvent::Publish {
        event_id: MessageId::new("round20-publish-v1")?,
        contract: contract.clone(),
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "captain-round20-v1".into(),
        }],
        expected_state_revision: None,
    };

    // Keep serde's wire representation and the public schema decoder in parity.
    let publish_wire = serde_json::to_vec(&publish)?;
    let via_serde: InterfaceEvent = serde_json::from_slice(&publish_wire)?;
    let via_decoder = decode_event(&publish_wire)?;
    assert_eq!(via_serde, via_decoder);
    let published = reduce_interface_for(&None, &via_decoder, &scope, &producer, &owner)?;
    let replayed = reduce_interface_for(
        &Some(published.state.clone()),
        &via_decoder,
        &scope,
        &producer,
        &owner,
    )?;
    assert!(replayed.replayed);
    assert_eq!(replayed.state, published.state);

    // A reused event ID with altered evidence is a collision and does not mutate state.
    let mut collision_json: serde_json::Value = serde_json::from_slice(&publish_wire)?;
    collision_json["contract"]["artifact_digest"] = digest(b"substituted artifact").into();
    let collision: InterfaceEvent = serde_json::from_value(collision_json)?;
    assert!(
        reduce_interface_for(
            &Some(published.state.clone()),
            &collision,
            &scope,
            &producer,
            &owner,
        )
        .is_err()
    );

    let ack = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round20-ack-v1")?,
        interface_id: contract.interface_id.clone(),
        scope: scope.clone(),
        actor: captain.clone(),
        recipient_epoch: "captain-round20-v1".into(),
        interface_revision: contract.revision.clone(),
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
    let contract_v2 = InterfaceContract {
        revision: DecimalU64::new(2),
        artifact_digest: digest(b"round20 candidate v2"),
        schema_digest: digest(b"round20-schema-v2"),
        ..contract
    };
    let publish_v2 = InterfaceEvent::Publish {
        event_id: MessageId::new("round20-publish-v2")?,
        contract: contract_v2,
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "captain-round20-v2".into(),
        }],
        expected_state_revision: Some(acknowledged.state.revision.clone()),
    };
    let cutover = reduce_interface_for(
        &Some(acknowledged.state),
        &publish_v2,
        &scope,
        &producer,
        &owner,
    )?;
    assert_eq!(cutover.invalidations.len(), 1);
    assert!(cutover.state.evidence.is_empty());

    let mut foreign_scope = scope.clone();
    foreign_scope.task_id = TaskId::new("foreign-task")?;
    assert!(
        reduce_interface_for(&Some(cutover.state), &ack, &foreign_scope, &captain, &owner,)
            .is_err()
    );
    let mut unknown_event: serde_json::Value = serde_json::from_slice(&publish_wire)?;
    unknown_event["acceptance_granted"] = serde_json::Value::Bool(true);
    assert!(decode_event(&serde_json::to_vec(&unknown_event)?).is_err());
    Ok(())
}
