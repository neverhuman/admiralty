//! P23 heartbeat, fencing recovery, and deterministic release audit qualification.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeError, NodeId, NodeIdentity, NodeRegistry,
    NodeScope, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId, content_digest};
use admiralty_runtime::recovery::{Blocker, plan_recovery};
use serde::Serialize;

#[path = "../../../../tests/integration/support/recovery.rs"]
mod recovery_fixture;

fn digest(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

fn scope() -> NodeScope {
    NodeScope {
        project_id: ProjectId::new("round19-project").expect("project fixture"),
        root_id: RootId::new("round19-root").expect("root fixture"),
        task_id: TaskId::new("round19-task").expect("task fixture"),
        authority: PrincipalId::new("round19-controller").expect("authority fixture"),
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

fn replay_release(scope: &NodeScope, events: &[EnrollmentEvent]) -> NodeRegistry {
    let mut state = NodeRegistry::default();
    for (index, event) in events.iter().enumerate() {
        let now = u64::try_from(index + 1).expect("bounded release event clock");
        state = apply(&state, event, scope, &scope.authority, now)
            .expect("recorded release event replays");
    }
    state
}

#[derive(Serialize)]
struct ReleaseAudit<'a> {
    events: &'a [EnrollmentEvent],
    final_registry: &'a NodeRegistry,
}

#[test]
fn p23_round19_heartbeat_fencing_recovery_release_audit() {
    let scope = scope();
    let actor = scope.authority.clone();
    let node_id = NodeId::new("round19-node").expect("node fixture");
    let token_a = EnrollmentToken {
        token_id: "round19-token-a".into(),
        token_digest: digest('a'),
        scope: scope.clone(),
        expires_tick: DecimalU64::new(40),
        consumed_binding: None,
    };
    let mut accepted = vec![EnrollmentEvent::TokenIssued {
        event_id: "round19-issue-a".into(),
        token: token_a,
    }];
    let mut state = apply(&NodeRegistry::default(), &accepted[0], &scope, &actor, 1)
        .expect("issue enrollment token");

    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "round19-enroll-a".into(),
        token_id: "round19-token-a".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: node_id.clone(),
            boot_id: "round19-boot-a".into(),
            public_key_digest: digest('b'),
            generation: DecimalU64::new(1),
            capability_digest: digest('c'),
        },
    };
    state = apply(&state, &enrollment, &scope, &actor, 2).expect("enroll node");
    accepted.push(enrollment);

    for tick in 3..=6 {
        let heartbeat = EnrollmentEvent::CapacityObserved {
            event_id: format!("round19-heartbeat-{tick}"),
            node_id: node_id.clone(),
            boot_id: "round19-boot-a".into(),
            generation: DecimalU64::new(1),
            observed_tick: Some(DecimalU64::new(tick)),
            fresh_for_ticks: DecimalU64::new(3),
            capacity_digest: digest(char::from_digit(tick as u32, 16).expect("hex digest seed")),
        };
        let next = apply(&state, &heartbeat, &scope, &actor, tick).expect("accept heartbeat");
        assert_eq!(
            apply(&state, &heartbeat, &scope, &actor, tick).expect("replay heartbeat"),
            next,
            "heartbeat replay yields the same projection"
        );
        state = next;
        accepted.push(heartbeat);
    }

    let rotation = EnrollmentEvent::Rotated {
        event_id: "round19-rotate".into(),
        node_id: node_id.clone(),
        expected_generation: DecimalU64::new(1),
        new_public_key_digest: digest('e'),
    };
    state = apply(&state, &rotation, &scope, &actor, 7).expect("rotate identity");
    accepted.push(rotation);
    assert!(
        state.capacity.is_empty(),
        "rotation invalidates every old heartbeat"
    );
    assert_eq!(
        state.nodes.get(&node_id).map(|node| &node.generation),
        Some(&DecimalU64::new(2))
    );

    let stale_after_rotation = EnrollmentEvent::CapacityObserved {
        event_id: "round19-heartbeat-stale-generation".into(),
        node_id: node_id.clone(),
        boot_id: "round19-boot-a".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(7)),
        fresh_for_ticks: DecimalU64::new(2),
        capacity_digest: digest('f'),
    };
    assert_eq!(
        apply(&state, &stale_after_rotation, &scope, &actor, 8),
        Err(NodeError::StaleNode),
        "old generation heartbeat is fenced after rotation"
    );

    let revoke = EnrollmentEvent::Revoked {
        event_id: "round19-revoke".into(),
        node_id: node_id.clone(),
        expected_generation: DecimalU64::new(2),
    };
    state = apply(&state, &revoke, &scope, &actor, 8).expect("revoke rotated node");
    accepted.push(revoke);
    assert!(!state.nodes.contains_key(&node_id));
    assert!(state.capacity.is_empty());
    assert_eq!(
        state.revoked_generations.get(&node_id),
        Some(&DecimalU64::new(3))
    );
    assert_eq!(
        apply(&state, &stale_after_rotation, &scope, &actor, 9),
        Err(NodeError::StaleNode),
        "revocation continues fencing old-generation heartbeats"
    );

    let token_b = EnrollmentEvent::TokenIssued {
        event_id: "round19-issue-b".into(),
        token: EnrollmentToken {
            token_id: "round19-token-b".into(),
            token_digest: digest('d'),
            scope: scope.clone(),
            expires_tick: DecimalU64::new(40),
            consumed_binding: None,
        },
    };
    state = apply(&state, &token_b, &scope, &actor, 9).expect("issue recovery token");
    accepted.push(token_b);
    let recovered_enrollment = EnrollmentEvent::Enrolled {
        event_id: "round19-enroll-b".into(),
        token_id: "round19-token-b".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: node_id.clone(),
            boot_id: "round19-boot-b".into(),
            public_key_digest: digest('e'),
            generation: DecimalU64::new(3),
            capability_digest: digest('c'),
        },
    };
    state = apply(&state, &recovered_enrollment, &scope, &actor, 10)
        .expect("recover only at next fenced generation");
    accepted.push(recovered_enrollment);
    let recovered_heartbeat = EnrollmentEvent::CapacityObserved {
        event_id: "round19-heartbeat-recovered".into(),
        node_id: node_id.clone(),
        boot_id: "round19-boot-b".into(),
        generation: DecimalU64::new(3),
        observed_tick: Some(DecimalU64::new(11)),
        fresh_for_ticks: DecimalU64::new(3),
        capacity_digest: digest('c'),
    };
    state = apply(&state, &recovered_heartbeat, &scope, &actor, 11)
        .expect("accept recovered node heartbeat");
    accepted.push(recovered_heartbeat);

    let replayed = replay_release(&scope, &accepted);
    assert_eq!(
        replayed, state,
        "complete release audit replays to same state"
    );
    let audit = ReleaseAudit {
        events: &accepted,
        final_registry: &state,
    };
    let audit_digest = content_digest(&audit).expect("release audit digest");
    let replay_audit = ReleaseAudit {
        events: &accepted,
        final_registry: &replayed,
    };
    assert_eq!(
        content_digest(&replay_audit).expect("replayed audit digest"),
        audit_digest,
        "deterministic event replay preserves the release audit identity"
    );

    let (manifest, observation, external_fence) = recovery_fixture::valid_recovery_evidence();
    let held = plan_recovery(&manifest, &observation, None).expect("recovery audit without fence");
    assert!(held.blockers.contains(&Blocker::MissingExternalFence));
    assert!(!held.resume_proposed);
    let fenced = plan_recovery(&manifest, &observation, Some(&external_fence))
        .expect("recovery audit with external fence");
    assert!(fenced.resume_proposed);
    assert!(!fenced.restore_authorized());
    assert!(!fenced.effects_authorized());
}
