//! Final deterministic end-to-end gate for P23 enrollment state.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeError, NodeId, NodeIdentity, NodeRegistry,
    NodeScope, decode_event, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::MANAGED_EXECUTION_QUALIFIED;

fn digest(ch: char) -> String {
    format!("sha256:{}", ch.to_string().repeat(64))
}

fn scope() -> NodeScope {
    NodeScope {
        project_id: ProjectId::new("round11-project").expect("project"),
        root_id: RootId::new("round11-root").expect("root"),
        task_id: TaskId::new("round11-task").expect("task"),
        authority: PrincipalId::new("round11-authority").expect("authority"),
    }
}

fn token(id: &str, expires: u64) -> EnrollmentToken {
    EnrollmentToken {
        token_id: id.into(),
        token_digest: digest('a'),
        scope: scope(),
        expires_tick: DecimalU64::new(expires),
        consumed_binding: None,
    }
}

fn identity(id: &str, boot: &str, generation: u64, key: char) -> NodeIdentity {
    NodeIdentity {
        node_id: NodeId::new(id).expect("node ID"),
        boot_id: boot.into(),
        public_key_digest: digest(key),
        generation: DecimalU64::new(generation),
        capability_digest: digest('c'),
    }
}

fn apply(
    state: &NodeRegistry,
    event: &EnrollmentEvent,
    now: Option<u64>,
) -> Result<NodeRegistry, NodeError> {
    let event_scope = match event {
        EnrollmentEvent::TokenIssued { token, .. } => token.scope.clone(),
        EnrollmentEvent::Enrolled { scope, .. } => scope.clone(),
        EnrollmentEvent::Rotated { node_id, .. }
        | EnrollmentEvent::Revoked { node_id, .. }
        | EnrollmentEvent::CapacityObserved { node_id, .. } => state
            .scopes
            .get(node_id)
            .expect("node event has registered scope")
            .clone(),
    };
    let actor = event_scope.authority.clone();
    reduce_enrollment_for(state, event, &event_scope, &actor, now)
}

#[test]
fn p23_round11_item4_final_validation_gate() {
    let initial = NodeRegistry::default();
    let issue = EnrollmentEvent::TokenIssued {
        event_id: "round11-issue-a".into(),
        token: token("round11-token-a", 20),
    };
    let issued = apply(&initial, &issue, Some(1)).expect("scoped token issue");
    assert_eq!(
        apply(&issued, &issue, Some(1)).expect("exact issue replay"),
        issued
    );
    let mut changed_issue = issue.clone();
    if let EnrollmentEvent::TokenIssued { token, .. } = &mut changed_issue {
        token.expires_tick = DecimalU64::new(21);
    }
    assert_eq!(
        apply(&issued, &changed_issue, Some(1)),
        Err(NodeError::Collision)
    );

    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "round11-enroll-a".into(),
        token_id: "round11-token-a".into(),
        scope: scope(),
        identity: identity("round11-node", "boot-a", 1, 'b'),
    };
    let wire = serde_json::to_vec(&enrollment).expect("encode enrollment wire event");
    let decoded = decode_event(&wire).expect("strict event decode");
    let enrolled = apply(&issued, &decoded, Some(2)).expect("enrollment accepted");
    assert_eq!(
        apply(&enrolled, &decoded, Some(2)).expect("exact enrollment replay"),
        enrolled
    );
    let mut with_unknown_field = serde_json::to_value(&enrollment).expect("event JSON");
    with_unknown_field
        .as_object_mut()
        .expect("event object")
        .insert("authority_override".into(), serde_json::json!(true));
    assert!(
        decode_event(&serde_json::to_vec(&with_unknown_field).expect("encode unknown field"))
            .is_err()
    );

    let node_id = NodeId::new("round11-node").expect("node ID");
    let capacity = EnrollmentEvent::CapacityObserved {
        event_id: "round11-capacity-a".into(),
        node_id: node_id.clone(),
        boot_id: "boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(3)),
        fresh_for_ticks: DecimalU64::new(4),
        capacity_digest: digest('d'),
    };
    let observed = apply(&enrolled, &capacity, Some(5)).expect("fresh capacity accepted");
    assert_eq!(
        apply(&observed, &capacity, Some(5)).expect("capacity replay"),
        observed
    );
    let stale_boot = EnrollmentEvent::CapacityObserved {
        event_id: "round11-capacity-stale-boot".into(),
        node_id: node_id.clone(),
        boot_id: "boot-before-restart".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(4)),
        fresh_for_ticks: DecimalU64::new(4),
        capacity_digest: digest('e'),
    };
    assert_eq!(
        apply(&observed, &stale_boot, Some(5)),
        Err(NodeError::StaleNode)
    );

    let rotated = apply(
        &observed,
        &EnrollmentEvent::Rotated {
            event_id: "round11-rotate".into(),
            node_id: node_id.clone(),
            expected_generation: DecimalU64::new(1),
            new_public_key_digest: digest('e'),
        },
        Some(6),
    )
    .expect("key rotation advances generation");
    assert!(
        rotated.capacity.is_empty(),
        "rotation invalidates prior capacity"
    );
    assert_eq!(rotated.nodes[&node_id].generation, DecimalU64::new(2));
    let stale_generation = EnrollmentEvent::CapacityObserved {
        event_id: "round11-capacity-old-generation".into(),
        node_id: node_id.clone(),
        boot_id: "boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(6)),
        fresh_for_ticks: DecimalU64::new(4),
        capacity_digest: digest('d'),
    };
    assert_eq!(
        apply(&rotated, &stale_generation, Some(6)),
        Err(NodeError::StaleNode)
    );

    let revoked = apply(
        &rotated,
        &EnrollmentEvent::Revoked {
            event_id: "round11-revoke".into(),
            node_id: node_id.clone(),
            expected_generation: DecimalU64::new(2),
        },
        Some(7),
    )
    .expect("current node generation revoked");
    assert!(!revoked.nodes.contains_key(&node_id));
    assert!(revoked.capacity.is_empty());
    assert_eq!(revoked.revoked_generations[&node_id], DecimalU64::new(3));
    let stale_reenrollment = EnrollmentEvent::Enrolled {
        event_id: "round11-stale-reenroll".into(),
        token_id: "round11-token-a".into(),
        scope: scope(),
        identity: identity("round11-node", "boot-a", 1, 'b'),
    };
    assert_eq!(
        apply(&revoked, &stale_reenrollment, Some(8)),
        Err(NodeError::Consumed)
    );

    // A fresh boot must use the next generation, and backward clocks fail closed.
    let issued_b = apply(
        &revoked,
        &EnrollmentEvent::TokenIssued {
            event_id: "round11-issue-b".into(),
            token: token("round11-token-b", 30),
        },
        Some(8),
    )
    .expect("fresh token after revocation");
    let rebooted = apply(
        &issued_b,
        &EnrollmentEvent::Enrolled {
            event_id: "round11-enroll-b".into(),
            token_id: "round11-token-b".into(),
            scope: scope(),
            identity: identity("round11-node", "boot-b", 3, 'f'),
        },
        Some(9),
    )
    .expect("reboot enrolls at next generation");
    assert_eq!(rebooted.nodes[&node_id].generation, DecimalU64::new(3));
    let rollback = EnrollmentEvent::TokenIssued {
        event_id: "round11-rollback-issue".into(),
        token: token("round11-rollback", 40),
    };
    assert!(matches!(
        apply(&rebooted, &rollback, Some(8)),
        Err(NodeError::Invalid("clock rollback"))
    ));
    assert!(!rebooted.tokens.contains_key("round11-rollback"));

    // This reducer gate does not qualify an authenticated node transport or runner.
    assert!(!MANAGED_EXECUTION_QUALIFIED);
}
