#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Release bundle and schema parity qualification for P23 node state.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeId, NodeIdentity, NodeRegistry, NodeScope,
    decode_event, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId, content_digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn digest(value: char) -> String {
    format!("sha256:{}", value.to_string().repeat(64))
}

fn scope() -> NodeScope {
    NodeScope {
        project_id: ProjectId::new("round18-project").expect("project fixture"),
        root_id: RootId::new("round18-root").expect("root fixture"),
        task_id: TaskId::new("round18-task").expect("task fixture"),
        authority: PrincipalId::new("round18-controller").expect("authority fixture"),
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ReleaseBundle {
    schema_digest: String,
    events: Vec<EnrollmentEvent>,
    registry: NodeRegistry,
}

fn schema_has_key(value: &Value, expected: &str) -> bool {
    match value {
        Value::Object(object) => {
            object.contains_key(expected)
                || object
                    .values()
                    .any(|nested| schema_has_key(nested, expected))
        }
        Value::Array(items) => items.iter().any(|nested| schema_has_key(nested, expected)),
        _ => false,
    }
}

#[test]
fn p23_round18_release_bundle_schema_parity_qualification() {
    let schema = serde_json::to_value(schemars::schema_for!(EnrollmentEvent))
        .expect("serialize enrollment schema");
    for field in [
        "kind",
        "event_id",
        "token",
        "token_id",
        "scope",
        "identity",
        "node_id",
        "boot_id",
        "generation",
        "observed_tick",
        "fresh_for_ticks",
        "capacity_digest",
        "expected_generation",
        "new_public_key_digest",
    ] {
        assert!(
            schema_has_key(&schema, field),
            "schema omits wire field {field}"
        );
    }
    assert_eq!(
        serde_json::to_value(schemars::schema_for!(EnrollmentEvent)).expect("repeat schema"),
        schema,
        "schema output is deterministic for release bundle hashing"
    );

    let scope = scope();
    let actor = scope.authority.clone();
    let node_id = NodeId::new("round18-node").expect("node fixture");
    let events = vec![
        EnrollmentEvent::TokenIssued {
            event_id: "round18-issue".into(),
            token: EnrollmentToken {
                token_id: "round18-token".into(),
                token_digest: digest('a'),
                scope: scope.clone(),
                expires_tick: DecimalU64::new(30),
                consumed_binding: None,
            },
        },
        EnrollmentEvent::Enrolled {
            event_id: "round18-enroll".into(),
            token_id: "round18-token".into(),
            scope: scope.clone(),
            identity: NodeIdentity {
                node_id: node_id.clone(),
                boot_id: "round18-boot-a".into(),
                public_key_digest: digest('b'),
                generation: DecimalU64::new(1),
                capability_digest: digest('c'),
            },
        },
        EnrollmentEvent::CapacityObserved {
            event_id: "round18-health".into(),
            node_id: node_id.clone(),
            boot_id: "round18-boot-a".into(),
            generation: DecimalU64::new(1),
            observed_tick: Some(DecimalU64::new(3)),
            fresh_for_ticks: DecimalU64::new(4),
            capacity_digest: digest('d'),
        },
        EnrollmentEvent::Rotated {
            event_id: "round18-rotate".into(),
            node_id: node_id.clone(),
            expected_generation: DecimalU64::new(1),
            new_public_key_digest: digest('e'),
        },
        EnrollmentEvent::CapacityObserved {
            event_id: "round18-health-after-rotate".into(),
            node_id: node_id.clone(),
            boot_id: "round18-boot-a".into(),
            generation: DecimalU64::new(2),
            observed_tick: Some(DecimalU64::new(5)),
            fresh_for_ticks: DecimalU64::new(4),
            capacity_digest: digest('f'),
        },
        EnrollmentEvent::Revoked {
            event_id: "round18-revoke".into(),
            node_id: node_id.clone(),
            expected_generation: DecimalU64::new(2),
        },
    ];

    let mut registry = NodeRegistry::default();
    for (index, event) in events.iter().enumerate() {
        let wire = serde_json::to_vec(event).expect("serialize event wire");
        assert_eq!(
            decode_event(&wire).expect("schema-compatible event decode"),
            *event
        );
        let now = u64::try_from(index + 1).expect("bounded fixture clock");
        registry = reduce_enrollment_for(&registry, event, &scope, &actor, Some(now))
            .expect("apply release bundle event");
    }
    assert!(
        registry.nodes.is_empty(),
        "release bundle ends after revocation"
    );
    assert!(
        registry.capacity.is_empty(),
        "revocation removes all health state"
    );
    assert_eq!(
        registry.revoked_generations.get(&node_id),
        Some(&DecimalU64::new(3))
    );

    let schema_digest = content_digest(&schema).expect("canonical schema digest");
    let bundle = ReleaseBundle {
        schema_digest,
        events,
        registry,
    };
    let bytes = serde_json::to_vec(&bundle).expect("serialize release bundle");
    let replayed: ReleaseBundle = serde_json::from_slice(&bytes).expect("decode release bundle");
    assert_eq!(
        replayed, bundle,
        "bundle preserves schema, event sequence and projection"
    );
    assert_eq!(
        content_digest(&bundle).expect("bundle content digest"),
        content_digest(&replayed).expect("replayed bundle content digest"),
        "bundle replay preserves its content address"
    );

    let mut unknown = serde_json::to_value(&bundle.events[0]).expect("token event JSON");
    unknown
        .as_object_mut()
        .expect("tagged event object")
        .insert("release_override".into(), serde_json::json!(true));
    assert!(
        decode_event(&serde_json::to_vec(&unknown).expect("unknown event JSON")).is_err(),
        "wire decoder remains strict where the generated schema is closed"
    );
}
