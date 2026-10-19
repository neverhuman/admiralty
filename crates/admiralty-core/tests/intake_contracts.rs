use admiralty_core::{
    AcceptanceCriterion, CheckSpec, CommandId, PrincipalId, ProjectId, RootId, TaskContract,
    TaskId, content_digest, intake::*,
};
use serde_json::json;
fn provenance() -> SourceProvenance {
    SourceProvenance {
        schema_version: 1,
        source_id: "paste-1".into(),
        revision: "1".into(),
        captured_at: "2026-10-19T00:00:00Z".into(),
        requester: RequesterAttribution::Unknown {},
        origin: None,
    }
}
fn scope() -> AuthorizedIntakeScope {
    AuthorizedIntakeScope {
        principal: PrincipalId::new("authenticated-operator").expect("valid intake fixture"),
        project_id: Some(ProjectId::new("p").expect("valid intake fixture")),
        root_id: Some(RootId::new("r").expect("valid intake fixture")),
        authority_ref: "scope:intake".into(),
        authority_generation: 7,
    }
}
fn preview(raw: &[u8], format: IntakeFormat) -> Result<IntakePreview, IntakeError> {
    preview_intake(
        raw,
        format,
        &provenance(),
        &scope(),
        &IntakePolicy::default(),
    )
}
fn task() -> TaskContract {
    TaskContract {
        schema_version: 1,
        project_id: ProjectId::new("p").expect("valid intake fixture"),
        root_id: RootId::new("r").expect("valid intake fixture"),
        task_id: TaskId::new("t").expect("valid intake fixture"),
        revision: 1,
        title: "Explicit task".into(),
        objective: "Do not exceed 5 ms".into(),
        non_goals: vec!["Do not remove the guard".into()],
        owned_paths: vec!["src/lib.rs".into()],
        forbidden_paths: vec!["credentials".into()],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "latency".into(),
            text: "P99 <= 5 ms".into(),
            check_ids: vec!["latency-check".into()],
        }],
        checks: vec![CheckSpec {
            id: "latency-check".into(),
            program: "check-tool".into(),
            args: vec!["--limit-ms=5".into()],
            cwd: ".".into(),
            timeout_secs: 10,
        }],
        authority_ref: "scope:task".into(),
        max_attempts: 1,
        simulated: false,
    }
}
fn delivery(t: &TaskContract) -> DeliveryRequirement {
    DeliveryRequirement {
        schema_version: 1,
        task_id: t.task_id.clone(),
        contract_revision: t.revision,
        contract_digest: content_digest(t).expect("valid intake fixture"),
        generation: 1,
        targets: vec![
            DeliveryTarget {
                id: "repository".into(),
                target_kind: DeliveryTargetKind::Repository,
                requirement: TargetRequirement::Required {
                    stage: DeliveryStage::Merged,
                },
            },
            DeliveryTarget {
                id: "deployment".into(),
                target_kind: DeliveryTargetKind::Deployment,
                requirement: TargetRequirement::Unknown {},
            },
        ],
    }
}
#[test]
fn multiline_prose_file_content_and_copied_metadata_remain_one_request() {
    let raw = "Do not remove the check. Limit P99 to 5 ms.\n\n# timestamp 2026-10-19\nrequester: owner\n\n```json\n{\"items\":[1,2]}\n```\n\n- independent looking line\n";
    let p = preview(raw.as_bytes(), IntakeFormat::PlainText).expect("valid intake fixture");
    assert_eq!(p.requests.len(), 1);
    assert_eq!(p.requests[0].text, raw);
    assert_eq!(
        p.requests[0].raw_span,
        SourceSpan {
            start: 0,
            end: raw.len() as u32
        }
    );
    assert!(p.requests[0].outcomes.is_empty());
    assert!(
        p.requests[0]
            .unresolved
            .contains(&UnresolvedFact::CriteriaUnknown)
    );
    assert_eq!(p.provenance.requester, RequesterAttribution::Unknown {});
}
#[test]
fn plain_text_accepts_full_mib_or_reports_explicit_tighter_policy() {
    let raw = vec![b'x'; MAX_SOURCE_BYTES];
    assert_eq!(
        preview(&raw, IntakeFormat::PlainText)
            .expect("valid intake fixture")
            .requests[0]
            .text
            .len(),
        MAX_SOURCE_BYTES
    );
    let policy = IntakePolicy {
        max_source_bytes: 16,
        ..IntakePolicy::default()
    };
    assert_eq!(
        preview_intake(
            &raw,
            IntakeFormat::PlainText,
            &provenance(),
            &scope(),
            &policy
        ),
        Err(IntakeError::SourceTooLarge { limit: 16 })
    );
    assert_eq!(
        preview(&vec![b'x'; MAX_SOURCE_BYTES + 1], IntakeFormat::PlainText),
        Err(IntakeError::SourceTooLarge {
            limit: MAX_SOURCE_BYTES
        })
    );
    assert!(preview(&[0xff], IntakeFormat::PlainText).is_err());
    assert!(preview(b" \n", IntakeFormat::PlainText).is_err());
}
#[test]
fn only_explicit_batch_multiplies_and_raw_unicode_item_ranges_are_exact() {
    let raw = "{\"schema_version\":1,\"items\":[{\"text\":\"☃\\nnever remove\"}, {\"text\":\"second\\n\\nparagraph\"}]}";
    let p = preview(raw.as_bytes(), IntakeFormat::JsonBatch).expect("valid intake fixture");
    assert_eq!(p.requests.len(), 2);
    assert_eq!(p.requests[0].text, "☃\nnever remove");
    for r in &p.requests {
        let slice = &raw[r.raw_span.start as usize..r.raw_span.end as usize];
        assert!(slice.starts_with('{') && slice.ends_with('}'));
        let item: IntakeItem = serde_json::from_str(slice).expect("valid intake fixture");
        assert_eq!(item.text, r.text);
    }
    assert_eq!(
        preview(raw.as_bytes(), IntakeFormat::PlainText)
            .expect("valid intake fixture")
            .requests
            .len(),
        1
    );
    assert!(preview(br#"[{"text":"a"},{"text":"b"}]"#, IntakeFormat::JsonBatch).is_err());
}
#[test]
fn outcome_spans_use_raw_utf8_not_unescaped_text_coordinates() {
    let make = |span: SourceSpan| {
        format!(
            "{{\"schema_version\":1,\"items\":[{{\"text\":\"☃\\nrequirement\",\"outcomes\":[{{\"id\":\"o\",\"description\":\"Explicit only\",\"source_spans\":[{{\"start\":{},\"end\":{}}}]}}]}}]}}",
            span.start, span.end
        )
    };
    let sample = make(SourceSpan { start: 0, end: 1 });
    let at = sample.find('☃').expect("valid intake fixture") as u32;
    let good = make(SourceSpan {
        start: at,
        end: at + 3,
    });
    let p = preview(good.as_bytes(), IntakeFormat::JsonBatch).expect("valid intake fixture");
    let outcome = &p.requests[0].outcomes[0];
    assert_eq!(&good[at as usize..at as usize + 3], "☃");
    assert!(
        outcome
            .unresolved
            .contains(&UnresolvedFact::CriteriaUnknown)
    );
    assert_eq!(
        preview(
            make(SourceSpan {
                start: at,
                end: at + 1
            })
            .as_bytes(),
            IntakeFormat::JsonBatch
        ),
        Err(IntakeError::Span)
    );
    assert_eq!(
        preview(
            make(SourceSpan { start: 0, end: 1 }).as_bytes(),
            IntakeFormat::JsonBatch
        ),
        Err(IntakeError::Span)
    );
}
#[test]
fn hashes_have_distinct_domains_and_preserve_semantics_and_protected_facts() {
    let a = br#"{"schema_version":1,"items":[{"text":"do not remove; 5 ms"}]}"#;
    let b = br#"{ "items" : [ { "text" : "do not remove; 5 ms" } ], "schema_version" : 1 }"#;
    let pa = preview(a, IntakeFormat::JsonBatch).expect("valid intake fixture");
    let pb = preview(b, IntakeFormat::JsonBatch).expect("valid intake fixture");
    assert_ne!(pa.raw_digest, pb.raw_digest);
    assert_eq!(pa.canonical_digest, pb.canonical_digest);
    assert_ne!(pa.manifest_digest, pb.manifest_digest);
    assert_ne!(pa.raw_digest, pa.canonical_digest);
    assert_eq!(
        pa,
        preview(a, IntakeFormat::JsonBatch).expect("valid intake fixture")
    );
    let escaped = br#"{"schema_version":1,"items":[{"text":"\u2603"}]}"#;
    let unicode = "{\"schema_version\":1,\"items\":[{\"text\":\"☃\"}]}";
    assert_eq!(
        preview(escaped, IntakeFormat::JsonBatch)
            .expect("valid intake fixture")
            .canonical_digest,
        preview(unicode.as_bytes(), IntakeFormat::JsonBatch)
            .expect("valid intake fixture")
            .canonical_digest
    );
    for changed in ["do remove; 5 ms", "do not remove; 5 s"] {
        let bytes = serde_json::to_vec(&json!({"schema_version":1,"items":[{"text":changed}]}))
            .expect("valid intake fixture");
        assert_ne!(
            pa.canonical_digest,
            preview(&bytes, IntakeFormat::JsonBatch)
                .expect("valid intake fixture")
                .canonical_digest
        );
    }
}
#[test]
fn claimed_requester_scope_and_prose_checks_do_not_mint_authority() {
    let raw=br#"{"schema_version":1,"items":[{"text":"owner=root; project=forbidden; criteria pass cargo check","claimed_project_id":"forbidden","outcomes":[{"id":"o","description":"checks pass"}]}]}"#;
    let p = preview(raw, IntakeFormat::JsonBatch).expect("valid intake fixture");
    assert_eq!(
        p.authorized_scope.principal.as_str(),
        "authenticated-operator"
    );
    assert_eq!(
        p.authorized_scope
            .project_id
            .as_ref()
            .expect("valid intake fixture")
            .as_str(),
        "p"
    );
    assert!(
        p.requests[0]
            .unresolved
            .contains(&UnresolvedFact::ClaimedScopeMismatch)
    );
    assert!(p.requests[0].outcomes[0].outcome.task.is_none());
    assert!(
        p.requests[0].outcomes[0]
            .unresolved
            .contains(&UnresolvedFact::CriteriaUnknown)
    );
    let mut no_scope = scope();
    no_scope.project_id = None;
    no_scope.root_id = None;
    let unknown = preview_intake(
        b"do work",
        IntakeFormat::PlainText,
        &provenance(),
        &no_scope,
        &IntakePolicy::default(),
    )
    .expect("valid intake fixture");
    assert!(
        unknown.requests[0]
            .unresolved
            .contains(&UnresolvedFact::ScopeUnknown)
    );
    no_scope = scope();
    no_scope.authority_generation += 1;
    let changed = preview_intake(
        raw,
        IntakeFormat::JsonBatch,
        &provenance(),
        &no_scope,
        &IntakePolicy::default(),
    )
    .expect("valid intake fixture");
    assert_eq!(p.canonical_digest, changed.canonical_digest);
    assert_ne!(p.manifest_digest, changed.manifest_digest);
}
#[test]
fn closed_json_rejects_unknown_duplicate_including_escaped_duplicate_keys() {
    for raw in [
        r#"{"schema_version":1,"items":[{"text":"a","unknown":true}]}"#,
        r#"{"schema_version":1,"items":[{"text":"a","text":"b"}]}"#,
        r#"{"schema_version":1,"items":[{"text":"a","te\u0078t":"b"}]}"#,
        r#"{"schema_version":1,"schema_version":1,"items":[]}"#,
    ] {
        assert!(
            preview(raw.as_bytes(), IntakeFormat::JsonBatch).is_err(),
            "{raw}"
        );
    }
    assert_eq!(
        preview(
            br#"{"schema_version":1,"items":[{"text":"a","text":"b"}]}"#,
            IntakeFormat::JsonBatch
        ),
        Err(IntakeError::DuplicateKey)
    );
    let mut t = serde_json::to_value(task()).expect("valid intake fixture");
    t["delivery_requirement"] = json!({});
    assert!(serde_json::from_value::<TaskContract>(t).is_err());
}
#[test]
fn bounded_depth_counts_fields_references_and_policy_fail_closed() {
    let nested = format!(
        "{{\"schema_version\":1,\"items\":[],\"unknown\":{}0{}}}",
        "[".repeat(33),
        "]".repeat(33)
    );
    assert_eq!(
        preview(nested.as_bytes(), IntakeFormat::JsonBatch),
        Err(IntakeError::Bound)
    );
    let count = serde_json::to_vec(
        &json!({"schema_version":1,"items":vec![json!({"text":"a"});MAX_REQUESTS+1]}),
    )
    .expect("valid intake fixture");
    assert_eq!(
        preview(&count, IntakeFormat::JsonBatch),
        Err(IntakeError::Bound)
    );
    let too_long = serde_json::to_vec(
        &json!({"schema_version":1,"items":[{"text":"x".repeat(MAX_JSON_ITEM_TEXT_BYTES+1)}]}),
    )
    .expect("valid intake fixture");
    assert!(matches!(
        preview(&too_long, IntakeFormat::JsonBatch),
        Err(IntakeError::TextTooLarge { .. })
    ));
    let mut p = provenance();
    p.origin = Some(OriginReference {
        system: "external".into(),
        record_id: "../../secret".into(),
        revision: "1".into(),
    });
    assert!(
        preview_intake(
            b"a",
            IntakeFormat::PlainText,
            &p,
            &scope(),
            &IntakePolicy::default()
        )
        .is_err()
    );
    p.origin.as_mut().expect("valid intake fixture").record_id = "javascript:alert(1)".into();
    assert!(
        preview_intake(
            b"a",
            IntakeFormat::PlainText,
            &p,
            &scope(),
            &IntakePolicy::default()
        )
        .is_err()
    );
    let policy = IntakePolicy {
        max_requests: MAX_REQUESTS + 1,
        ..IntakePolicy::default()
    };
    assert_eq!(
        preview_intake(
            b"a",
            IntakeFormat::PlainText,
            &provenance(),
            &scope(),
            &policy
        ),
        Err(IntakeError::Bound)
    );
}
#[test]
fn delivery_is_generation_and_exact_contract_bound_with_unknown_distinct() {
    let t = task();
    let mut requirement = delivery(&t);
    assert!(requirement.binds(&t).expect("valid intake fixture"));
    assert!(matches!(
        requirement.targets[1].requirement,
        TargetRequirement::Unknown {}
    ));
    let unknown =
        serde_json::to_value(&requirement.targets[1].requirement).expect("valid intake fixture");
    assert_eq!(unknown, json!({"kind":"unknown"}));
    assert!(serde_json::from_value::<TargetRequirement>(json!(null)).is_err());
    assert!(
        serde_json::from_value::<TargetRequirement>(json!({"kind":"unknown","stage":"released"}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<RequesterAttribution>(
            json!({"kind":"unknown","requester":"root"})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<DuplicateCoverage>(
            json!({"kind":"complete","continuation":"cursor:next"})
        )
        .is_err()
    );
    for value in [
        TargetRequirement::Required {
            stage: DeliveryStage::Released,
        },
        TargetRequirement::Optional {
            stage: DeliveryStage::Released,
        },
        TargetRequirement::NotApplicable {
            reason: "source-only task".into(),
        },
    ] {
        assert_ne!(
            unknown,
            serde_json::to_value(value).expect("valid intake fixture")
        );
    }
    let mut changed = t.clone();
    changed.criteria[0].text = "P99 <= 50 ms".into();
    assert!(!requirement.binds(&changed).expect("valid intake fixture"));
    requirement.generation = 0;
    assert!(requirement.validate().is_err());
    requirement = delivery(&t);
    requirement.targets.push(requirement.targets[0].clone());
    assert!(requirement.validate().is_err());
}
#[test]
fn explicit_contract_outcome_keeps_named_checks_constraints_and_no_inference() {
    let t = task();
    let raw=serde_json::to_vec(&json!({"schema_version":1,"items":[{"text":"never remove; <= 5 ms","requested_timing":"after the named check; timezone unresolved","constraints":["preserve the global guard"],"outcomes":[{"id":"o","description":"explicit outcome","constraints":["never remove","<= 5 ms"],"task":t,"delivery":delivery(&t)}]}]})).expect("valid intake fixture");
    let p = preview(&raw, IntakeFormat::JsonBatch).expect("valid intake fixture");
    let o = &p.requests[0].outcomes[0];
    assert_eq!(p.requests[0].constraints, vec!["preserve the global guard"]);
    assert_eq!(
        p.requests[0].requested_timing.as_deref(),
        Some("after the named check; timezone unresolved")
    );
    assert_eq!(o.outcome.task.as_ref().expect("valid intake fixture"), &t);
    assert_eq!(
        o.contract_digest.as_ref().expect("valid intake fixture"),
        &content_digest(&t).expect("valid intake fixture")
    );
    assert_eq!(o.outcome.constraints, vec!["never remove", "<= 5 ms"]);
    assert!(o.unresolved.contains(&UnresolvedFact::SourceSpansUnknown));
    assert!(o.unresolved.contains(&UnresolvedFact::DeliveryUnknown));
    assert!(!o.unresolved.contains(&UnresolvedFact::CriteriaUnknown));
}
#[test]
fn duplicate_pages_are_bounded_query_bound_and_never_imply_exhaustive_similarity() {
    let p = preview(b"a", IntakeFormat::PlainText).expect("valid intake fixture");
    let query = DuplicateQuery {
        schema_version: 1,
        project_id: ProjectId::new("p").expect("valid intake fixture"),
        raw_digest: p.raw_digest,
        canonical_digest: p.canonical_digest,
        origin: None,
        command_id: None,
        cursor: None,
        limit: 10,
    };
    let mut page = DuplicatePage {
        schema_version: 1,
        query_digest: query.identity_digest().expect("valid intake fixture"),
        candidates: vec![],
        coverage: DuplicateCoverage::Partial {
            continuation: "cursor:next".into(),
        },
    };
    page.validate_for(&query).expect("valid intake fixture");
    page.coverage = DuplicateCoverage::Incomplete {
        reason: "bounded scan budget exhausted".into(),
    };
    page.validate_for(&query).expect("valid intake fixture");
    let mut other = query.clone();
    other.project_id = ProjectId::new("other").expect("valid intake fixture");
    assert_eq!(page.validate_for(&other), Err(IntakeError::Binding));
    other.limit = 101;
    assert_eq!(other.validate(), Err(IntakeError::Bound));
    assert_ne!(
        serde_json::to_value(DuplicateReason::ExactSourceIdentity).expect("valid intake fixture"),
        serde_json::to_value(DuplicateReason::AdvisoryCanonicalContent)
            .expect("valid intake fixture")
    );
}
fn record() -> RecordIntakeRequest {
    let raw = "Do not remove the check; 5 ms";
    let p = preview(raw.as_bytes(), IntakeFormat::PlainText).expect("valid intake fixture");
    let t = task();
    let d = delivery(&t);
    let binding = WorkOrderTaskBinding {
        work_order_id: "wo-1".into(),
        work_order_revision: 1,
        request_index: 0,
        outcome_id: "o".into(),
        outcome_digest: content_digest(&"explicit outcome").expect("valid intake fixture"),
        source_spans: vec![SourceSpan {
            start: 0,
            end: raw.len() as u32,
        }],
        task_id: t.task_id.clone(),
        contract_revision: t.revision,
        contract_digest: content_digest(&t).expect("valid intake fixture"),
    };
    RecordIntakeRequest {
        schema_version: 1,
        command_id: CommandId::new("record-1").expect("valid intake fixture"),
        expected_work_order_revision: 0,
        plan_id: "plan".into(),
        expected_plan_revision: 0,
        preview_manifest_digest: p.manifest_digest,
        policy_revision: p.policy_revision,
        authority_generation: 7,
        work_order: WorkOrderRevision {
            schema_version: 1,
            id: "wo-1".into(),
            revision: 1,
            scope: p.authorized_scope,
            provenance: p.provenance,
            title: None,
            outcome: None,
            rationale: None,
            constraints: vec![],
            requested_timing: None,
            raw_digest: p.raw_digest,
            canonical_digest: p.canonical_digest,
            parser_revision: p.parser_revision,
            source_payload: SourcePayload::InlineUtf8 { text: raw.into() },
        },
        selected_tasks: vec![SelectedTask {
            binding,
            task: t,
            delivery: d,
        }],
    }
}
#[test]
fn record_contract_binds_source_scope_cas_and_exact_contract_but_grants_nothing() {
    let r = record();
    r.validate().expect("valid intake fixture");
    let bytes = serde_json::to_vec(&r).expect("valid intake fixture");
    assert_eq!(
        parse_record_request(&bytes).expect("valid intake fixture"),
        r
    );
    let mut changed = r.clone();
    changed.work_order.source_payload = SourcePayload::InlineUtf8 {
        text: "remove the check; 5 s".into(),
    };
    assert_eq!(changed.validate(), Err(IntakeError::Binding));
    changed = r.clone();
    changed.expected_work_order_revision = 2;
    assert_eq!(changed.validate(), Err(IntakeError::Binding));
    changed = r.clone();
    changed.authority_generation = 8;
    assert_eq!(changed.validate(), Err(IntakeError::Binding));
    changed = r.clone();
    changed.selected_tasks[0].task.checks[0].args = vec!["--limit-ms=50".into()];
    assert_eq!(changed.validate(), Err(IntakeError::Binding));
    changed = r.clone();
    changed
        .selected_tasks
        .push(changed.selected_tasks[0].clone());
    assert_eq!(changed.validate(), Err(IntakeError::Binding));
    let mut value = serde_json::to_value(r).expect("valid intake fixture");
    value["dispatch"] = json!(true);
    assert!(
        parse_record_request(&serde_json::to_vec(&value).expect("valid intake fixture")).is_err()
    );
}

#[test]
fn record_inline_source_does_not_inherit_json_batch_item_text_limit() {
    let mut request = record();
    let raw = "x".repeat(MAX_JSON_ITEM_TEXT_BYTES + 1);
    let p = preview(raw.as_bytes(), IntakeFormat::PlainText).expect("valid intake fixture");
    request.work_order.raw_digest = p.raw_digest;
    request.work_order.canonical_digest = p.canonical_digest;
    request.work_order.source_payload = SourcePayload::InlineUtf8 { text: raw };
    request.validate().expect("valid intake fixture");
    let wire = serde_json::to_vec(&request).expect("valid intake fixture");
    assert!(wire.len() < MAX_SOURCE_BYTES);
    assert_eq!(
        parse_record_request(&wire).expect("valid intake fixture"),
        request
    );
}

#[test]
fn source_bound_preview_roundtrip_and_schema_preserve_protected_selection() {
    let raw=br#"{"schema_version":1,"items":[{"text":"never remove; 5 ms","outcomes":[{"id":"o","description":"explicit outcome","constraints":["never remove"]}]}]}"#;
    let p = preview(raw, IntakeFormat::JsonBatch).expect("valid intake fixture");
    let encoded = serde_json::to_vec(&p).expect("valid intake fixture");
    let decoded: IntakePreview = serde_json::from_slice(&encoded).expect("valid intake fixture");
    decoded
        .verify_source(raw, &scope(), &IntakePolicy::default())
        .expect("valid intake fixture");
    let schema = schemars::schema_for!(IntakePreview);
    assert_eq!(
        serde_json::to_value(schema).expect("valid intake fixture")["additionalProperties"],
        false
    );
    let mut changed = decoded.clone();
    changed.requests[0].outcomes[0].outcome.constraints[0] = "remove".into();
    assert_eq!(
        changed.verify_source(raw, &scope(), &IntakePolicy::default()),
        Err(IntakeError::Binding)
    );
    changed = decoded.clone();
    changed.schema_version = 2;
    assert_eq!(
        changed.verify_source(raw, &scope(), &IntakePolicy::default()),
        Err(IntakeError::Binding)
    );
    changed = decoded;
    changed.requests[0].outcomes[0].outcome_digest =
        content_digest(&"forged").expect("valid intake fixture");
    assert_eq!(
        changed.verify_source(raw, &scope(), &IntakePolicy::default()),
        Err(IntakeError::Binding)
    );
}

#[test]
fn outcome_collection_and_typed_contract_bounds_precede_hash_or_clone() {
    let bytes=serde_json::to_vec(&json!({"schema_version":1,"items":[{"text":"a","outcomes":vec![json!({"id":"o","description":"d"});MAX_OUTCOMES+1]}]})).expect("valid intake fixture");
    assert_eq!(
        preview(&bytes, IntakeFormat::JsonBatch),
        Err(IntakeError::Bound)
    );
    let mut request = record();
    request.selected_tasks[0].task.checks[0].args = vec!["x".into(); 129];
    assert_eq!(request.validate(), Err(IntakeError::Bound));
    let policy = IntakePolicy {
        max_json_item_text_bytes: 3,
        ..IntakePolicy::default()
    };
    let bytes = br#"{"schema_version":1,"items":[{"text":"four"}]}"#;
    assert_eq!(
        preview_intake(
            bytes,
            IntakeFormat::JsonBatch,
            &provenance(),
            &scope(),
            &policy
        ),
        Err(IntakeError::TextTooLarge { limit: 3 })
    );
}
