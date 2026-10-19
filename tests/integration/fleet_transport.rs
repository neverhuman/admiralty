//! P23 enrollment contracts plus actual mTLS, durable replay and transfer acceptance.

use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeError, NodeId, NodeIdentity, NodeRegistry,
    NodeScope, decode_event, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
use admiralty_runtime::MANAGED_EXECUTION_QUALIFIED;
use admiralty_testkit::faults::{FaultEffect, FaultPlan, FaultRule};

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
fn uppercase_digest_corruption_is_rejected_without_mutating_registry() {
    let state = NodeRegistry::default();
    let mut corrupted_token = token("corrupted-digest", 20);
    corrupted_token.token_digest = format!("sha256:{}", "A".repeat(64));
    let corrupted_event = EnrollmentEvent::TokenIssued {
        event_id: "issue-corrupted-digest".into(),
        token: corrupted_token,
    };

    assert_eq!(
        apply(&state, &corrupted_event, Some(1)),
        Err(NodeError::Invalid("malformed SHA-256 digest"))
    );
    assert!(state.tokens.is_empty());
    assert!(state.applied_events.is_empty());
}

#[test]
fn dropped_enrollment_send_preserves_state_until_deterministic_retry() {
    let start = issued_token("dropped-send", 20, 1);
    let enrollment = EnrollmentEvent::Enrolled {
        event_id: "enroll-after-drop".into(),
        token_id: "dropped-send".into(),
        scope: scope(),
        identity: identity("node-after-drop", "boot-1", 1, 'c'),
    };
    let mut state = start.clone();
    let mut faults = FaultPlan::new(vec![FaultRule {
        point: "enrollment.send".into(),
        occurrence: 1,
        effect: FaultEffect::Drop,
    }])
    .expect("one deterministic drop rule");

    assert_eq!(faults.hit("enrollment.send"), Ok(Some(FaultEffect::Drop)));
    assert_eq!(state, start, "dropped send applies no enrollment event");
    assert_eq!(faults.hit("enrollment.send"), Ok(None));
    state = apply(&state, &enrollment, Some(2)).expect("retry after dropped send");
    assert!(
        state
            .nodes
            .contains_key(&NodeId::new("node-after-drop").expect("node ID fixture"))
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

    let stale_enrollment = EnrollmentEvent::Enrolled {
        event_id: "replay-revoked-enrollment".into(),
        token_id: "boot-a-token".into(),
        scope: scope(),
        identity: identity("node-a", "boot-a", 1, 'c'),
    };
    assert_eq!(
        apply(&revoked, &stale_enrollment, Some(7)),
        Err(NodeError::Consumed)
    );
    assert!(
        !revoked
            .nodes
            .contains_key(&NodeId::new("node-a").expect("node ID fixture"))
    );
    assert_eq!(
        revoked.revoked_generations[&NodeId::new("node-a").expect("node ID fixture")],
        DecimalU64::new(3)
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
fn node_transport_receipt_does_not_grant_managed_execution() {
    const { assert!(!MANAGED_EXECUTION_QUALIFIED) };
}

#[cfg(unix)]
mod authenticated {
    use super::scope;
    use admiralty_core::nodes::DecimalU64;
    use admiralty_runtime::fleet::{
        digest,
        enrollment::{Controller, Credentials, Inventory},
        transfer::{Chunk, Manifest},
        transport::{self, Ack, Request, Response, Spool},
    };
    use std::{io, net::TcpListener, path::Path};

    fn inventory() -> Inventory {
        Inventory {
            cpu_millis: 2000,
            memory_bytes: DecimalU64::new(1_000_000),
            spool_bytes: DecimalU64::new(1_000_000),
            runners: vec!["isolated-oci".into()],
        }
    }

    fn credentials() -> Credentials {
        Credentials::generate(
            "localhost",
            digest(&serde_json::to_vec(&inventory()).expect("inventory")),
        )
        .expect("key and certificate")
    }

    fn enroll(root: &Path, node: &Credentials) {
        let mut controller = Controller::open(root).expect("controller");
        let (token, secret) = controller.issue(scope(), 1, 60).expect("issue");
        assert!(
            controller
                .enroll(
                    &token,
                    &"0".repeat(64),
                    &node.identity,
                    &node.certificate,
                    &inventory(),
                    2
                )
                .is_err()
        );
        controller
            .enroll(
                &token,
                &secret,
                &node.identity,
                &node.certificate,
                &inventory(),
                2,
            )
            .expect("enroll");
        drop(controller);
        let mut controller = Controller::open(root).expect("reopen durable registry");
        controller
            .enroll(
                &token,
                &secret,
                &node.identity,
                &node.certificate,
                &inventory(),
                3,
            )
            .expect("lost enrollment response retry");
        let different = credentials();
        assert!(
            controller
                .enroll(
                    &token,
                    &secret,
                    &different.identity,
                    &different.certificate,
                    &inventory(),
                    3
                )
                .is_err()
        );
    }

    fn exchange(
        root: &Path,
        server: &Credentials,
        client: &Credentials,
        request: &Request,
        after_handshake: impl FnOnce(),
    ) -> (io::Result<Response>, io::Result<()>) {
        let controller = Controller::open(root).expect("controller config");
        let config = transport::server_config(
            server,
            controller.certificates().expect("certificate inventory"),
        )
        .expect("mandatory mutual TLS");
        drop(controller);
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let address = listener.local_addr().expect("address");
        let root = root.to_owned();
        let handle = std::thread::spawn(move || {
            let mut controller = Controller::open(&root)?;
            transport::serve_once(listener.accept()?.0, config, &mut controller)
        });
        let result = (|| {
            let config = transport::client_config(client, server.certificate.clone())?;
            let mut stream = transport::connect(address, "localhost".into(), config)?;
            after_handshake();
            transport::write_frame(&mut stream, request)?;
            transport::read_frame(&mut stream)
        })();
        (result, handle.join().expect("server thread"))
    }

    fn ack(response: Response) -> Ack {
        match response {
            Response::Ack(ack) => ack,
            _ => panic!("expected durable observation acknowledgement"),
        }
    }

    #[test]
    fn persistent_uuid_key_certificate_and_expiring_one_use_enrollment() {
        let scratch = tempfile::tempdir().expect("scratch");
        let root = scratch.path().join("identity");
        let capability = digest(&serde_json::to_vec(&inventory()).expect("inventory"));
        let first = Credentials::load_or_create(&root, "localhost", capability.clone())
            .expect("initial identity");
        let second =
            Credentials::load_or_create(&root, "localhost", capability).expect("restart identity");
        assert_eq!(first.identity, second.identity);
        assert_eq!(first.certificate, second.certificate);
        first.validate().expect("matching persistent private key");
        second.validate().expect("matching restored private key");
        let root = scratch.path().join("controller");
        enroll(&root, &first);
        let mut controller = Controller::open(&root).expect("controller");
        let (id, secret) = controller.issue(scope(), 4, 1).expect("short token");
        let third = credentials();
        assert!(
            controller
                .enroll(
                    &id,
                    &secret,
                    &third.identity,
                    &third.certificate,
                    &inventory(),
                    5
                )
                .is_err()
        );
        assert!(
            !controller
                .registry()
                .expect("registry")
                .nodes
                .contains_key(&third.identity.node_id)
        );
    }

    #[test]
    fn actual_mutual_tls_replays_gaps_and_lost_acks_after_both_sides_restart() {
        let scratch = tempfile::tempdir().expect("scratch");
        let root = scratch.path().join("controller");
        let node = credentials();
        let server = credentials();
        enroll(&root, &node);
        let spool_root = scratch.path().join("spool");
        let mut spool = Spool::open(&spool_root).expect("producer spool");
        let first = spool
            .append(node.identity.clone(), scope(), b"first".to_vec())
            .expect("durable first");
        let second = spool
            .append(node.identity.clone(), scope(), b"second".to_vec())
            .expect("durable second");
        let (response, result) = exchange(
            &root,
            &server,
            &node,
            &Request::Observe(second.clone()),
            || {},
        );
        result.expect("gap response");
        let gap = ack(response.expect("gap acknowledgement"));
        assert_eq!(gap.through, DecimalU64::new(0));
        assert_eq!(gap.replay_from, Some(DecimalU64::new(1)));
        let (response, result) = exchange(
            &root,
            &server,
            &node,
            &Request::Observe(first.clone()),
            || {},
        );
        result.expect("first persisted");
        assert_eq!(
            ack(response.expect("first ACK")).through,
            DecimalU64::new(1)
        );
        // Deliberately lose the ACK: reload both durable sides and replay.
        drop(spool);
        let mut spool = Spool::open(&spool_root).expect("restart producer");
        assert_eq!(
            spool.pending().expect("pending"),
            vec![first.clone(), second.clone()]
        );
        let (response, result) = exchange(
            &root,
            &server,
            &node,
            &Request::Observe(first.clone()),
            || {},
        );
        result.expect("identical replay");
        spool
            .acknowledge(&ack(response.expect("replayed ACK")))
            .expect("scoped acknowledgement");
        assert_eq!(spool.pending().expect("pending"), vec![second.clone()]);
        let mut collision = first;
        collision.payload = b"changed".to_vec();
        let (response, result) =
            exchange(&root, &server, &node, &Request::Observe(collision), || {});
        assert!(response.is_err());
        assert!(result.is_err());
        let (response, result) = exchange(&root, &server, &node, &Request::Observe(second), || {});
        result.expect("gap repaired");
        let final_ack = ack(response.expect("final ACK"));
        let mut forged = final_ack.clone();
        forged.stream = digest(b"foreign stream");
        assert!(spool.acknowledge(&forged).is_err());
        spool.acknowledge(&final_ack).expect("drain exact producer");
        assert!(spool.pending().expect("drained").is_empty());
    }

    #[test]
    fn certificate_revocation_rotation_reboot_and_foreign_scope_fail_closed() {
        let scratch = tempfile::tempdir().expect("scratch");
        let root = scratch.path().join("controller");
        let node = credentials();
        let server = credentials();
        enroll(&root, &node);
        let mut spool = Spool::open(&scratch.path().join("spool")).expect("spool");
        let observation = spool
            .append(node.identity.clone(), scope(), b"observation".to_vec())
            .expect("append");
        let stranger = credentials();
        let (response, result) = exchange(
            &root,
            &server,
            &stranger,
            &Request::Observe(observation.clone()),
            || {},
        );
        assert!(response.is_err());
        assert!(result.is_err());
        let mut foreign = observation.clone();
        foreign.scope.task_id = admiralty_core::TaskId::new("foreign-task").expect("task");
        let (response, result) = exchange(&root, &server, &node, &Request::Observe(foreign), || {});
        assert!(response.is_err());
        assert!(result.is_err());
        // The stale TLS trust snapshot still accepts the cryptographic handshake;
        // revocation committed before the frame writer turn must reject receipt.
        let (response, result) = exchange(
            &root,
            &server,
            &node,
            &Request::Observe(observation.clone()),
            || {
                Controller::open(&root)
                    .expect("operator")
                    .revoke(&node.identity.node_id, 4)
                    .expect("revoke connected peer");
            },
        );
        assert!(response.is_err());
        assert!(result.is_err());
        let mut reboot = credentials();
        reboot.identity.node_id = node.identity.node_id.clone();
        reboot.identity.generation = DecimalU64::new(2);
        let mut controller = Controller::open(&root).expect("operator");
        let (token, secret) = controller.issue(scope(), 5, 60).expect("reboot token");
        controller
            .enroll(
                &token,
                &secret,
                &reboot.identity,
                &reboot.certificate,
                &inventory(),
                6,
            )
            .expect("reboot re-enrollment");
        assert!(controller.receive(&node.certificate, &observation).is_err());
        let mut fresh = observation;
        fresh.identity = reboot.identity.clone();
        let (response, result) = exchange(
            &root,
            &server,
            &reboot,
            &Request::Observe(fresh.clone()),
            || {},
        );
        result.expect("fresh boot TLS");
        response.expect("fresh boot receipt");
        let mut rotated = credentials();
        rotated.identity = controller
            .rotate(&reboot.identity.node_id, &rotated.certificate, 7)
            .expect("rotate");
        assert_eq!(rotated.identity.generation, DecimalU64::new(3));
        assert!(controller.receive(&reboot.certificate, &fresh).is_err());
        fresh.identity = rotated.identity.clone();
        let (response, result) =
            exchange(&root, &server, &rotated, &Request::Observe(fresh), || {});
        result.expect("rotated TLS");
        response.expect("rotated receipt");
    }

    #[test]
    fn partial_artifact_transfer_restarts_checks_custody_and_never_exposes_bad_bytes() {
        let scratch = tempfile::tempdir().expect("scratch");
        let root = scratch.path().join("controller");
        let node = credentials();
        let server = credentials();
        enroll(&root, &node);
        let bytes = b"durable artifact";
        let manifest = Manifest {
            transfer_id: uuid::Uuid::new_v4().to_string(),
            identity: node.identity.clone(),
            scope: scope(),
            digest: digest(bytes),
            size: DecimalU64::new(bytes.len() as u64),
        };
        let mut chunk = Chunk {
            manifest: manifest.clone(),
            offset: DecimalU64::new(0),
            bytes: bytes[..4].to_vec(),
            finish: false,
        };
        let mut controller = Controller::open(&root).expect("operator");
        assert!(controller.transfer(&node.certificate, &chunk).is_err());
        controller
            .grant_transfer(&node.certificate, &manifest)
            .expect("explicit local custody grant");
        assert!(controller.artifact(&manifest).is_err());
        drop(controller);
        let (response, result) = exchange(
            &root,
            &server,
            &node,
            &Request::Transfer(chunk.clone()),
            || {},
        );
        result.expect("partial persisted");
        response.expect("partial ACK");
        // Repeat a range after a lost response and actual database reopen.
        let (response, result) = exchange(
            &root,
            &server,
            &node,
            &Request::Transfer(chunk.clone()),
            || {},
        );
        result.expect("range replay");
        response.expect("range ACK");
        chunk.offset = DecimalU64::new(4);
        chunk.bytes = vec![b'x'; bytes.len() - 4];
        chunk.finish = true;
        let (response, result) = exchange(
            &root,
            &server,
            &node,
            &Request::Transfer(chunk.clone()),
            || {},
        );
        assert!(response.is_err());
        assert!(result.is_err());
        let controller = Controller::open(&root).expect("reopen after failed finish");
        assert!(controller.artifact(&manifest).is_err());
        drop(controller);
        chunk.bytes = bytes[4..].to_vec();
        let (response, result) = exchange(
            &root,
            &server,
            &node,
            &Request::Transfer(chunk.clone()),
            || {},
        );
        result.expect("digest checked finish");
        response.expect("complete ACK");
        let mut controller = Controller::open(&root).expect("restart consumer");
        assert_eq!(
            controller.artifact(&manifest).expect("verified artifact"),
            bytes
        );
        chunk.bytes[0] ^= 1;
        assert!(controller.transfer(&node.certificate, &chunk).is_err());
        controller
            .revoke(&node.identity.node_id, 4)
            .expect("revoke transfer sender");
        chunk.bytes = bytes[4..].to_vec();
        assert!(controller.transfer(&node.certificate, &chunk).is_err());
    }

    #[test]
    fn framing_and_spool_backpressure_preserve_unacknowledged_observations() {
        let mut oversized = &(u32::MAX.to_be_bytes())[..];
        assert!(transport::read_frame::<Request>(&mut oversized).is_err());
        let scratch = tempfile::tempdir().expect("scratch");
        let node = credentials();
        let mut spool = Spool::open(&scratch.path().join("spool")).expect("spool");
        for _ in 0..1024 {
            spool
                .append(node.identity.clone(), scope(), vec![1])
                .expect("within bounded spool");
        }
        assert!(
            spool
                .append(node.identity.clone(), scope(), vec![2])
                .is_err()
        );
        drop(spool);
        assert_eq!(
            Spool::open(&scratch.path().join("spool"))
                .expect("restart")
                .pending()
                .expect("retained evidence")
                .len(),
            1024
        );
    }
}
