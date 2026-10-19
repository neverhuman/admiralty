use admiralty_core::coordination::envelope::*;
use admiralty_core::interfaces::DecimalU64;
use admiralty_core::{
    CoordinationMessage, MessageId, MessagePayload, PrincipalId, ProjectId, RootId, TaskId,
};
use serde_json::{Value, json};

const PAYLOAD: &[u8] =
    b"Do NOT publish; timeout <= 5 ms; uncertainty remains; next: inspect candidate.";

fn fixture() -> CoordinationEnvelopeV2 {
    let scope = DeclaredScope {
        project_id: ProjectId::new("project-1").expect("valid fixture or asserted JSON shape"),
        root_id: RootId::new("root-1").expect("valid fixture or asserted JSON shape"),
        sender: PrincipalId::new("worker-1").expect("valid fixture or asserted JSON shape"),
        recipient: RecipientScope::Principal {
            principal_id: PrincipalId::new("commodore-1")
                .expect("valid fixture or asserted JSON shape"),
        },
        topic: TopicId::new("task.failure").expect("valid fixture or asserted JSON shape"),
        task_revision: Some(TaskRevisionRef {
            task_id: TaskId::new("task-1").expect("valid fixture or asserted JSON shape"),
            revision: 2,
            contract_digest: Sha256Digest::of_bytes(b"contract-2"),
        }),
    };
    let scope_digest = scope
        .digest()
        .expect("valid fixture or asserted JSON shape");
    CoordinationEnvelopeV2 {
        protocol_version: ProtocolVersion2,
        message_id: MessageId::new("message-1").expect("valid fixture or asserted JSON shape"),
        project_id: scope.project_id,
        root_id: scope.root_id,
        sender: scope.sender,
        recipient: scope.recipient,
        topic: scope.topic,
        task_revision: scope.task_revision,
        causation_id: Some(
            MessageId::new("message-0").expect("valid fixture or asserted JSON shape"),
        ),
        correlation_id: CorrelationId::new("correlation-1")
            .expect("valid fixture or asserted JSON shape"),
        source_cursor: Cursor {
            recovery_generation: DecimalU64::new(3),
            event_seq: DecimalU64::new(u64::MAX),
            projection_schema: 2,
            authorization_scope_digest: scope_digest.clone(),
        },
        authority: AuthorityRef {
            reference: "policy:worker-1:root-1".into(),
            generation: AuthorityGeneration::new(4).expect("valid fixture or asserted JSON shape"),
            policy_digest: Sha256Digest::of_bytes(b"operator policy"),
            scope_digest,
        },
        context_epoch: Some(
            ContextEpoch::new("epoch-1").expect("valid fixture or asserted JSON shape"),
        ),
        baseline_digest: Some(Sha256Digest::of_bytes(b"baseline")),
        protected_facts_digest: Sha256Digest::of_bytes(PAYLOAD),
        payload_ref: PayloadRef {
            sha256: Sha256Digest::of_bytes(PAYLOAD),
            size_bytes: DecimalU64::new(PAYLOAD.len() as u64),
        },
        kind: CoordinationKind::VerificationFailed,
        priority: Priority::Decision,
        expires_at_unix_ms: Some(DecimalU64::new(500)),
    }
}

fn bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("valid fixture or asserted JSON shape")
}

fn check_exact(
    actual: &CoordinationEnvelopeV2,
    expected: &CoordinationEnvelopeV2,
) -> Result<(), EnvelopeError> {
    actual.validate_for(&ExpectedBindings {
        scope: &expected.scope(),
        source_cursor: &expected.source_cursor,
        authority: &expected.authority,
        context_epoch: expected.context_epoch.as_ref(),
        baseline_digest: expected.baseline_digest.as_ref(),
        protected_facts_digest: &expected.protected_facts_digest,
        envelope_digest: &expected.digest()?,
    })
}

#[test]
fn roundtrip_preserves_full_machine_identity_and_exact_payload() {
    let envelope = fixture();
    let raw = envelope
        .canonical_bytes()
        .expect("valid fixture or asserted JSON shape");
    assert!(raw.len() < MAX_ENVELOPE_BYTES);
    let parsed = CoordinationEnvelopeV2::parse(&raw).expect("valid fixture or asserted JSON shape");
    assert_eq!(parsed, envelope);
    assert_eq!(
        parsed
            .canonical_bytes()
            .expect("valid fixture or asserted JSON shape"),
        raw
    );
    assert_eq!(
        parsed
            .digest()
            .expect("valid fixture or asserted JSON shape"),
        Sha256Digest::of_bytes(&raw)
    );
    assert_eq!(check_exact(&parsed, &envelope), Ok(()));
    assert_eq!(parsed.verify_payload_bytes(PAYLOAD), Ok(()));
    let value: Value = serde_json::from_slice(&raw).expect("valid fixture or asserted JSON shape");
    assert_eq!(value["protocol_version"], 2);
    assert_eq!(value["source_cursor"]["event_seq"], u64::MAX.to_string());
    assert_eq!(value["authority"]["generation"], "4");
    assert_eq!(value["kind"], "verification_failed");
    assert!(value.get("schema_version").is_none());
}

#[test]
fn every_protected_header_field_is_bound_even_after_recomputing_scope() {
    let expected = fixture();
    let base = serde_json::to_value(&expected).expect("valid fixture or asserted JSON shape");
    let mutations = [
        ("/message_id", json!("message-2")),
        ("/project_id", json!("project-2")),
        ("/root_id", json!("root-2")),
        ("/sender", json!("worker-2")),
        ("/recipient/principal_id", json!("commodore-2")),
        ("/topic", json!("task.other")),
        ("/task_revision/task_id", json!("task-2")),
        ("/task_revision/revision", json!(3)),
        (
            "/task_revision/contract_digest",
            json!(Sha256Digest::of_bytes(b"other contract")),
        ),
        ("/causation_id", json!("message-other")),
        ("/correlation_id", json!("correlation-other")),
        ("/source_cursor/recovery_generation", json!("4")),
        ("/source_cursor/event_seq", json!("10")),
        ("/source_cursor/projection_schema", json!(3)),
        ("/authority/reference", json!("policy:other")),
        ("/authority/generation", json!("5")),
        (
            "/authority/policy_digest",
            json!(Sha256Digest::of_bytes(b"other policy")),
        ),
        ("/context_epoch", json!("epoch-2")),
        (
            "/baseline_digest",
            json!(Sha256Digest::of_bytes(b"other baseline")),
        ),
        (
            "/protected_facts_digest",
            json!(Sha256Digest::of_bytes(b"missing NOT")),
        ),
        (
            "/payload_ref/sha256",
            json!(Sha256Digest::of_bytes(b"other payload")),
        ),
        ("/payload_ref/size_bytes", json!("1")),
        ("/kind", json!("cancel_requested")),
        ("/priority", json!("control")),
        ("/expires_at_unix_ms", json!("501")),
    ];
    for (pointer, value) in mutations {
        let mut mutated = base.clone();
        *mutated
            .pointer_mut(pointer)
            .expect("valid fixture or asserted JSON shape") = value;
        let mut decoded: CoordinationEnvelopeV2 =
            serde_json::from_value(mutated).expect("valid fixture or asserted JSON shape");
        // Recomputing caller-supplied scope claims must not defeat independent
        // expected scope or the exact externally retained header identity.
        let scope_digest = decoded
            .scope()
            .digest()
            .expect("valid fixture or asserted JSON shape");
        decoded.authority.scope_digest = scope_digest.clone();
        decoded.source_cursor.authorization_scope_digest = scope_digest;
        assert!(
            check_exact(&decoded, &expected).is_err(),
            "mutation {pointer}"
        );
    }
}

#[test]
fn payload_digest_preserves_negation_units_uncertainty_and_next_action() {
    let envelope = fixture();
    for changed in [
        b"Do publish; timeout <= 5 ms; uncertainty remains; next: inspect candidate.".as_slice(),
        b"Do NOT publish; timeout <= 5 s; uncertainty remains; next: inspect candidate.".as_slice(),
        b"Do NOT publish; timeout <= 5 ms; uncertainty resolved; next: inspect candidate."
            .as_slice(),
        b"Do NOT publish; timeout <= 5 ms; uncertainty remains; next: publish candidate."
            .as_slice(),
    ] {
        assert_eq!(
            envelope.verify_payload_bytes(changed),
            Err(EnvelopeError::PayloadMismatch)
        );
    }
    assert_eq!(
        envelope.verify_payload_bytes(&[]),
        Err(EnvelopeError::PayloadMismatch)
    );
}

#[test]
fn serde_rejects_missing_unknown_duplicate_and_unsupported_versions() {
    let value = serde_json::to_value(fixture()).expect("valid fixture or asserted JSON shape");
    for field in [
        "protocol_version",
        "message_id",
        "root_id",
        "correlation_id",
        "source_cursor",
        "authority",
        "protected_facts_digest",
        "payload_ref",
        "kind",
        "priority",
    ] {
        let mut changed = value.clone();
        changed
            .as_object_mut()
            .expect("valid fixture or asserted JSON shape")
            .remove(field);
        assert!(
            CoordinationEnvelopeV2::parse(&bytes(&changed)).is_err(),
            "missing {field}"
        );
    }
    for field in [
        "authorized",
        "authority_granted",
        "delivered",
        "accepted",
        "unknown_required_semantics",
    ] {
        let mut changed = value.clone();
        changed[field] = json!(true);
        assert!(CoordinationEnvelopeV2::parse(&bytes(&changed)).is_err());
    }
    for pointer in [
        "/authority",
        "/source_cursor",
        "/task_revision",
        "/payload_ref",
        "/recipient",
    ] {
        let mut changed = value.clone();
        changed
            .pointer_mut(pointer)
            .expect("valid fixture or asserted JSON shape")["authorized"] = json!(true);
        assert!(CoordinationEnvelopeV2::parse(&bytes(&changed)).is_err());
    }
    for version in [json!(1), json!(3), json!("2"), json!(2.0), Value::Null] {
        let mut changed = value.clone();
        changed["protocol_version"] = version;
        assert!(CoordinationEnvelopeV2::parse(&bytes(&changed)).is_err());
    }
    let raw = String::from_utf8(bytes(&value)).expect("valid fixture or asserted JSON shape");
    for needle in ["\"protocol_version\":2", "\"generation\":\"4\""] {
        assert!(raw.contains(needle));
        let duplicate = raw.replacen(needle, &format!("{needle},{needle}"), 1);
        assert!(CoordinationEnvelopeV2::parse(duplicate.as_bytes()).is_err());
    }
    let needle = format!("\"size_bytes\":\"{}\"", PAYLOAD.len());
    let duplicate = raw.replacen(&needle, &format!("{needle},{needle}"), 1);
    assert!(CoordinationEnvelopeV2::parse(duplicate.as_bytes()).is_err());
}

#[test]
fn decimal_counters_reject_numbers_leading_zeroes_signs_and_overflow() {
    let value = serde_json::to_value(fixture()).expect("valid fixture or asserted JSON shape");
    for pointer in [
        "/source_cursor/recovery_generation",
        "/source_cursor/event_seq",
        "/authority/generation",
        "/payload_ref/size_bytes",
        "/expires_at_unix_ms",
    ] {
        for bad in [
            json!(1),
            json!("01"),
            json!("+1"),
            json!("-1"),
            json!("1.0"),
            json!("18446744073709551616"),
            json!(" 1"),
            json!("1\n"),
        ] {
            let mut changed = value.clone();
            *changed
                .pointer_mut(pointer)
                .expect("valid fixture or asserted JSON shape") = bad;
            assert!(
                CoordinationEnvelopeV2::parse(&bytes(&changed)).is_err(),
                "{pointer}"
            );
        }
    }
    let mut changed = value;
    changed["authority"]["generation"] = json!("0");
    assert!(CoordinationEnvelopeV2::parse(&bytes(&changed)).is_err());
    assert!(AuthorityGeneration::new(0).is_err());
    assert_eq!(
        AuthorityGeneration::new(u64::MAX)
            .expect("valid fixture or asserted JSON shape")
            .get(),
        u64::MAX
    );
}

#[test]
fn exact_scope_and_authorization_scope_cannot_be_swapped() {
    let original = fixture();
    for pointer in [
        "/authority/scope_digest",
        "/source_cursor/authorization_scope_digest",
    ] {
        let mut changed =
            serde_json::to_value(&original).expect("valid fixture or asserted JSON shape");
        *changed
            .pointer_mut(pointer)
            .expect("valid fixture or asserted JSON shape") =
            json!(Sha256Digest::of_bytes(b"foreign scope"));
        assert!(matches!(
            CoordinationEnvelopeV2::parse(&bytes(&changed)),
            Err(EnvelopeError::Binding(_))
        ));
    }
    let mut other = original.clone();
    other.task_revision = None;
    assert!(other.validate().is_err());
    let digest = other
        .scope()
        .digest()
        .expect("valid fixture or asserted JSON shape");
    other.authority.scope_digest = digest.clone();
    other.source_cursor.authorization_scope_digest = digest;
    assert!(other.validate().is_ok()); // A taskless header is not task authority.
    assert!(check_exact(&other, &original).is_err());
}

#[test]
fn epoch_baseline_and_positive_revisions_are_fail_closed() {
    let value = serde_json::to_value(fixture()).expect("valid fixture or asserted JSON shape");
    for (pointer, bad) in [
        ("/context_epoch", Value::Null),
        ("/context_epoch", json!("")),
        ("/context_epoch", json!(" ")),
        ("/task_revision/revision", json!(0)),
        ("/source_cursor/projection_schema", json!(0)),
        ("/authority/reference", json!(" ")),
        ("/authority/reference", json!("a\nb")),
        (
            "/authority/reference",
            json!("a".repeat(MAX_AUTHORITY_REFERENCE_BYTES + 1)),
        ),
    ] {
        let mut changed = value.clone();
        *changed
            .pointer_mut(pointer)
            .expect("valid fixture or asserted JSON shape") = bad;
        assert!(
            CoordinationEnvelopeV2::parse(&bytes(&changed)).is_err(),
            "{pointer}"
        );
    }
    let mut fresh = fixture();
    fresh.context_epoch = None;
    fresh.baseline_digest = None;
    assert!(fresh.validate().is_ok());
    assert!(check_exact(&fresh, &fixture()).is_err());
    let mut self_caused = fixture();
    self_caused.causation_id = Some(self_caused.message_id.clone());
    assert!(self_caused.validate().is_err());
}

#[test]
fn canonical_identifiers_and_sha256_never_accept_aliases() {
    for bad in ["", ".", "..", "a/b", "a b", "é", "x\n"] {
        assert!(TopicId::new(bad).is_err());
        assert!(CorrelationId::new(bad).is_err());
        assert!(ContextEpoch::new(bad).is_err());
    }
    assert!(TopicId::new("x".repeat(257)).is_err());
    assert!(CorrelationId::new("x".repeat(129)).is_err());
    assert!(ContextEpoch::new("x".repeat(129)).is_err());
    for bad in [
        "sha256:a".to_owned(),
        format!("sha256:{}", "A".repeat(64)),
        format!("{}\n", Sha256Digest::of_bytes(b"x").as_str()),
        "x".repeat(71),
    ] {
        assert!(Sha256Digest::new(bad).is_err());
    }
}

#[test]
fn preflight_bounds_actual_escaped_bytes_and_depth_before_decoding() {
    let mut huge = fixture();
    huge.authority.reference = "\"".repeat(MAX_ENVELOPE_BYTES);
    assert_eq!(huge.validate(), Err(EnvelopeError::BoundExceeded));
    assert_eq!(
        CoordinationEnvelopeV2::parse(&vec![b' '; MAX_ENVELOPE_BYTES + 1]),
        Err(EnvelopeError::BoundExceeded)
    );
    let deep = format!(
        "{}0{}",
        "[".repeat(MAX_ENVELOPE_DEPTH + 1),
        "]".repeat(MAX_ENVELOPE_DEPTH + 1)
    );
    assert_eq!(
        CoordinationEnvelopeV2::parse(deep.as_bytes()),
        Err(EnvelopeError::BoundExceeded)
    );
    for malformed in [
        "{",
        "}",
        "\"unterminated",
        "[] []",
        "{]",
        "null",
        "[]",
        "{}",
        "",
    ] {
        assert!(CoordinationEnvelopeV2::parse(malformed.as_bytes()).is_err());
    }
    // Delimiters and escaped quotes inside strings are not structural depth.
    let mut valid = fixture();
    valid.authority.reference = "policy:[[[{{{\\\"".into();
    assert!(
        CoordinationEnvelopeV2::parse(
            &valid
                .canonical_bytes()
                .expect("valid fixture or asserted JSON shape")
        )
        .is_ok()
    );
}

#[test]
fn expiry_uses_only_explicit_observation_and_keeps_unknown_distinct() {
    let mut envelope = fixture();
    assert_eq!(envelope.expiry_at(None), Expiry::UnknownObservation);
    assert_eq!(
        envelope.expiry_at(Some(&DecimalU64::new(499))),
        Expiry::Live
    );
    assert_eq!(
        envelope.expiry_at(Some(&DecimalU64::new(500))),
        Expiry::Expired
    );
    assert_eq!(
        envelope.expiry_at(Some(&DecimalU64::new(u64::MAX))),
        Expiry::Expired
    );
    envelope.expires_at_unix_ms = Some(DecimalU64::new(u64::MAX));
    assert_eq!(
        envelope.expiry_at(Some(&DecimalU64::new(u64::MAX - 1))),
        Expiry::Live
    );
    envelope.expires_at_unix_ms = None;
    assert_eq!(envelope.expiry_at(None), Expiry::NoExpiry);
}

#[test]
fn all_sixteen_kind_labels_roundtrip_without_implying_body_or_authority() {
    let kinds = [
        CoordinationKind::TaskAssigned,
        CoordinationKind::TaskChanged,
        CoordinationKind::ProgressChanged,
        CoordinationKind::ArtifactReady,
        CoordinationKind::InterfaceProposed,
        CoordinationKind::InterfaceAccepted,
        CoordinationKind::QuestionAsked,
        CoordinationKind::QuestionAnswered,
        CoordinationKind::DecisionRequested,
        CoordinationKind::DecisionRecorded,
        CoordinationKind::VerificationFailed,
        CoordinationKind::HandoffReady,
        CoordinationKind::ClaimConflict,
        CoordinationKind::BudgetWarning,
        CoordinationKind::CancelRequested,
        CoordinationKind::SnapshotRequired,
    ];
    let mut observed = std::collections::BTreeSet::new();
    for kind in kinds {
        let mut envelope = fixture();
        envelope.kind = kind;
        let value = serde_json::to_value(kind)
            .expect("valid fixture or asserted JSON shape")
            .as_str()
            .expect("valid fixture or asserted JSON shape")
            .to_owned();
        assert!(observed.insert(value));
        let parsed = CoordinationEnvelopeV2::parse(
            &envelope
                .canonical_bytes()
                .expect("valid fixture or asserted JSON shape"),
        )
        .expect("valid fixture or asserted JSON shape");
        assert_eq!(parsed.kind, kind);
        assert!(check_exact(&parsed, &fixture()).is_err() || kind == fixture().kind);
    }
    assert_eq!(observed.len(), 16);
    let mut changed =
        serde_json::to_value(fixture()).expect("valid fixture or asserted JSON shape");
    changed["kind"] = json!("auto_execute");
    assert!(CoordinationEnvelopeV2::parse(&bytes(&changed)).is_err());
}

#[test]
fn schema_has_exact_v2_constant_closed_objects_and_canonical_counter() {
    let schema = serde_json::to_value(schemars::schema_for!(CoordinationEnvelopeV2))
        .expect("valid fixture or asserted JSON shape");
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["$defs"]["CoordinationProtocolVersion2"]["const"], 2);
    assert_eq!(
        schema["$defs"]["AuthorityRef"]["additionalProperties"],
        false
    );
    assert_eq!(schema["$defs"]["Cursor"]["additionalProperties"], false);
    assert_eq!(schema["$defs"]["DecimalU64"]["type"], "string");
    assert_eq!(
        schema["$defs"]["CoordinationAuthorityGeneration"]["allOf"][1]["not"]["const"],
        "0"
    );
    assert!(schema["properties"].get("schema_version").is_none());
    for field in ["message_id", "project_id", "root_id", "sender"] {
        assert_eq!(schema["properties"][field]["maxLength"], 128);
        assert_eq!(schema["properties"][field]["minLength"], 1);
        assert!(
            schema["properties"][field]["pattern"]
                .as_str()
                .expect("ID pattern")
                .contains("A-Za-z0-9")
        );
    }
}

#[test]
fn nullable_metadata_must_be_explicit_and_schema_keeps_nullability() {
    let schema = serde_json::to_value(schemars::schema_for!(CoordinationEnvelopeV2))
        .expect("valid fixture or asserted JSON shape");
    let value = serde_json::to_value(fixture()).expect("valid fixture or asserted JSON shape");
    for field in [
        "task_revision",
        "causation_id",
        "context_epoch",
        "baseline_digest",
        "expires_at_unix_ms",
    ] {
        let mut omitted = value.clone();
        omitted
            .as_object_mut()
            .expect("valid fixture or asserted JSON shape")
            .remove(field);
        assert!(
            CoordinationEnvelopeV2::parse(&bytes(&omitted)).is_err(),
            "omitted {field}"
        );
        assert!(
            schema["required"]
                .as_array()
                .expect("valid fixture or asserted JSON shape")
                .contains(&json!(field)),
            "schema required {field}"
        );
        assert!(
            schema["properties"][field].to_string().contains("null"),
            "schema nullable {field}"
        );
    }
    let mut explicit_nulls = value;
    for field in [
        "task_revision",
        "causation_id",
        "context_epoch",
        "baseline_digest",
        "expires_at_unix_ms",
    ] {
        explicit_nulls[field] = Value::Null;
    }
    let mut parsed: CoordinationEnvelopeV2 =
        serde_json::from_value(explicit_nulls).expect("valid fixture or asserted JSON shape");
    let scope = parsed
        .scope()
        .digest()
        .expect("valid fixture or asserted JSON shape");
    parsed.authority.scope_digest = scope.clone();
    parsed.source_cursor.authorization_scope_digest = scope;
    assert!(
        CoordinationEnvelopeV2::parse(
            &parsed
                .canonical_bytes()
                .expect("valid fixture or asserted JSON shape")
        )
        .is_ok()
    );
}

#[test]
fn closed_v1_bytes_and_v1_version_rejection_remain_unchanged() {
    let message = CoordinationMessage {
        schema_version: 1,
        message_id: MessageId::new("m1").expect("valid fixture or asserted JSON shape"),
        project_id: ProjectId::new("p1").expect("valid fixture or asserted JSON shape"),
        sender: PrincipalId::new("worker").expect("valid fixture or asserted JSON shape"),
        recipient: PrincipalId::new("commodore").expect("valid fixture or asserted JSON shape"),
        causation_id: None,
        context_epoch: "epoch-1".into(),
        authority_ref: "grant:fixture".into(),
        payload: MessagePayload::TaskAssigned {
            task_id: TaskId::new("t1").expect("valid fixture or asserted JSON shape"),
            revision: 1,
            contract_digest: "digest".into(),
        },
    };
    let raw = serde_json::to_string(&message).expect("valid fixture or asserted JSON shape");
    assert_eq!(
        raw,
        r#"{"schema_version":1,"message_id":"m1","project_id":"p1","sender":"worker","recipient":"commodore","causation_id":null,"context_epoch":"epoch-1","authority_ref":"grant:fixture","payload":{"kind":"task_assigned","task_id":"t1","revision":1,"contract_digest":"digest"}}"#
    );
    assert!(CoordinationEnvelopeV2::parse(raw.as_bytes()).is_err());
    assert!(
        serde_json::from_slice::<CoordinationMessage>(
            &fixture()
                .canonical_bytes()
                .expect("valid fixture or asserted JSON shape")
        )
        .is_err()
    );
}
