#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::interfaces::{
    DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId, InterfaceScope,
    MAX_INTERFACE_EVENT_BYTES, RecipientEpoch, decode_event, reduce_interface_for,
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

fn task(task_id: &str, project_id: ProjectId) -> Result<TaskContract, Box<dyn Error>> {
    Ok(TaskContract {
        schema_version: 1,
        project_id,
        root_id: RootId::new("round21-root")?,
        task_id: TaskId::new(task_id)?,
        revision: 1,
        title: format!("Hermetic release {task_id}"),
        objective: "Qualify deterministic release evidence and interface parity".into(),
        non_goals: vec![],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec![],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "release-evidence".into(),
            text: "Candidate and interface evidence remain bound".into(),
            check_ids: vec!["check".into()],
        }],
        checks: vec![CheckSpec {
            id: "check".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "grant:round21".into(),
        max_attempts: 1,
        simulated: true,
    })
}

fn principal(value: &str) -> Result<PrincipalId, Box<dyn Error>> {
    Ok(PrincipalId::new(value)?)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct HermeticReleaseBundle {
    schema_version: u32,
    work_order_digest: String,
    task_ids: Vec<String>,
    task_digests: Vec<String>,
    candidate_digest: String,
    interface_id: String,
    interface_revision: String,
    interface_schema_digest: String,
}

#[test]
fn p07_p17_round21_hermetic_ci_cross_packet_release_qualification() -> Result<(), Box<dyn Error>> {
    let project = ProjectId::new("round21-project")?;
    let workplan = Workplan {
        schema_version: 1,
        plan_id: "round21-plan".into(),
        revision: 1,
        project_id: project.clone(),
        tasks: vec![
            task("release", project.clone())?,
            task("audit", project.clone())?,
        ],
    };
    let workplan_bytes = serde_json::to_vec(&workplan)?;
    let preview = preview_workplan(&workplan_bytes)?;
    let replay = preview_workplan_with_existing(&workplan_bytes, &preview.task_digests)?;
    assert_eq!(
        preview, replay,
        "hermetic work-order replay is deterministic"
    );
    let task_ids = preview
        .task_order
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let task_digests = task_ids
        .iter()
        .map(|task_id| {
            preview
                .task_digests
                .iter()
                .find(|(id, _)| id.as_str() == task_id)
                .map(|(_, value)| value.clone())
                .ok_or("preview omitted task digest")
        })
        .collect::<Result<Vec<_>, _>>()?;

    let candidate = b"round21 hermetic release candidate\n";
    let candidate_digest = digest(candidate);
    let scope = InterfaceScope {
        project_id: project,
        root_id: RootId::new("round21-root")?,
        task_id: TaskId::new("release")?,
        authority_ref: "grant:round21".into(),
    };
    let producer = principal("round21-producer")?;
    let reviewer = principal("round21-reviewer")?;
    let owner = principal("round21-owner")?;
    let contract = InterfaceContract {
        interface_id: InterfaceId::new("round21-release-interface")?,
        scope: scope.clone(),
        producer: producer.clone(),
        consumers: vec![reviewer.clone()],
        revision: DecimalU64::new(1),
        artifact_digest: candidate_digest.clone(),
        schema_digest: digest(b"round21-interface-schema-v1"),
    };
    let bundle = HermeticReleaseBundle {
        schema_version: 1,
        work_order_digest: preview.source_digest,
        task_ids,
        task_digests,
        candidate_digest,
        interface_id: contract.interface_id.as_str().to_owned(),
        interface_revision: "1".into(),
        interface_schema_digest: contract.schema_digest.clone(),
    };
    let bundle_bytes = serde_json::to_vec(&bundle)?;
    let bundle_digest = digest(&bundle_bytes);
    let decoded_bundle: HermeticReleaseBundle = serde_json::from_slice(&bundle_bytes)?;
    assert_eq!(decoded_bundle, bundle);
    assert_eq!(digest(&serde_json::to_vec(&decoded_bundle)?), bundle_digest);
    let mut forged_bundle: serde_json::Value = serde_json::from_slice(&bundle_bytes)?;
    forged_bundle["acceptance_granted"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<HermeticReleaseBundle>(forged_bundle).is_err());

    let publish = InterfaceEvent::Publish {
        event_id: MessageId::new("round21-publish")?,
        contract: contract.clone(),
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: reviewer.clone(),
            epoch: "round21-reviewer-epoch".into(),
        }],
        expected_state_revision: None,
    };
    let wire = serde_json::to_vec(&publish)?;
    let by_serde: InterfaceEvent = serde_json::from_slice(&wire)?;
    let by_api = decode_event(&wire)?;
    assert_eq!(
        by_serde, by_api,
        "public decoder and serde schema stay in parity"
    );
    assert!(decode_event(&vec![b' '; MAX_INTERFACE_EVENT_BYTES + 1]).is_err());

    let published = reduce_interface_for(&None, &by_api, &scope, &producer, &owner)?;
    let replayed = reduce_interface_for(
        &Some(published.state.clone()),
        &by_api,
        &scope,
        &producer,
        &owner,
    )?;
    assert!(replayed.replayed);
    assert_eq!(replayed.state, published.state);

    let ack = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round21-reviewer-ack")?,
        interface_id: contract.interface_id.clone(),
        scope: scope.clone(),
        actor: reviewer.clone(),
        recipient_epoch: "round21-reviewer-epoch".into(),
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
        &reviewer,
        &owner,
    )?;
    assert_eq!(
        acknowledged.state.contract.artifact_digest, bundle.candidate_digest,
        "P17 compatibility evidence must bind the P07 candidate"
    );
    let ack_replay = reduce_interface_for(
        &Some(acknowledged.state.clone()),
        &ack,
        &scope,
        &reviewer,
        &owner,
    )?;
    assert!(ack_replay.replayed);
    assert_eq!(ack_replay.state, acknowledged.state);

    let mut foreign_scope = scope.clone();
    foreign_scope.task_id = TaskId::new("foreign-task")?;
    assert!(
        reduce_interface_for(
            &Some(acknowledged.state),
            &ack,
            &foreign_scope,
            &reviewer,
            &owner,
        )
        .is_err()
    );
    Ok(())
}
