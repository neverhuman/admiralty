#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Hermetic CI and cross-packet release qualification for P23.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeError, NodeId, NodeIdentity, NodeRegistry,
    NodeScope, decode_event, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId, content_digest};
use admiralty_runtime::recovery::{Blocker, plan_recovery};
use serde::Serialize;
use std::{fs, path::Path};

#[path = "../../../../tests/integration/support/recovery.rs"]
mod recovery_fixture;

fn digest(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

fn scope() -> NodeScope {
    NodeScope {
        project_id: ProjectId::new("round21-project").expect("project fixture"),
        root_id: RootId::new("round21-root").expect("root fixture"),
        task_id: TaskId::new("round21-task").expect("task fixture"),
        authority: PrincipalId::new("round21-controller").expect("authority fixture"),
    }
}

fn apply(
    state: &NodeRegistry,
    event: &EnrollmentEvent,
    scope: &NodeScope,
    actor: &PrincipalId,
    now: u64,
) -> Result<NodeRegistry, NodeError> {
    reduce_enrollment_for(state, event, scope, actor, Some(now))
}

#[derive(Serialize)]
struct HermeticReleaseAudit<'a> {
    events: &'a [EnrollmentEvent],
    final_registry: &'a NodeRegistry,
}

fn write_and_read_audit(path: &Path, audit: &HermeticReleaseAudit<'_>) -> Vec<u8> {
    let encoded = serde_json::to_vec(audit).expect("serialize audit bundle");
    fs::write(path, &encoded).expect("write audit bundle to isolated temp directory");
    fs::read(path).expect("read back audit bundle")
}

#[test]
fn p23_round21_hermetic_ci_cross_packet_release_qualification() {
    let scratch = tempfile::tempdir().expect("isolated temporary workspace");
    let audit_path = scratch.path().join("p23-release-audit.json");
    let scope = scope();
    let controller = scope.authority.clone();
    let untrusted = PrincipalId::new("round21-untrusted").expect("principal fixture");
    let node_id = NodeId::new("round21-node").expect("node fixture");
    let token = EnrollmentEvent::TokenIssued {
        event_id: "round21-issue".into(),
        token: EnrollmentToken {
            token_id: "round21-token".into(),
            token_digest: digest('a'),
            scope: scope.clone(),
            expires_tick: DecimalU64::new(40),
            consumed_binding: None,
        },
    };
    let mut events = vec![token];
    let mut registry = apply(&NodeRegistry::default(), &events[0], &scope, &controller, 1)
        .expect("issue scoped token");
    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "round21-enroll".into(),
        token_id: "round21-token".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: node_id.clone(),
            boot_id: "round21-boot-a".into(),
            public_key_digest: digest('b'),
            generation: DecimalU64::new(1),
            capability_digest: digest('c'),
        },
    };
    let enrollment_wire = serde_json::to_vec(&enrollment).expect("encode enrollment event");
    let enrollment = decode_event(&enrollment_wire).expect("decode enrollment event");
    registry = apply(&registry, &enrollment, &scope, &controller, 2).expect("enroll node");
    events.push(enrollment);

    for tick in 3..=7 {
        let heartbeat = EnrollmentEvent::CapacityObserved {
            event_id: format!("round21-heartbeat-{tick}"),
            node_id: node_id.clone(),
            boot_id: "round21-boot-a".into(),
            generation: DecimalU64::new(1),
            observed_tick: Some(DecimalU64::new(tick)),
            fresh_for_ticks: DecimalU64::new(3),
            capacity_digest: digest(char::from_digit(tick as u32, 16).expect("hex fixture")),
        };
        let next =
            apply(&registry, &heartbeat, &scope, &controller, tick).expect("accept heartbeat");
        assert_eq!(
            apply(&registry, &heartbeat, &scope, &controller, tick).expect("replay heartbeat"),
            next,
            "replay is deterministic in the isolated test environment"
        );
        registry = next;
        events.push(heartbeat);
    }

    let unchanged = registry.clone();
    let unauthorized_heartbeat = EnrollmentEvent::CapacityObserved {
        event_id: "round21-unauthorized-heartbeat".into(),
        node_id: node_id.clone(),
        boot_id: "round21-boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(8)),
        fresh_for_ticks: DecimalU64::new(2),
        capacity_digest: digest('d'),
    };
    assert_eq!(
        apply(&registry, &unauthorized_heartbeat, &scope, &untrusted, 8),
        Err(NodeError::ForeignScope)
    );
    assert_eq!(
        registry, unchanged,
        "rejected packet leaves registry unchanged"
    );

    let rotation = EnrollmentEvent::Rotated {
        event_id: "round21-rotate".into(),
        node_id: node_id.clone(),
        expected_generation: DecimalU64::new(1),
        new_public_key_digest: digest('e'),
    };
    registry = apply(&registry, &rotation, &scope, &controller, 8).expect("rotate node");
    events.push(rotation);
    assert!(registry.capacity.is_empty());
    let delayed_packet = EnrollmentEvent::CapacityObserved {
        event_id: "round21-delayed-heartbeat".into(),
        node_id: node_id.clone(),
        boot_id: "round21-boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(8)),
        fresh_for_ticks: DecimalU64::new(4),
        capacity_digest: digest('f'),
    };
    assert_eq!(
        apply(&registry, &delayed_packet, &scope, &controller, 9),
        Err(NodeError::StaleNode),
        "delayed heartbeat from pre-cutover generation is fenced"
    );
    let revoke = EnrollmentEvent::Revoked {
        event_id: "round21-revoke".into(),
        node_id: node_id.clone(),
        expected_generation: DecimalU64::new(2),
    };
    registry = apply(&registry, &revoke, &scope, &controller, 9).expect("revoke generation");
    events.push(revoke);
    assert!(!registry.nodes.contains_key(&node_id));
    assert!(registry.capacity.is_empty());

    let audit = HermeticReleaseAudit {
        events: &events,
        final_registry: &registry,
    };
    let bytes = write_and_read_audit(&audit_path, &audit);
    let audit_digest = content_digest(&bytes).expect("audit content digest");
    let repeat_bytes = write_and_read_audit(&audit_path, &audit);
    assert_eq!(bytes, repeat_bytes, "audit bundle bytes are repeatable");
    assert_eq!(
        content_digest(&repeat_bytes).expect("repeat audit digest"),
        audit_digest
    );

    let (manifest, observation, fence) = recovery_fixture::valid_recovery_evidence();
    let blocked = plan_recovery(&manifest, &observation, None).expect("recovery without fence");
    assert!(blocked.blockers.contains(&Blocker::MissingExternalFence));
    assert!(!blocked.resume_proposed);
    let fenced = plan_recovery(&manifest, &observation, Some(&fence)).expect("recovery with fence");
    assert!(fenced.resume_proposed);
    assert!(!fenced.restore_authorized());
    assert!(!fenced.effects_authorized());
}
