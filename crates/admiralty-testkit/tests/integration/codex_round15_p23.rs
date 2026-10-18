//! Round 15 release-smoke and multi-step deterministic P23 qualification.

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
        project_id: ProjectId::new("round15-project").expect("project fixture"),
        root_id: RootId::new("round15-root").expect("root fixture"),
        task_id: TaskId::new("round15-task").expect("task fixture"),
        authority: PrincipalId::new("round15-authority").expect("authority fixture"),
    }
}

fn reduce(
    state: &NodeRegistry,
    event: &EnrollmentEvent,
    scope: &NodeScope,
    actor: &PrincipalId,
    now: Option<u64>,
) -> Result<NodeRegistry, NodeError> {
    reduce_enrollment_for(state, event, scope, actor, now)
}

#[test]
fn p23_round15_post_item4_release_smoke_multi_step_stress() {
    let scope = scope();
    let authority = scope.authority.clone();
    let wrong_authority = PrincipalId::new("round15-untrusted").expect("principal fixture");
    let initial = NodeRegistry::default();
    let token = EnrollmentToken {
        token_id: "round15-token".into(),
        token_digest: digest('a'),
        scope: scope.clone(),
        expires_tick: DecimalU64::new(100),
        consumed_binding: None,
    };
    let issue = EnrollmentEvent::TokenIssued {
        event_id: "round15-issue".into(),
        token,
    };
    assert_eq!(
        reduce(&initial, &issue, &scope, &authority, None),
        Err(NodeError::Invalid("clock unknown")),
        "token admission without an explicit clock fails closed"
    );
    let issued = reduce(&initial, &issue, &scope, &authority, Some(1)).expect("issue scoped token");
    let node_id = NodeId::new("round15-node").expect("node fixture");
    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "round15-enroll".into(),
        token_id: "round15-token".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: node_id.clone(),
            boot_id: "boot-round15".into(),
            public_key_digest: digest('b'),
            generation: DecimalU64::new(1),
            capability_digest: digest('c'),
        },
    };
    let encoded = serde_json::to_vec(&enrollment).expect("encode enrollment event");
    let decoded = decode_event(&encoded).expect("decode strict enrollment event");
    let enrolled = reduce(&issued, &decoded, &scope, &authority, Some(2)).expect("enroll node");
    assert_eq!(
        reduce(&issued, &decoded, &scope, &authority, Some(2)).expect("replay enrollment"),
        enrolled,
        "replaying enrollment from the same state has identical output"
    );
    assert_eq!(
        reduce(&issued, &decoded, &scope, &wrong_authority, Some(2)),
        Err(NodeError::ForeignScope),
        "authenticated scope mismatch is rejected"
    );

    let mut replay = enrolled.clone();
    for tick in 3..=12 {
        let event = EnrollmentEvent::CapacityObserved {
            event_id: format!("round15-health-{tick}"),
            node_id: node_id.clone(),
            boot_id: "boot-round15".into(),
            generation: DecimalU64::new(1),
            observed_tick: Some(DecimalU64::new(tick)),
            fresh_for_ticks: DecimalU64::new(3),
            capacity_digest: digest(
                char::from_digit(tick as u32, 16).expect("lowercase hexadecimal digest seed"),
            ),
        };
        let next =
            reduce(&replay, &event, &scope, &authority, Some(tick)).expect("record health sample");
        assert_eq!(
            reduce(&replay, &event, &scope, &authority, Some(tick)).expect("replay health sample"),
            next,
            "each health sample replays deterministically"
        );
        replay = next;
    }

    let stale = EnrollmentEvent::CapacityObserved {
        event_id: "round15-health-stale".into(),
        node_id: node_id.clone(),
        boot_id: "boot-old".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(13)),
        fresh_for_ticks: DecimalU64::new(2),
        capacity_digest: digest('d'),
    };
    assert_eq!(
        reduce(&replay, &stale, &scope, &authority, Some(13)),
        Err(NodeError::StaleNode),
        "health from a different boot is fenced"
    );
    let unknown_clock = EnrollmentEvent::CapacityObserved {
        event_id: "round15-health-unknown-clock".into(),
        node_id: node_id.clone(),
        boot_id: "boot-round15".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(13)),
        fresh_for_ticks: DecimalU64::new(2),
        capacity_digest: digest('e'),
    };
    assert_eq!(
        reduce(&replay, &unknown_clock, &scope, &authority, None),
        Err(NodeError::Invalid("clock unknown")),
        "telemetry without a controller clock is rejected"
    );

    let rotated = reduce(
        &replay,
        &EnrollmentEvent::Rotated {
            event_id: "round15-rotate".into(),
            node_id: node_id.clone(),
            expected_generation: DecimalU64::new(1),
            new_public_key_digest: digest('f'),
        },
        &scope,
        &authority,
        Some(13),
    )
    .expect("rotate credentials");
    assert!(
        rotated.capacity.is_empty(),
        "rotation invalidates all prior health"
    );
    assert_eq!(
        rotated
            .nodes
            .get(&node_id)
            .map(|identity| &identity.generation),
        Some(&DecimalU64::new(2))
    );
    let revoked = reduce(
        &rotated,
        &EnrollmentEvent::Revoked {
            event_id: "round15-revoke".into(),
            node_id: node_id.clone(),
            expected_generation: DecimalU64::new(2),
        },
        &scope,
        &authority,
        Some(14),
    )
    .expect("revoke rotated node");
    assert!(!revoked.nodes.contains_key(&node_id));
    assert!(revoked.capacity.is_empty());
    assert_eq!(
        reduce(&revoked, &stale, &scope, &authority, Some(15)),
        Err(NodeError::StaleNode),
        "revoked nodes cannot report health"
    );
}
