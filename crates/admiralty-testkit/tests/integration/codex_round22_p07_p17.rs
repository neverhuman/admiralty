#![allow(clippy::unwrap_used, clippy::expect_used)]

use admiralty_core::interfaces::{
    DecimalU64, InterfaceContract, InterfaceEvent, InterfaceId, InterfaceScope, RecipientEpoch,
    decode_event, reduce_interface_for,
};
use admiralty_core::{MessageId, PrincipalId, ProjectId, RootId, TaskId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::error::Error;

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn principal(name: &str) -> Result<PrincipalId, Box<dyn Error>> {
    Ok(PrincipalId::new(name)?)
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CanaryPackageEvidence {
    schema_version: u32,
    candidate_digest: String,
    interface_id: String,
    interface_revision: String,
    schema_digest: String,
    smoke_status: String,
}

#[test]
fn p07_p17_round22_canary_smoke_release_package_qualification() -> Result<(), Box<dyn Error>> {
    let candidate = b"round22 deterministic canary release payload\n";
    let candidate_digest = digest(candidate);
    let schema_digest = digest(b"round22 P17 interface schema v1");
    let project = ProjectId::new("round22-project")?;
    let scope = InterfaceScope {
        project_id: project,
        root_id: RootId::new("round22-root")?,
        task_id: TaskId::new("canary-release")?,
        authority_ref: "grant:round22-canary".into(),
    };
    let producer = principal("round22-builder")?;
    let canary = principal("round22-canary")?;
    let owner = principal("round22-owner")?;
    let contract = InterfaceContract {
        interface_id: InterfaceId::new("round22-release-interface")?,
        scope: scope.clone(),
        producer: producer.clone(),
        consumers: vec![canary.clone()],
        revision: DecimalU64::new(1),
        artifact_digest: candidate_digest.clone(),
        schema_digest: schema_digest.clone(),
    };
    let evidence = CanaryPackageEvidence {
        schema_version: 1,
        candidate_digest: candidate_digest.clone(),
        interface_id: contract.interface_id.as_str().to_owned(),
        interface_revision: "1".into(),
        schema_digest: schema_digest.clone(),
        smoke_status: "passed".into(),
    };
    let evidence_bytes = serde_json::to_vec(&evidence)?;
    let evidence_digest = digest(&evidence_bytes);
    let decoded: CanaryPackageEvidence = serde_json::from_slice(&evidence_bytes)?;
    assert_eq!(decoded, evidence);
    assert_eq!(digest(&serde_json::to_vec(&decoded)?), evidence_digest);

    let publish = InterfaceEvent::Publish {
        event_id: MessageId::new("round22-publish")?,
        contract: contract.clone(),
        actor: producer.clone(),
        authority_owner: owner.clone(),
        recipient_epochs: vec![RecipientEpoch {
            recipient: canary.clone(),
            epoch: "round22-canary-epoch".into(),
        }],
        expected_state_revision: None,
    };
    let wire = serde_json::to_vec(&publish)?;
    let public_decode = decode_event(&wire)?;
    let serde_decode: InterfaceEvent = serde_json::from_slice(&wire)?;
    assert_eq!(public_decode, serde_decode, "P17 schema and API decoder agree");
    let published = reduce_interface_for(&None, &public_decode, &scope, &producer, &owner)?;
    let replay = reduce_interface_for(
        &Some(published.state.clone()),
        &public_decode,
        &scope,
        &producer,
        &owner,
    )?;
    assert!(replay.replayed);
    assert_eq!(replay.state, published.state);

    let ack = InterfaceEvent::Acknowledge {
        event_id: MessageId::new("round22-canary-ack")?,
        interface_id: contract.interface_id.clone(),
        scope: scope.clone(),
        actor: canary.clone(),
        recipient_epoch: "round22-canary-epoch".into(),
        interface_revision: contract.revision.clone(),
        artifact_digest: evidence.candidate_digest.clone(),
        schema_digest: evidence.schema_digest.clone(),
        expected_state_revision: published.state.revision.clone(),
        authority_ref: scope.authority_ref.clone(),
    };
    let accepted = reduce_interface_for(
        &Some(published.state.clone()),
        &ack,
        &scope,
        &canary,
        &owner,
    )?;
    assert_eq!(accepted.state.contract.artifact_digest, candidate_digest);

    let mut forged = serde_json::to_value(&evidence)?;
    forged["release_authorized"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<CanaryPackageEvidence>(forged).is_err());
    let mut substituted_ack = ack.clone();
    if let InterfaceEvent::Acknowledge { artifact_digest, .. } = &mut substituted_ack {
        *artifact_digest = digest(b"substituted package");
    }
    assert!(reduce_interface_for(
        &Some(published.state.clone()),
        &substituted_ack,
        &scope,
        &canary,
        &owner,
    )
    .is_err());
    let mut outside_scope = scope.clone();
    outside_scope.task_id = TaskId::new("other-release")?;
    assert!(reduce_interface_for(
        &Some(accepted.state),
        &ack,
        &outside_scope,
        &canary,
        &owner,
    )
    .is_err());
    Ok(())
}
