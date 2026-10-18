#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Post-PR-57 P23 release qualification and strict Clippy audit fixture.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeError, NodeId, NodeIdentity, NodeRegistry,
    NodeScope, decode_event, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId, content_digest};
use serde::Serialize;

fn digest(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

fn scope() -> NodeScope {
    NodeScope {
        project_id: ProjectId::new("round20-project").expect("project fixture"),
        root_id: RootId::new("round20-root").expect("root fixture"),
        task_id: TaskId::new("round20-task").expect("task fixture"),
        authority: PrincipalId::new("round20-controller").expect("authority fixture"),
    }
}

fn apply(
    state: &NodeRegistry,
    event: &EnrollmentEvent,
    scope: &NodeScope,
    actor: &PrincipalId,
    tick: Option<u64>,
) -> Result<NodeRegistry, NodeError> {
    reduce_enrollment_for(state, event, scope, actor, tick)
}

#[derive(Serialize)]
struct AuditRecord<'a> {
    events: &'a [EnrollmentEvent],
    final_state: &'a NodeRegistry,
}

#[test]
fn p23_round20_post_pr57_release_qualification_strict_audit() {
    let scope = scope();
    let actor = scope.authority.clone();
    let node_id = NodeId::new("round20-node").expect("node fixture");
    let issue = EnrollmentEvent::TokenIssued {
        event_id: "round20-issue".into(),
        token: EnrollmentToken {
            token_id: "round20-token".into(),
            token_digest: digest('a'),
            scope: scope.clone(),
            expires_tick: DecimalU64::new(40),
            consumed_binding: None,
        },
    };
    let issued = apply(&NodeRegistry::default(), &issue, &scope, &actor, Some(1))
        .expect("issue enrollment token");
    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "round20-enroll".into(),
        token_id: "round20-token".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: node_id.clone(),
            boot_id: "round20-boot-a".into(),
            public_key_digest: digest('b'),
            generation: DecimalU64::new(1),
            capability_digest: digest('c'),
        },
    };
    let decoded = decode_event(&serde_json::to_vec(&enrollment).expect("encode enrollment"))
        .expect("strict enrollment decode");
    let enrolled = apply(&issued, &decoded, &scope, &actor, Some(2)).expect("enroll node");
    assert_eq!(
        apply(&issued, &decoded, &scope, &actor, Some(2)).expect("exact replay"),
        enrolled
    );

    let mut accepted = vec![issue, decoded];
    let mut state = enrolled;
    for tick in 3..=9 {
        let heartbeat = EnrollmentEvent::CapacityObserved {
            event_id: format!("round20-heartbeat-{tick}"),
            node_id: node_id.clone(),
            boot_id: "round20-boot-a".into(),
            generation: DecimalU64::new(1),
            observed_tick: Some(DecimalU64::new(tick)),
            fresh_for_ticks: DecimalU64::new(3),
            capacity_digest: digest(char::from_digit(tick as u32, 16).expect("hex fixture")),
        };
        let next = apply(&state, &heartbeat, &scope, &actor, Some(tick))
            .expect("accept current heartbeat");
        assert_eq!(
            apply(&state, &heartbeat, &scope, &actor, Some(tick)).expect("replay heartbeat"),
            next
        );
        state = next;
        accepted.push(heartbeat);
    }

    let snapshot = state.clone();
    let unauthorized = apply(
        &state,
        accepted.last().expect("heartbeat history"),
        &scope,
        &actor,
        Some(8),
    );
    assert_eq!(unauthorized, Err(NodeError::Collision));
    assert_eq!(
        state, snapshot,
        "failed replay does not mutate the registry"
    );

    let rotation = EnrollmentEvent::Rotated {
        event_id: "round20-rotate".into(),
        node_id: node_id.clone(),
        expected_generation: DecimalU64::new(1),
        new_public_key_digest: digest('d'),
    };
    state = apply(&state, &rotation, &scope, &actor, Some(10)).expect("rotate node");
    accepted.push(rotation);
    assert!(
        state.capacity.is_empty(),
        "rotation clears old health evidence"
    );
    let stale_health = EnrollmentEvent::CapacityObserved {
        event_id: "round20-stale-health".into(),
        node_id: node_id.clone(),
        boot_id: "round20-boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(10)),
        fresh_for_ticks: DecimalU64::new(3),
        capacity_digest: digest('e'),
    };
    assert_eq!(
        apply(&state, &stale_health, &scope, &actor, Some(10)),
        Err(NodeError::StaleNode)
    );
    let revoke = EnrollmentEvent::Revoked {
        event_id: "round20-revoke".into(),
        node_id: node_id.clone(),
        expected_generation: DecimalU64::new(2),
    };
    state = apply(&state, &revoke, &scope, &actor, Some(11)).expect("revoke current generation");
    accepted.push(revoke);
    assert!(!state.nodes.contains_key(&node_id));
    assert!(state.capacity.is_empty());
    assert_eq!(
        state.revoked_generations.get(&node_id),
        Some(&DecimalU64::new(3))
    );

    let audit = AuditRecord {
        events: &accepted,
        final_state: &state,
    };
    let audit_bytes = serde_json::to_vec(&audit).expect("serialize release audit");
    let audit_digest = content_digest(&audit).expect("audit digest");
    let decoded_audit: AuditRecordOwned =
        serde_json::from_slice(&audit_bytes).expect("decode release audit");
    let repeated = content_digest(&decoded_audit).expect("repeat audit digest");
    assert_eq!(
        audit_digest, repeated,
        "release audit serialization is deterministic"
    );

    let mut unknown = serde_json::to_value(&stale_health).expect("health event JSON");
    unknown
        .as_object_mut()
        .expect("tagged event object")
        .insert("generation_override".into(), serde_json::json!(99));
    assert!(decode_event(&serde_json::to_vec(&unknown).expect("encode unknown field")).is_err());
}

#[derive(serde::Deserialize, Serialize)]
struct AuditRecordOwned {
    events: Vec<EnrollmentEvent>,
    final_state: NodeRegistry,
}
