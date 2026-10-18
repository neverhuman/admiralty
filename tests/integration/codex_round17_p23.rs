//! Post-Item-5 release audit for P23 node state and cross-packet authority boundaries.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeError, NodeId, NodeIdentity, NodeRegistry,
    NodeScope, decode_event, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::{
    MANAGED_EXECUTION_QUALIFIED,
    recovery::{Blocker, plan_recovery},
};

#[path = "support/recovery.rs"]
mod recovery_fixture;

fn digest(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

fn scope() -> NodeScope {
    NodeScope {
        project_id: ProjectId::new("round17-project").expect("project fixture"),
        root_id: RootId::new("round17-root").expect("root fixture"),
        task_id: TaskId::new("round17-task").expect("task fixture"),
        authority: PrincipalId::new("round17-controller").expect("authority fixture"),
    }
}

fn apply(
    state: &NodeRegistry,
    event: &EnrollmentEvent,
    scope: &NodeScope,
    actor: &PrincipalId,
    now: Option<u64>,
) -> Result<NodeRegistry, NodeError> {
    reduce_enrollment_for(state, event, scope, actor, now)
}

#[test]
fn p23_round17_post_item5_release_audit_cross_packet_invariant_qualification() {
    const { assert!(!MANAGED_EXECUTION_QUALIFIED) };

    let scope = scope();
    let controller = scope.authority.clone();
    let foreign = PrincipalId::new("round17-foreign").expect("foreign principal fixture");
    let node_id = NodeId::new("round17-node").expect("node fixture");
    let empty = NodeRegistry::default();
    let issue = EnrollmentEvent::TokenIssued {
        event_id: "round17-issue".into(),
        token: EnrollmentToken {
            token_id: "round17-token".into(),
            token_digest: digest('a'),
            scope: scope.clone(),
            expires_tick: DecimalU64::new(50),
            consumed_binding: None,
        },
    };
    let issued = apply(&empty, &issue, &scope, &controller, Some(1)).expect("issue token");
    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "round17-enroll".into(),
        token_id: "round17-token".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: node_id.clone(),
            boot_id: "round17-boot-a".into(),
            public_key_digest: digest('b'),
            generation: DecimalU64::new(1),
            capability_digest: digest('c'),
        },
    };
    let wire = serde_json::to_vec(&enrollment).expect("serialize enrollment");
    let enrollment = decode_event(&wire).expect("strict enrollment decode");
    let enrolled = apply(&issued, &enrollment, &scope, &controller, Some(2)).expect("enroll");
    let baseline = enrolled.clone();
    assert_eq!(
        apply(&issued, &enrollment, &scope, &controller, Some(2)).expect("replay enrollment"),
        baseline,
        "event replay reproduces the same registry projection"
    );

    let mut replay = enrolled;
    for tick in 3..=7 {
        let health = EnrollmentEvent::CapacityObserved {
            event_id: format!("round17-health-{tick}"),
            node_id: node_id.clone(),
            boot_id: "round17-boot-a".into(),
            generation: DecimalU64::new(1),
            observed_tick: Some(DecimalU64::new(tick)),
            fresh_for_ticks: DecimalU64::new(3),
            capacity_digest: digest(char::from_digit(tick as u32, 16).expect("hex digest seed")),
        };
        let next =
            apply(&replay, &health, &scope, &controller, Some(tick)).expect("health accepted");
        assert_eq!(
            apply(&replay, &health, &scope, &controller, Some(tick)).expect("health replay"),
            next,
            "health ingestion is deterministic across event replay"
        );
        replay = next;
    }

    let before_rejection = replay.clone();
    let unauthorized = EnrollmentEvent::CapacityObserved {
        event_id: "round17-foreign-health".into(),
        node_id: node_id.clone(),
        boot_id: "round17-boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(8)),
        fresh_for_ticks: DecimalU64::new(2),
        capacity_digest: digest('d'),
    };
    assert_eq!(
        apply(&replay, &unauthorized, &scope, &foreign, Some(8)),
        Err(NodeError::ForeignScope)
    );
    assert_eq!(
        replay, before_rejection,
        "rejection leaves the accepted projection unchanged"
    );
    let clock_rollback = EnrollmentEvent::TokenIssued {
        event_id: "round17-clock-rollback".into(),
        token: EnrollmentToken {
            token_id: "round17-rollback-token".into(),
            token_digest: digest('e'),
            scope: scope.clone(),
            expires_tick: DecimalU64::new(50),
            consumed_binding: None,
        },
    };
    assert!(matches!(
        apply(&replay, &clock_rollback, &scope, &controller, Some(6)),
        Err(NodeError::Invalid("clock rollback"))
    ));
    assert_eq!(
        replay, before_rejection,
        "clock failure does not mutate prior state"
    );

    let rotated = apply(
        &replay,
        &EnrollmentEvent::Rotated {
            event_id: "round17-rotate".into(),
            node_id: node_id.clone(),
            expected_generation: DecimalU64::new(1),
            new_public_key_digest: digest('f'),
        },
        &scope,
        &controller,
        Some(8),
    )
    .expect("rotate node credentials");
    assert!(rotated.capacity.is_empty());
    assert_eq!(
        rotated.nodes.get(&node_id).map(|node| &node.generation),
        Some(&DecimalU64::new(2))
    );
    let stale_health = EnrollmentEvent::CapacityObserved {
        event_id: "round17-old-generation-health".into(),
        node_id: node_id.clone(),
        boot_id: "round17-boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(8)),
        fresh_for_ticks: DecimalU64::new(2),
        capacity_digest: digest('d'),
    };
    assert_eq!(
        apply(&rotated, &stale_health, &scope, &controller, Some(8)),
        Err(NodeError::StaleNode),
        "the generation fence rejects packets from before cutover"
    );
    let revoked = apply(
        &rotated,
        &EnrollmentEvent::Revoked {
            event_id: "round17-revoke".into(),
            node_id: node_id.clone(),
            expected_generation: DecimalU64::new(2),
        },
        &scope,
        &controller,
        Some(9),
    )
    .expect("revoke current generation");
    assert!(!revoked.nodes.contains_key(&node_id));
    assert!(revoked.capacity.is_empty());
    assert_eq!(
        revoked.revoked_generations.get(&node_id),
        Some(&DecimalU64::new(3))
    );

    // P25 recovery remains a separate proposal boundary and cannot authorize effects.
    let (manifest, observation, fence) = recovery_fixture::valid_recovery_evidence();
    let blocked = plan_recovery(&manifest, &observation, None).expect("blocked recovery plan");
    assert!(blocked.blockers.contains(&Blocker::MissingExternalFence));
    assert!(!blocked.resume_proposed);
    let fenced =
        plan_recovery(&manifest, &observation, Some(&fence)).expect("fenced recovery plan");
    assert!(fenced.resume_proposed);
    assert!(!fenced.restore_authorized());
    assert!(!fenced.effects_authorized());
}
