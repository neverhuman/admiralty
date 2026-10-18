//! Final deterministic end-to-end gate for P23 enrollment state.

use admiralty_core::economics::{
    BoundedId, CohortSpec, DecimalU64 as EconomicsTick, RootBinding, RootOutcome, RootStatus,
    summarize_cohort,
};
use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeError, NodeId, NodeIdentity, NodeRegistry,
    NodeScope, decode_event, reduce_enrollment_for,
};
use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId, content_digest};
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
    const { assert!(!MANAGED_EXECUTION_QUALIFIED) };
}

#[test]
fn p23_round12_item5_redteam_audit_gate() {
    let empty = NodeRegistry::default();
    let issued_event = EnrollmentEvent::TokenIssued {
        event_id: "round12-receipt-issue".into(),
        token: token("round12-token", 12),
    };
    let issued = apply(&empty, &issued_event, Some(1)).expect("valid scoped token issue");

    // The reducer's durable event receipt binds both the exact event and its explicit clock.
    let receipt = content_digest(&(issued_event.clone(), Some(1))).expect("event receipt digest");
    assert_eq!(issued.applied_events["round12-receipt-issue"], receipt);
    assert_eq!(
        apply(&issued, &issued_event, Some(1)).expect("identical event receipt replay"),
        issued
    );
    assert_eq!(
        apply(&issued, &issued_event, Some(2)),
        Err(NodeError::Collision),
        "the event receipt also binds the clock"
    );

    let mut forged_receipt = issued.clone();
    forged_receipt
        .applied_events
        .insert("round12-receipt-issue".into(), digest('f'));
    assert_eq!(
        apply(&forged_receipt, &issued_event, Some(1)),
        Err(NodeError::Collision),
        "a forged stored receipt cannot authorize replay"
    );
    assert_eq!(
        forged_receipt.applied_events["round12-receipt-issue"],
        digest('f'),
        "failed replay does not repair or mutate forged history"
    );

    let mut missing_clock = issued.clone();
    missing_clock.last_tick = None;
    let next_issue = EnrollmentEvent::TokenIssued {
        event_id: "round12-missing-clock".into(),
        token: token("round12-next-token", 20),
    };
    assert_eq!(
        apply(&missing_clock, &next_issue, Some(3)),
        Err(NodeError::Invalid("recorded clock history missing"))
    );

    let mut foreign_scope = scope();
    foreign_scope.project_id = ProjectId::new("untrusted-project").expect("project");
    let foreign_event = EnrollmentEvent::TokenIssued {
        event_id: "round12-foreign-scope".into(),
        token: EnrollmentToken {
            token_id: "round12-foreign-token".into(),
            token_digest: digest('a'),
            scope: foreign_scope,
            expires_tick: DecimalU64::new(12),
            consumed_binding: None,
        },
    };
    assert_eq!(
        reduce_enrollment_for(
            &empty,
            &foreign_event,
            &scope(),
            &scope().authority,
            Some(1)
        ),
        Err(NodeError::ForeignScope),
        "event-provided scope cannot replace the authenticated controller scope"
    );

    let malformed_digest = EnrollmentEvent::TokenIssued {
        event_id: "round12-malformed-digest".into(),
        token: EnrollmentToken {
            token_id: "round12-malformed-token".into(),
            token_digest: "sha256:NOT-HEX".into(),
            scope: scope(),
            expires_tick: DecimalU64::new(12),
            consumed_binding: None,
        },
    };
    assert!(
        decode_event(
            &serde_json::to_vec(&malformed_digest).expect("encode malformed digest event")
        )
        .is_err()
    );

    let expired = EnrollmentEvent::TokenIssued {
        event_id: "round12-expired-token".into(),
        token: token("round12-expired", 2),
    };
    assert_eq!(
        apply(&empty, &expired, Some(2)),
        Err(NodeError::TokenExpired)
    );
    assert!(!empty.tokens.contains_key("round12-expired"));

    let stale_node = EnrollmentEvent::CapacityObserved {
        event_id: "round12-stale-node".into(),
        node_id: NodeId::new("unregistered-round12-node").expect("node ID"),
        boot_id: "untrusted-boot".into(),
        generation: DecimalU64::new(1),
        observed_tick: Some(DecimalU64::new(1)),
        fresh_for_ticks: DecimalU64::new(10),
        capacity_digest: digest('b'),
    };
    assert_eq!(
        reduce_enrollment_for(&issued, &stale_node, &scope(), &scope().authority, Some(2)),
        Err(NodeError::ForeignScope),
        "an unregistered node cannot create a scope or capacity receipt"
    );
    assert!(
        !issued
            .capacity
            .contains_key(&NodeId::new("unregistered-round12-node").expect("node ID"))
    );
    const { assert!(!MANAGED_EXECUTION_QUALIFIED) };
}

#[test]
fn p23_round13_wo_node_running_evidence_gate() {
    // A registered, live P23 node and fresh capacity observation are scheduling evidence only.
    let s = NodeScope {
        project_id: ProjectId::new("round13-project").expect("project ID"),
        root_id: RootId::new("round13-root").expect("root ID"),
        task_id: TaskId::new("round13-task").expect("task ID"),
        authority: PrincipalId::new("round13-authority").expect("authority"),
    };
    let node = NodeId::new("round13-node").expect("node ID");
    let token_event = EnrollmentEvent::TokenIssued {
        event_id: "round13-token-issue".into(),
        token: EnrollmentToken {
            token_id: "round13-token".into(),
            token_digest: digest('a'),
            scope: s.clone(),
            expires_tick: DecimalU64::new(30),
            consumed_binding: None,
        },
    };
    let issued = apply(&NodeRegistry::default(), &token_event, Some(1)).expect("issue token");
    let enrolled = apply(
        &issued,
        &EnrollmentEvent::Enrolled {
            event_id: "round13-enroll".into(),
            token_id: "round13-token".into(),
            scope: s.clone(),
            identity: identity("round13-node", "boot-1", 1, 'b'),
        },
        Some(2),
    )
    .expect("enroll node");
    let running = apply(
        &enrolled,
        &EnrollmentEvent::CapacityObserved {
            event_id: "round13-capacity".into(),
            node_id: node,
            boot_id: "boot-1".into(),
            generation: DecimalU64::new(1),
            observed_tick: Some(DecimalU64::new(3)),
            fresh_for_ticks: DecimalU64::new(10),
            capacity_digest: digest('c'),
        },
        Some(4),
    )
    .expect("record fresh node running capacity");
    assert_eq!(running.capacity.len(), 1);

    // Bind the work-order outcome to its exact project/root/task contract and release stage.
    let binding = RootBinding {
        root_id: RootId::new("round13-root").expect("root ID"),
        task_id: TaskId::new("round13-task").expect("task ID"),
        contract_revision: 2,
        contract_digest: digest('d'),
        protected_oracle_digest: digest('e'),
    };
    let cohort = CohortSpec {
        cohort_id: BoundedId::new("round13-work-order").expect("work-order ID"),
        project_id: s.project_id.clone(),
        fixed_assignment_roots: vec![binding.root_id.clone()],
        required_delivery_stage: BoundedId::new("release").expect("release stage"),
        valuation_digest: digest('f'),
        assigned_at: EconomicsTick(1),
        bindings: vec![binding.clone()],
    };
    let outcome = |status, accepted_stage: Option<&str>, receipt: Option<String>| RootOutcome {
        project_id: s.project_id.clone(),
        binding: binding.clone(),
        acceptance_receipt_digest: receipt,
        root_id: binding.root_id.clone(),
        status,
        accepted_stage: accepted_stage.map(|stage| BoundedId::new(stage).expect("stage")),
        maturity: EconomicsTick(20),
    };

    let node_is_running = summarize_cohort(
        &cohort,
        &[],
        &[outcome(RootStatus::Running, Some("running"), None)],
        10,
    )
    .expect("running evidence is valid but not release acceptance");
    assert_eq!(node_is_running.accepted_roots, Some(EconomicsTick(0)));
    assert_eq!(node_is_running.immature_roots, EconomicsTick(1));
    assert!(node_is_running.accepted_yield.is_none());

    let merged_without_release_receipt = summarize_cohort(
        &cohort,
        &[],
        &[outcome(RootStatus::Succeeded, Some("release"), None)],
        10,
    )
    .expect("missing receipt remains unknown");
    assert_eq!(merged_without_release_receipt.accepted_roots, None);
    assert!(merged_without_release_receipt.accepted_yield.is_none());

    let wrong_stage = summarize_cohort(
        &cohort,
        &[],
        &[outcome(
            RootStatus::Succeeded,
            Some("installed"),
            Some(digest('a')),
        )],
        10,
    )
    .expect("wrong delivery stage cannot satisfy release contract");
    assert_eq!(wrong_stage.accepted_roots, Some(EconomicsTick(0)));
    assert_eq!(
        wrong_stage
            .accepted_yield
            .as_ref()
            .map(|r| r.numerator.as_u128()),
        Some(0)
    );

    let released = summarize_cohort(
        &cohort,
        &[],
        &[outcome(
            RootStatus::Succeeded,
            Some("release"),
            Some(digest('a')),
        )],
        10,
    )
    .expect("exact mature release receipt accepted");
    assert_eq!(released.accepted_roots, Some(EconomicsTick(1)));
    assert_eq!(
        released
            .accepted_yield
            .as_ref()
            .map(|r| r.numerator.as_u128()),
        Some(1)
    );
    const { assert!(!MANAGED_EXECUTION_QUALIFIED) };
}

#[test]
fn p23_round13_wo_node_running_evidence_gate() {
    // A registered, live P23 node and fresh capacity observation are scheduling evidence only.
    let s = NodeScope {
        project_id: ProjectId::new("round13-project").expect("project ID"),
        root_id: RootId::new("round13-root").expect("root ID"),
        task_id: TaskId::new("round13-task").expect("task ID"),
        authority: PrincipalId::new("round13-authority").expect("authority"),
    };
    let node = NodeId::new("round13-node").expect("node ID");
    let token_event = EnrollmentEvent::TokenIssued {
        event_id: "round13-token-issue".into(),
        token: EnrollmentToken {
            token_id: "round13-token".into(),
            token_digest: digest('a'),
            scope: s.clone(),
            expires_tick: DecimalU64::new(30),
            consumed_binding: None,
        },
    };
    let issued = apply(&NodeRegistry::default(), &token_event, Some(1)).expect("issue token");
    let enrolled = apply(
        &issued,
        &EnrollmentEvent::Enrolled {
            event_id: "round13-enroll".into(),
            token_id: "round13-token".into(),
            scope: s.clone(),
            identity: identity("round13-node", "boot-1", 1, 'b'),
        },
        Some(2),
    )
    .expect("enroll node");
    let running = apply(
        &enrolled,
        &EnrollmentEvent::CapacityObserved {
            event_id: "round13-capacity".into(),
            node_id: node,
            boot_id: "boot-1".into(),
            generation: DecimalU64::new(1),
            observed_tick: Some(DecimalU64::new(3)),
            fresh_for_ticks: DecimalU64::new(10),
            capacity_digest: digest('c'),
        },
        Some(4),
    )
    .expect("record fresh node running capacity");
    assert_eq!(running.capacity.len(), 1);

    // Bind the work-order outcome to its exact project/root/task contract and release stage.
    let binding = RootBinding {
        root_id: RootId::new("round13-root").expect("root ID"),
        task_id: TaskId::new("round13-task").expect("task ID"),
        contract_revision: 2,
        contract_digest: digest('d'),
        protected_oracle_digest: digest('e'),
    };
    let cohort = CohortSpec {
        cohort_id: BoundedId::new("round13-work-order").expect("work-order ID"),
        project_id: s.project_id.clone(),
        fixed_assignment_roots: vec![binding.root_id.clone()],
        required_delivery_stage: BoundedId::new("release").expect("release stage"),
        valuation_digest: digest('f'),
        assigned_at: EconomicsTick(1),
        bindings: vec![binding.clone()],
    };
    let outcome = |status, accepted_stage: Option<&str>, receipt: Option<String>| RootOutcome {
        project_id: s.project_id.clone(),
        binding: binding.clone(),
        acceptance_receipt_digest: receipt,
        root_id: binding.root_id.clone(),
        status,
        accepted_stage: accepted_stage.map(|stage| BoundedId::new(stage).expect("stage")),
        maturity: EconomicsTick(20),
    };

    let node_is_running = summarize_cohort(
        &cohort,
        &[],
        &[outcome(RootStatus::Running, Some("running"), None)],
        10,
    )
    .expect("running evidence is valid but not release acceptance");
    assert_eq!(node_is_running.accepted_roots, Some(EconomicsTick(0)));
    assert_eq!(node_is_running.immature_roots, EconomicsTick(1));
    assert!(node_is_running.accepted_yield.is_none());

    let merged_without_release_receipt = summarize_cohort(
        &cohort,
        &[],
        &[outcome(RootStatus::Succeeded, Some("release"), None)],
        10,
    )
    .expect("missing receipt remains unknown");
    assert_eq!(merged_without_release_receipt.accepted_roots, None);
    assert!(merged_without_release_receipt.accepted_yield.is_none());

    let wrong_stage = summarize_cohort(
        &cohort,
        &[],
        &[outcome(
            RootStatus::Succeeded,
            Some("installed"),
            Some(digest('a')),
        )],
        10,
    )
    .expect("wrong delivery stage cannot satisfy release contract");
    assert_eq!(wrong_stage.accepted_roots, Some(EconomicsTick(0)));
    assert_eq!(
        wrong_stage
            .accepted_yield
            .as_ref()
            .map(|r| r.numerator.as_u128()),
        Some(0)
    );

    let released = summarize_cohort(
        &cohort,
        &[],
        &[outcome(
            RootStatus::Succeeded,
            Some("release"),
            Some(digest('a')),
        )],
        10,
    )
    .expect("exact mature release receipt accepted");
    assert_eq!(released.accepted_roots, Some(EconomicsTick(1)));
    assert_eq!(
        released
            .accepted_yield
            .as_ref()
            .map(|r| r.numerator.as_u128()),
        Some(1)
    );
    assert!(!MANAGED_EXECUTION_QUALIFIED);
}
