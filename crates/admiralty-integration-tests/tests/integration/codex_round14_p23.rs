//! Deterministic release readiness qualification for P23 node state.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeError, NodeId, NodeIdentity, NodeRegistry,
    NodeScope, decode_event, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};

fn digest(ch: char) -> String {
    format!("sha256:{}", ch.to_string().repeat(64))
}

fn scope() -> NodeScope {
    NodeScope {
        project_id: ProjectId::new("round14-project").expect("valid project ID"),
        root_id: RootId::new("round14-root").expect("valid root ID"),
        task_id: TaskId::new("round14-task").expect("valid task ID"),
        authority: PrincipalId::new("round14-controller").expect("valid authority ID"),
    }
}

fn apply(
    state: &NodeRegistry,
    event: &EnrollmentEvent,
    event_scope: &NodeScope,
    actor: &PrincipalId,
    now: Option<u64>,
) -> Result<NodeRegistry, NodeError> {
    reduce_enrollment_for(state, event, event_scope, actor, now)
}

fn capacity_event(event_id: &str, node_id: &NodeId) -> EnrollmentEvent {
    EnrollmentEvent::CapacityObserved {
        event_id: event_id.into(),
        node_id: node_id.clone(),
        boot_id: "boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(3)),
        fresh_for_ticks: DecimalU64::new(4),
        capacity_digest: digest('d'),
    }
}

#[test]
fn codex_round14_p23() {
    let scope = scope();
    let actor = scope.authority.clone();
    let initial = NodeRegistry::default();
    let issue = EnrollmentEvent::TokenIssued {
        event_id: "r14-issue".into(),
        token: EnrollmentToken {
            token_id: "r14-token".into(),
            token_digest: digest('a'),
            scope: scope.clone(),
            expires_tick: DecimalU64::new(20),
            consumed_binding: None,
        },
    };

    let issued = apply(&initial, &issue, &scope, &actor, Some(1)).expect("issue token");
    assert_eq!(
        apply(&initial, &issue, &scope, &actor, Some(1)).expect("replay issue"),
        issued,
        "replaying an identical event from the same state is deterministic"
    );
    let mut collision = issue.clone();
    if let EnrollmentEvent::TokenIssued { token, .. } = &mut collision {
        token.expires_tick = DecimalU64::new(21);
    }
    assert_eq!(
        apply(&issued, &collision, &scope, &actor, Some(1)),
        Err(NodeError::Collision),
        "a reused event ID with changed contents fails closed"
    );

    let enrolled_event = EnrollmentEvent::Enrolled {
        event_id: "r14-enroll".into(),
        token_id: "r14-token".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: NodeId::new("r14-node").expect("valid node ID"),
            boot_id: "boot-a".into(),
            public_key_digest: digest('b'),
            generation: DecimalU64::new(1),
            capability_digest: digest('c'),
        },
    };
    let wire = serde_json::to_vec(&enrolled_event).expect("serialize event");
    let decoded = decode_event(&wire).expect("strictly decode event");
    let enrolled = apply(&issued, &decoded, &scope, &actor, Some(2)).expect("enroll node");
    assert_eq!(
        apply(&issued, &decoded, &scope, &actor, Some(2)).expect("replay enrollment"),
        enrolled
    );
    let mut unknown_field = serde_json::to_value(&decoded).expect("event JSON");
    unknown_field
        .as_object_mut()
        .expect("event is an object")
        .insert("controller_override".into(), serde_json::json!(true));
    let unknown_wire = serde_json::to_vec(&unknown_field).expect("serialize unknown field");
    assert!(
        decode_event(&unknown_wire).is_err(),
        "unknown fields are rejected"
    );

    let node_id = NodeId::new("r14-node").expect("valid node ID");
    let telemetry = capacity_event("r14-telemetry", &node_id);
    let observed = apply(&enrolled, &telemetry, &scope, &actor, Some(5)).expect("accept telemetry");
    assert_eq!(
        apply(&enrolled, &telemetry, &scope, &actor, Some(5)).expect("replay telemetry"),
        observed
    );
    let stale_boot = EnrollmentEvent::CapacityObserved {
        event_id: "r14-stale-boot".into(),
        node_id: node_id.clone(),
        boot_id: "previous-boot".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(4)),
        fresh_for_ticks: DecimalU64::new(4),
        capacity_digest: digest('e'),
    };
    assert_eq!(
        apply(&observed, &stale_boot, &scope, &actor, Some(5)),
        Err(NodeError::StaleNode),
        "telemetry from a previous boot is rejected"
    );

    let rotated = apply(
        &observed,
        &EnrollmentEvent::Rotated {
            event_id: "r14-rotate".into(),
            node_id: node_id.clone(),
            expected_generation: DecimalU64::new(1),
            new_public_key_digest: digest('e'),
        },
        &scope,
        &actor,
        Some(6),
    )
    .expect("rotate node key");
    assert_eq!(
        rotated.nodes.get(&node_id).map(|n| &n.generation),
        Some(&DecimalU64::new(2))
    );
    assert!(
        rotated.capacity.is_empty(),
        "rotation fences prior telemetry"
    );
    assert_eq!(
        apply(
            &rotated,
            &capacity_event("r14-post-rotation-stale", &node_id),
            &scope,
            &actor,
            Some(6)
        ),
        Err(NodeError::StaleNode),
        "pre-rotation generation is fenced"
    );

    let revoked = apply(
        &rotated,
        &EnrollmentEvent::Revoked {
            event_id: "r14-revoke".into(),
            node_id: node_id.clone(),
            expected_generation: DecimalU64::new(2),
        },
        &scope,
        &actor,
        Some(7),
    )
    .expect("revoke node");
    assert!(!revoked.nodes.contains_key(&node_id));
    assert_eq!(
        apply(
            &revoked,
            &capacity_event("r14-post-revocation-stale", &node_id),
            &scope,
            &actor,
            Some(8)
        ),
        Err(NodeError::StaleNode),
        "revocation rejects later node telemetry"
    );
    let foreign_actor = PrincipalId::new("untrusted-controller").expect("valid principal ID");
    assert_eq!(
        apply(&initial, &issue, &scope, &foreign_actor, Some(1)),
        Err(NodeError::ForeignScope),
        "foreign authenticated actors fail closed"
    );
}
