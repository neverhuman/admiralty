#![allow(clippy::unwrap_used, clippy::expect_used)]
use admiralty_core::interfaces::{
    DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId, InterfaceScope, RecipientEpoch,
    decode_event, reduce_interface_for,
};
use admiralty_core::workplans::{Workplan, preview_workplan, preview_workplan_with_existing};
use admiralty_core::{
    AcceptanceCriterion, CheckSpec, MessageId, PrincipalId, ProjectId, RootId, TaskContract, TaskId,
};
use serde::{Deserialize, Serialize};
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
        root_id: RootId::new("round18-root")?,
        task_id: TaskId::new(task_id)?,
        revision: 1,
        title: format!("Release bundle {task_id}"),
        objective: "Verify the exact candidate and interface contract".into(),
        non_goals: vec![],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![],
        read_first: vec![],
        dependencies: dependencies
            .iter()
            .map(|dependency| TaskId::new(*dependency))
            .collect::<Result<Vec<_>, _>>()?,
        criteria: vec![AcceptanceCriterion {
            id: "release-evidence".into(),
            text: "The exact candidate passes the declared check".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "grant:round18".into(),
        max_attempts: 1,
        simulated: true,
    })
}

fn principal(value: &str) -> Result<PrincipalId, Box<dyn Error>> {
    Ok(PrincipalId::new(value)?)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReleaseBundle {
    schema_version: u32,
    work_order_digest: String,
    task_bindings: Vec<(String, String)>,
    candidate_digest: String,
    interface_id: String,
    interface_revision: u64,
    interface_schema_digest: String,
}

#[test]
fn p07_p17_round18_release_bundle_schema_parity() -> Result<(), Box<dyn Error>> {
    let project = ProjectId::new("round18-project")?;
    let plan = Workplan {
        schema_version: 1,
        plan_id: "round18-release-plan".into(),
        revision: 4,
        project_id: project.clone(),
        tasks: vec![
            task("release", project.clone(), &["build"])?,
            task("build", project.clone(), &[])?,
        ],
    };
    let work_order_bytes = serde_json::to_vec(&plan)?;
    let preview = preview_workplan(&work_order_bytes)?;
    let replay = preview_workplan_with_existing(&work_order_bytes, &preview.task_digests)?;
    assert_eq!(
        preview, replay,
        "work-order replay preserves all identities"
    );
    assert_eq!(
        preview.task_order,
        [TaskId::new("build")?, TaskId::new("release")?]
    );

    let candidate = b"round18 immutable release candidate\n";
    let candidate_digest = digest(candidate);
    let scope = InterfaceScope {
        project_id: project,
        root_id: RootId::new("round18-root")?,
        task_id: TaskId::new("release")?,
        authority_ref: "grant:round18".into(),
    };
    let producer = principal("round18-producer")?;
    let captain = principal("round18-captain")?;
    let owner = principal("round18-owner")?;
    let contract = InterfaceContract {
        interface_id: InterfaceId::new("round18-release-api")?,
        scope: scope.clone(),
        producer: producer.clone(),
        consumers: vec![captain.clone()],
        revision: DecimalU64::new(7),
        artifact_digest: candidate_digest.clone(),
        schema_digest: digest(b"round18-interface-schema"),
    };
    let bundle = ReleaseBundle {
        schema_version: 1,
        work_order_digest: preview.source_digest.clone(),
        task_bindings: preview
            .task_digests
            .iter()
            .map(|(task_id, task_digest)| (task_id.to_string(), task_digest.clone()))
            .collect(),
        candidate_digest: candidate_digest.clone(),
        interface_id: contract.interface_id.as_str().to_owned(),
        interface_revision: contract.revision.get(),
        interface_schema_digest: contract.schema_digest.clone(),
    };
    let bundle_bytes = serde_json::to_vec(&bundle)?;
    let bundle_digest = digest(&bundle_bytes);
    let decoded_bundle: ReleaseBundle = serde_json::from_slice(&bundle_bytes)?;
    assert_eq!(decoded_bundle, bundle);
    assert_eq!(digest(&serde_json::to_vec(&decoded_bundle)?), bundle_digest);
    let mut unknown_bundle: serde_json::Value = serde_json::from_slice(&bundle_bytes)?;
    unknown_bundle["accepted"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<ReleaseBundle>(unknown_bundle).is_err());

    let publish = InterfaceEvent::Publish {
        event_id: MessageId::new("round18-publish")?,
        contract: contract.clone(),
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: captain.clone(),
            epoch: "round18-epoch-7".into(),
        }],
        expected_state_revision: None,
    };
    let publish_wire = serde_json::to_vec(&publish)?;
    let decoded_by_serde: InterfaceEvent = serde_json::from_slice(&publish_wire)?;
    let decoded_by_public_api = decode_event(&publish_wire)?;
    assert_eq!(decoded_by_serde, decoded_by_public_api);
    let published = reduce_interface_for(&None, &decoded_by_public_api, &scope, &producer, &owner)?;
    let publish_replay = reduce_interface_for(
        &Some(published.state.clone()),
        &decoded_by_public_api,
        &scope,
        &producer,
        &owner,
    )?;
    assert!(publish_replay.replayed);
    assert_eq!(publish_replay.state, published.state);

    let ack = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round18-captain-ack")?,
        interface_id: contract.interface_id.clone(),
        scope: scope.clone(),
        actor: captain.clone(),
        recipient_epoch: "round18-epoch-7".into(),
        interface_revision: contract.revision.clone(),
        artifact_digest: bundle.candidate_digest.clone(),
        schema_digest: bundle.interface_schema_digest.clone(),
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
    assert_eq!(
        acknowledged.state.contract.artifact_digest, bundle.candidate_digest,
        "P17 evidence remains bound to the P07 release bundle candidate"
    );
    let ack_replay = reduce_interface_for(
        &Some(acknowledged.state.clone()),
        &ack,
        &scope,
        &captain,
        &owner,
    )?;
    assert!(ack_replay.replayed);
    assert_eq!(ack_replay.state, acknowledged.state);

    let tampered_ack = match ack {
        InterfaceEvent::Acknowledge {
            interface_id,
            scope: ack_scope,
            actor,
            recipient_epoch,
            interface_revision,
            schema_digest,
            expected_state_revision,
            authority_ref,
            ..
        } => InterfaceEvent::Acknowledge {
            event_id: MessageId::new("round18-tampered-ack")?,
            interface_id,
            scope: ack_scope,
            actor,
            recipient_epoch,
            interface_revision,
            artifact_digest: digest(b"different candidate"),
            schema_digest,
            expected_state_revision,
            authority_ref,
        },
        _ => return Err("acknowledgment fixture changed variant".into()),
    };
    assert!(
        reduce_interface_for(
            &Some(published.state),
            &tampered_ack,
            &scope,
            &captain,
            &owner,
        )
        .is_err()
    );
    Ok(())
}
