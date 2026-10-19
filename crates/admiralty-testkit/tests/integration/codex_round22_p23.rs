#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Canary smoke and deterministic release package qualification for P23.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeError, NodeId, NodeIdentity, NodeRegistry,
    NodeScope, decode_event, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId, content_digest};
use serde::{Deserialize, Serialize};

fn digest(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

fn scope() -> NodeScope {
    NodeScope {
        project_id: ProjectId::new("round22-project").expect("project fixture"),
        root_id: RootId::new("round22-root").expect("root fixture"),
        task_id: TaskId::new("round22-task").expect("task fixture"),
        authority: PrincipalId::new("round22-controller").expect("authority fixture"),
    }
}

fn apply(
    registry: &NodeRegistry,
    event: &EnrollmentEvent,
    scope: &NodeScope,
    actor: &PrincipalId,
    tick: Option<u64>,
) -> Result<NodeRegistry, NodeError> {
    reduce_enrollment_for(registry, event, scope, actor, tick)
}

#[derive(Deserialize, Serialize)]
struct ReleasePackage {
    qualification: String,
    events: Vec<EnrollmentEvent>,
    registry: NodeRegistry,
}

#[test]
fn p23_round22_canary_smoke_release_package_qualification() {
    let scope = scope();
    let controller = scope.authority.clone();
    let foreign = PrincipalId::new("round22-foreign").expect("foreign principal fixture");
    let node_id = NodeId::new("round22-canary-node").expect("node fixture");
    let token = EnrollmentEvent::TokenIssued {
        event_id: "round22-token-issued".into(),
        token: EnrollmentToken {
            token_id: "round22-token".into(),
            token_digest: digest('a'),
            scope: scope.clone(),
            expires_tick: DecimalU64::new(12),
            consumed_binding: None,
        },
    };
    let mut registry = apply(
        &NodeRegistry::default(),
        &token,
        &scope,
        &controller,
        Some(1),
    )
    .expect("issue scoped canary credential");
    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "round22-node-enrolled".into(),
        token_id: "round22-token".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: node_id.clone(),
            boot_id: "round22-boot-1".into(),
            public_key_digest: digest('b'),
            generation: DecimalU64::new(1),
            capability_digest: digest('c'),
        },
    };
    let enrollment = decode_event(&serde_json::to_vec(&enrollment).expect("encode enrollment"))
        .expect("decode enrollment wire event");
    registry =
        apply(&registry, &enrollment, &scope, &controller, Some(2)).expect("enroll canary node");

    let mut events = vec![token, enrollment];
    for tick in 3..=5 {
        let heartbeat = EnrollmentEvent::CapacityObserved {
            event_id: format!("round22-heartbeat-{tick}"),
            node_id: node_id.clone(),
            boot_id: "round22-boot-1".into(),
            generation: DecimalU64::new(1),
            observed_tick: Some(DecimalU64::new(tick)),
            fresh_for_ticks: DecimalU64::new(3),
            capacity_digest: digest(char::from_digit(tick as u32, 16).expect("hex fixture")),
        };
        let next = apply(&registry, &heartbeat, &scope, &controller, Some(tick))
            .expect("accept deterministic canary heartbeat");
        assert_eq!(
            apply(&registry, &heartbeat, &scope, &controller, Some(tick))
                .expect("replay canary heartbeat"),
            next,
            "same input produces the same registry state"
        );
        registry = next;
        events.push(heartbeat);
    }

    // The canary gate accepts only a current scoped controller observation.
    let before_rejection = registry.clone();
    let foreign_heartbeat = EnrollmentEvent::CapacityObserved {
        event_id: "round22-foreign-heartbeat".into(),
        node_id: node_id.clone(),
        boot_id: "round22-boot-1".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(6)),
        fresh_for_ticks: DecimalU64::new(2),
        capacity_digest: digest('f'),
    };
    assert_eq!(
        apply(&registry, &foreign_heartbeat, &scope, &foreign, Some(6)),
        Err(NodeError::ForeignScope)
    );
    assert_eq!(
        registry, before_rejection,
        "rejected canary probe is fail-closed"
    );

    let package = ReleasePackage {
        qualification: "p23-round22-canary-smoke".into(),
        events,
        registry,
    };
    let encoded = serde_json::to_vec(&package).expect("encode release package");
    let package_digest = content_digest(&encoded).expect("release package digest");
    let decoded: ReleasePackage = serde_json::from_slice(&encoded).expect("decode release package");
    let replayed = serde_json::to_vec(&decoded).expect("re-encode release package");
    assert_eq!(encoded, replayed, "release package encoding is stable");
    assert_eq!(
        content_digest(&replayed).expect("replayed package digest"),
        package_digest
    );

    let expired_token = EnrollmentEvent::TokenIssued {
        event_id: "round22-expired-token".into(),
        token: EnrollmentToken {
            token_id: "round22-expired".into(),
            token_digest: digest('d'),
            scope: scope.clone(),
            expires_tick: DecimalU64::new(7),
            consumed_binding: None,
        },
    };
    let issued = apply(
        &NodeRegistry::default(),
        &expired_token,
        &scope,
        &controller,
        Some(1),
    )
    .expect("record expiring token");
    let expired_enrollment = EnrollmentEvent::Enrolled {
        event_id: "round22-expired-enrollment".into(),
        token_id: "round22-expired".into(),
        scope: scope.clone(),
        identity: NodeIdentity {
            node_id: NodeId::new("round22-expired-node").expect("node fixture"),
            boot_id: "round22-expired-boot".into(),
            public_key_digest: digest('e'),
            generation: DecimalU64::new(1),
            capability_digest: digest('f'),
        },
    };
    assert_eq!(
        apply(&issued, &expired_enrollment, &scope, &controller, Some(8)),
        Err(NodeError::TokenExpired),
        "expired credentials cannot pass the release canary gate"
    );
}
