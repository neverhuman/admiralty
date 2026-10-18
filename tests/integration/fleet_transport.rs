//! P23 acceptance harness for the node enrollment API currently present on main.
//! The authenticated transport and durable transfer modules are not implemented yet;
//! these checks cover the real enrollment reducer and preserve the disabled runner gate.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeError, NodeId, NodeIdentity, NodeRegistry,
    NodeScope, decode_event, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::MANAGED_EXECUTION_QUALIFIED;

fn digest(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

fn scope() -> NodeScope {
    NodeScope {
        project_id: ProjectId::new("project-p23").expect("project fixture"),
        root_id: RootId::new("root-p23").expect("root fixture"),
        task_id: TaskId::new("task-p23").expect("task fixture"),
        authority: PrincipalId::new("captain-p23").expect("authority fixture"),
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
        node_id: NodeId::new(id).expect("node ID fixture"),
        boot_id: boot.into(),
        public_key_digest: digest(key),
        generation: DecimalU64::new(generation),
        capability_digest: digest('b'),
    }
}

fn apply(
    state: &NodeRegistry,
    event: &EnrollmentEvent,
    now: Option<u64>,
) -> Result<NodeRegistry, NodeError> {
    let admission_scope = match event {
        EnrollmentEvent::TokenIssued { token, .. } => token.scope.clone(),
        EnrollmentEvent::Enrolled { scope, .. } => scope.clone(),
        EnrollmentEvent::Rotated { node_id, .. }
        | EnrollmentEvent::Revoked { node_id, .. }
        | EnrollmentEvent::CapacityObserved { node_id, .. } => state
            .scopes
            .get(node_id)
            .expect("enrolled node has an authority scope")
            .clone(),
    };
    let actor = admission_scope.authority.clone();
    reduce_enrollment_for(state, event, &admission_scope, &actor, now)
}

fn issued_token(id: &str, expires: u64, now: u64) -> NodeRegistry {
    apply(
        &NodeRegistry::default(),
        &EnrollmentEvent::TokenIssued {
            event_id: format!("issue-{id}"),
            token: token(id, expires),
        },
        Some(now),
    )
    .expect("token issue is accepted in scope")
}

#[test]
fn enrollment_exact_replay_is_idempotent_and_changed_binding_fails_closed() {
    let start = issued_token("once", 20, 1);
    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "enroll-once".into(),
        token_id: "once".into(),
        scope: scope(),
        identity: identity("node-a", "boot-a", 1, 'c'),
    };

    let enrolled = apply(&start, &enrollment, Some(2)).expect("first enrollment");
    assert_eq!(
        apply(&enrolled, &enrollment, Some(2)).expect("exact replay"),
        enrolled
    );

    let changed_event = EnrollmentEvent::Enrolled {
        event_id: "enroll-once".into(),
        token_id: "once".into(),
        scope: scope(),
        identity: identity("node-a", "boot-a", 1, 'd'),
    };
    assert_eq!(
        apply(&enrolled, &changed_event, Some(2)),
        Err(NodeError::Collision)
    );

    let changed_binding = EnrollmentEvent::Enrolled {
        event_id: "enroll-different-binding".into(),
        token_id: "once".into(),
        scope: scope(),
        identity: identity("node-b", "boot-b", 1, 'd'),
    };
    assert_eq!(
        apply(&enrolled, &changed_binding, Some(3)),
        Err(NodeError::Consumed)
    );
}

#[test]
fn expiry_boundary_unknown_clock_and_foreign_authority_are_rejected() {
    let start = issued_token("expires", 4, 1);
    let expired = EnrollmentEvent::Enrolled {
        event_id: "enroll-expired".into(),
        token_id: "expires".into(),
        scope: scope(),
        identity: identity("node-expired", "boot-1", 1, 'c'),
    };
    assert_eq!(
        apply(&start, &expired, Some(4)),
        Err(NodeError::TokenExpired)
    );
    assert_eq!(apply(&start, &expired, None), Err(NodeError::TokenExpired));

    let event = EnrollmentEvent::TokenIssued {
        event_id: "foreign-issue".into(),
        token: token("foreign", 20),
    };
    let other_actor = PrincipalId::new("different-captain").expect("foreign actor fixture");
    assert_eq!(
        reduce_enrollment_for(
            &NodeRegistry::default(),
            &event,
            &scope(),
            &other_actor,
            Some(1)
        ),
        Err(NodeError::ForeignScope)
    );
}

#[test]
fn clock_rollback_cannot_replay_an_unexpired_enrollment_token() {
    let start = issued_token("rollback", 20, 5);
    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "enroll-after-rollback".into(),
        token_id: "rollback".into(),
        scope: scope(),
        identity: identity("node-rollback", "boot-1", 1, 'c'),
    };

    // An otherwise valid token must not admit a node when the controller's
    // explicit clock moves behind the last durable enrollment event.
    assert!(matches!(
        apply(&start, &enrollment, Some(4)),
        Err(NodeError::Invalid("clock rollback"))
    ));
    assert!(
        !start
            .nodes
            .contains_key(&NodeId::new("node-rollback").expect("node ID fixture"))
    );
    assert!(start.tokens["rollback"].consumed_binding.is_none());
}

#[test]
fn clock_rollback_cannot_issue_a_new_enrollment_token() {
    let start = issued_token("clock-anchor", 30, 5);
    let rollback = EnrollmentEvent::TokenIssued {
        event_id: "issue-after-rollback".into(),
        token: token("rollback-issued", 20),
    };

    assert!(matches!(
        apply(&start, &rollback, Some(4)),
        Err(NodeError::Invalid("clock rollback"))
    ));
    assert!(!start.tokens.contains_key("rollback-issued"));
}

#[test]
fn replaying_an_event_id_at_a_different_tick_fails_closed() {
    let start = issued_token("tick-replay", 20, 1);
    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "enroll-tick-replay".into(),
        token_id: "tick-replay".into(),
        scope: scope(),
        identity: identity("node-tick-replay", "boot-1", 1, 'c'),
    };
    let accepted = apply(&start, &enrollment, Some(2)).expect("initial enrollment");

    assert_eq!(
        apply(&accepted, &enrollment, Some(3)),
        Err(NodeError::Collision)
    );
    assert!(
        accepted
            .nodes
            .contains_key(&NodeId::new("node-tick-replay").expect("node ID fixture"))
    );
}

#[test]
fn token_issue_replay_is_idempotent_and_mutated_payload_collides() {
    let event = EnrollmentEvent::TokenIssued {
        event_id: "issue-idempotent-token".into(),
        token: token("idempotent-token", 20),
    };
    let accepted = apply(&NodeRegistry::default(), &event, Some(1)).expect("token issue");

    assert_eq!(
        apply(&accepted, &event, Some(1)).expect("exact issue replay"),
        accepted
    );
    let changed_payload = EnrollmentEvent::TokenIssued {
        event_id: "issue-idempotent-token".into(),
        token: token("idempotent-token", 21),
    };
    assert_eq!(
        apply(&accepted, &changed_payload, Some(1)),
        Err(NodeError::Collision)
    );
    assert_eq!(
        accepted.tokens["idempotent-token"].expires_tick,
        DecimalU64::new(20)
    );
}

#[test]
fn reboot_rotation_revocation_and_capacity_require_current_generation() {
    let start = issued_token("boot-a-token", 30, 1);
    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "enroll-boot-a".into(),
        token_id: "boot-a-token".into(),
        scope: scope(),
        identity: identity("node-a", "boot-a", 1, 'c'),
    };
    let enrolled = apply(&start, &enrollment, Some(2)).expect("enroll boot a");

    let observation = EnrollmentEvent::CapacityObserved {
        event_id: "capacity-boot-a".into(),
        node_id: NodeId::new("node-a").expect("node ID fixture"),
        boot_id: "boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(3)),
        fresh_for_ticks: DecimalU64::new(2),
        capacity_digest: digest('d'),
    };
    let observed = apply(&enrolled, &observation, Some(4)).expect("fresh capacity observation");

    let stale_boot = EnrollmentEvent::CapacityObserved {
        event_id: "capacity-old-boot".into(),
        node_id: NodeId::new("node-a").expect("node ID fixture"),
        boot_id: "boot-before-reboot".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(4)),
        fresh_for_ticks: DecimalU64::new(2),
        capacity_digest: digest('e'),
    };
    assert_eq!(
        apply(&observed, &stale_boot, Some(5)),
        Err(NodeError::StaleNode)
    );

    let rotated = apply(
        &observed,
        &EnrollmentEvent::Rotated {
            event_id: "rotate-node-a".into(),
            node_id: NodeId::new("node-a").expect("node ID fixture"),
            expected_generation: DecimalU64::new(1),
            new_public_key_digest: digest('e'),
        },
        Some(5),
    )
    .expect("key rotation");
    assert!(
        rotated.capacity.is_empty(),
        "rotation invalidates old capacity"
    );
    assert_eq!(
        rotated.nodes[&NodeId::new("node-a").expect("node ID fixture")]
            .generation
            .parse()
            .expect("generation is canonical"),
        2
    );

    let revoked = apply(
        &rotated,
        &EnrollmentEvent::Revoked {
            event_id: "revoke-node-a".into(),
            node_id: NodeId::new("node-a").expect("node ID fixture"),
            expected_generation: DecimalU64::new(2),
        },
        Some(6),
    )
    .expect("revoke current generation");
    assert!(
        !revoked
            .nodes
            .contains_key(&NodeId::new("node-a").expect("node ID fixture"))
    );
    assert!(revoked.capacity.is_empty());
    assert_eq!(
        revoked.revoked_generations[&NodeId::new("node-a").expect("node ID fixture")]
            .parse()
            .expect("revoked generation is canonical"),
        3
    );

    let reissued = apply(
        &revoked,
        &EnrollmentEvent::TokenIssued {
            event_id: "issue-boot-b-token".into(),
            token: token("boot-b-token", 30),
        },
        Some(7),
    )
    .expect("issue new boot token");
    let rebooted = apply(
        &reissued,
        &EnrollmentEvent::Enrolled {
            event_id: "enroll-boot-b".into(),
            token_id: "boot-b-token".into(),
            scope: scope(),
            identity: identity("node-a", "boot-b", 3, 'f'),
        },
        Some(8),
    )
    .expect("re-enroll at next generation after reboot");
    let old_generation = EnrollmentEvent::CapacityObserved {
        event_id: "capacity-old-generation".into(),
        node_id: NodeId::new("node-a").expect("node ID fixture"),
        boot_id: "boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(8)),
        fresh_for_ticks: DecimalU64::new(10),
        capacity_digest: digest('d'),
    };
    assert_eq!(
        apply(&rebooted, &old_generation, Some(9)),
        Err(NodeError::StaleNode)
    );
}

#[test]
fn strict_event_decoder_rejects_unknown_fields() {
    let event = EnrollmentEvent::TokenIssued {
        event_id: "issue-decode".into(),
        token: token("decode", 20),
    };
    let mut value = serde_json::to_value(event).expect("serialize event fixture");
    value.as_object_mut().expect("event is an object").insert(
        "unexpected_authority_override".into(),
        serde_json::json!(true),
    );
    let bytes = serde_json::to_vec(&value).expect("serialize unknown-field fixture");
    assert!(decode_event(&bytes).is_err(), "unknown fields fail closed");
}

#[test]
fn node_runner_remains_unqualified_without_authenticated_transport() {
    const { assert!(!MANAGED_EXECUTION_QUALIFIED) };
}
