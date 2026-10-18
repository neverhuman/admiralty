//! End-to-end P23 cutover and fault-containment qualification.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeError, NodeId, NodeIdentity, NodeRegistry,
    NodeScope, decode_event, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};

fn digest(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

fn scope() -> NodeScope {
    NodeScope {
        project_id: ProjectId::new("round16-project").expect("project ID fixture"),
        root_id: RootId::new("round16-root").expect("root ID fixture"),
        task_id: TaskId::new("round16-task").expect("task ID fixture"),
        authority: PrincipalId::new("round16-controller").expect("authority fixture"),
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

fn token(id: &str, scope: &NodeScope, expires_tick: u64) -> EnrollmentToken {
    EnrollmentToken {
        token_id: id.into(),
        token_digest: digest('a'),
        scope: scope.clone(),
        expires_tick: DecimalU64::new(expires_tick),
        consumed_binding: None,
    }
}

#[test]
fn p23_round16_end_to_end_cutover_fault_containment() {
    let scope = scope();
    let actor = scope.authority.clone();
    let rogue = PrincipalId::new("round16-rogue").expect("principal fixture");
    let node_id = NodeId::new("round16-node").expect("node ID fixture");
    let initial = NodeRegistry::default();

    // Enrollment requires a known clock and the independently authenticated actor.
    let issue = EnrollmentEvent::TokenIssued {
        event_id: "round16-issue-a".into(),
        token: token("round16-token-a", &scope, 30),
    };
    assert_eq!(
        apply(&initial, &issue, &scope, &actor, None),
        Err(NodeError::Invalid("clock unknown"))
    );
    assert_eq!(
        apply(&initial, &issue, &scope, &rogue, Some(1)),
        Err(NodeError::ForeignScope)
    );
    let issued = apply(&initial, &issue, &scope, &actor, Some(1)).expect("issue token");
    let expired = EnrollmentEvent::TokenIssued {
        event_id: "round16-issue-expired".into(),
        token: token("round16-token-expired", &scope, 2),
    };
    assert_eq!(
        apply(&issued, &expired, &scope, &actor, Some(2)),
        Err(NodeError::TokenExpired),
        "expiry equality is closed"
    );

    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "round16-enroll-a".into(),
        token_id: "round16-token-a".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: node_id.clone(),
            boot_id: "round16-boot-a".into(),
            public_key_digest: digest('b'),
            generation: DecimalU64::new(1),
            capability_digest: digest('c'),
        },
    };
    let encoded = serde_json::to_vec(&enrollment).expect("encode enrollment");
    let decoded = decode_event(&encoded).expect("decode enrollment");
    let mut forged_wire = serde_json::to_value(&enrollment).expect("event JSON");
    forged_wire
        .as_object_mut()
        .expect("event object")
        .insert("generation_override".into(), serde_json::json!(9));
    assert!(decode_event(&serde_json::to_vec(&forged_wire).expect("encode forged event")).is_err());

    let enrolled = apply(&issued, &decoded, &scope, &actor, Some(3)).expect("enroll node");
    assert_eq!(
        apply(&issued, &decoded, &scope, &actor, Some(3)).expect("replay enrollment"),
        enrolled
    );
    let mut changed_replay = decoded.clone();
    if let EnrollmentEvent::Enrolled { identity, .. } = &mut changed_replay {
        identity.capability_digest = digest('d');
    }
    assert_eq!(
        apply(&enrolled, &changed_replay, &scope, &actor, Some(3)),
        Err(NodeError::Collision),
        "event identity reuse with altered content is contained"
    );

    // A multi-sample health stream is replay-stable, but gaps in trust fail closed.
    let mut healthy = enrolled.clone();
    for tick in 4..=8 {
        let sample = EnrollmentEvent::CapacityObserved {
            event_id: format!("round16-health-{tick}"),
            node_id: node_id.clone(),
            boot_id: "round16-boot-a".into(),
            generation: DecimalU64::new(1),
            observed_tick: Some(DecimalU64::new(tick)),
            fresh_for_ticks: DecimalU64::new(2),
            capacity_digest: digest(char::from_digit(tick as u32, 16).expect("hex seed")),
        };
        let next = apply(&healthy, &sample, &scope, &actor, Some(tick)).expect("accept health");
        assert_eq!(
            apply(&healthy, &sample, &scope, &actor, Some(tick)).expect("replay health"),
            next
        );
        healthy = next;
    }
    let bad_digest = EnrollmentEvent::CapacityObserved {
        event_id: "round16-health-malformed".into(),
        node_id: node_id.clone(),
        boot_id: "round16-boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(9)),
        fresh_for_ticks: DecimalU64::new(2),
        capacity_digest: "sha256:NOT-A-DIGEST".into(),
    };
    assert!(matches!(
        apply(&healthy, &bad_digest, &scope, &actor, Some(9)),
        Err(NodeError::Invalid(_))
    ));
    let future_health = EnrollmentEvent::CapacityObserved {
        event_id: "round16-health-future".into(),
        node_id: node_id.clone(),
        boot_id: "round16-boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(10)),
        fresh_for_ticks: DecimalU64::new(1),
        capacity_digest: digest('e'),
    };
    assert!(matches!(
        apply(&healthy, &future_health, &scope, &actor, Some(9)),
        Err(NodeError::Invalid(_))
    ));

    // Rotation cuts over the generation atomically and fences old boot/generation reports.
    let rotated = apply(
        &healthy,
        &EnrollmentEvent::Rotated {
            event_id: "round16-rotate".into(),
            node_id: node_id.clone(),
            expected_generation: DecimalU64::new(1),
            new_public_key_digest: digest('f'),
        },
        &scope,
        &actor,
        Some(9),
    )
    .expect("rotate enrollment identity");
    assert!(
        rotated.capacity.is_empty(),
        "cutover clears old health observations"
    );
    assert_eq!(
        rotated.nodes.get(&node_id).map(|n| &n.generation),
        Some(&DecimalU64::new(2))
    );
    let stale_generation = EnrollmentEvent::CapacityObserved {
        event_id: "round16-health-old-generation".into(),
        node_id: node_id.clone(),
        boot_id: "round16-boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(9)),
        fresh_for_ticks: DecimalU64::new(2),
        capacity_digest: digest('e'),
    };
    assert_eq!(
        apply(&rotated, &stale_generation, &scope, &actor, Some(9)),
        Err(NodeError::StaleNode)
    );
    let stale_boot = EnrollmentEvent::CapacityObserved {
        event_id: "round16-health-old-boot".into(),
        node_id: node_id.clone(),
        boot_id: "round16-boot-before-cutover".into(),
        generation: DecimalU64::new(2),
        observed_tick: Some(DecimalU64::new(10)),
        fresh_for_ticks: DecimalU64::new(2),
        capacity_digest: digest('e'),
    };
    assert_eq!(
        apply(&rotated, &stale_boot, &scope, &actor, Some(10)),
        Err(NodeError::StaleNode)
    );

    let revoked = apply(
        &rotated,
        &EnrollmentEvent::Revoked {
            event_id: "round16-revoke".into(),
            node_id: node_id.clone(),
            expected_generation: DecimalU64::new(2),
        },
        &scope,
        &actor,
        Some(10),
    )
    .expect("revoke cutover generation");
    assert!(!revoked.nodes.contains_key(&node_id));
    assert!(revoked.capacity.is_empty());
    assert_eq!(
        revoked.revoked_generations.get(&node_id),
        Some(&DecimalU64::new(3))
    );

    // Only the fenced next generation can re-enroll after revocation.
    let issue_b = EnrollmentEvent::TokenIssued {
        event_id: "round16-issue-b".into(),
        token: token("round16-token-b", &scope, 30),
    };
    let issued_b =
        apply(&revoked, &issue_b, &scope, &actor, Some(11)).expect("issue cutover token");
    let stale_reenrollment = EnrollmentEvent::Enrolled {
        event_id: "round16-stale-reenroll".into(),
        token_id: "round16-token-b".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: node_id.clone(),
            boot_id: "round16-boot-a".into(),
            public_key_digest: digest('f'),
            generation: DecimalU64::new(2),
            capability_digest: digest('c'),
        },
    };
    assert_eq!(
        apply(&issued_b, &stale_reenrollment, &scope, &actor, Some(12)),
        Err(NodeError::StaleNode)
    );
    let cutover = EnrollmentEvent::Enrolled {
        event_id: "round16-enroll-b".into(),
        token_id: "round16-token-b".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: node_id.clone(),
            boot_id: "round16-boot-b".into(),
            public_key_digest: digest('f'),
            generation: DecimalU64::new(3),
            capability_digest: digest('c'),
        },
    };
    let cutover_state =
        apply(&issued_b, &cutover, &scope, &actor, Some(12)).expect("fresh cutover enroll");
    assert_eq!(
        cutover_state
            .nodes
            .get(&node_id)
            .map(|n| n.generation.parse()),
        Some(Ok(3))
    );
    assert!(cutover_state.capacity.is_empty());
}
