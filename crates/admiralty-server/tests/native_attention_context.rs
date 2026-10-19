use admiralty_core::{
    DomainEvent, PrincipalId, ProjectId, RootId, SourceIdentity, TaskContract, content_digest,
    coordination::context::{
        BudgetObservation, CheckObservation, FactObservation, ProjectExceptionFacts,
        ProtectedTaskFacts, RecipientRole, compile_project_exceptions,
    },
    governor::{AttentionUpdate, BlockState, GovernorConfig, Recipient, Role, Scope, UpdateKind},
};
use admiralty_server::native_attention_context::*;
use admiralty_store::{
    Store, StoreAuthorities, StoreError,
    attention::*,
    context_renders::{
        AdvanceContextBaseline, ContextNamespace, ContextScope, RecordContextRender, RenderKey,
        StoredContextRender,
    },
    outbox::OutboxIntent,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    any::Any,
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

struct PrivateProof(Arc<()>);
struct Authority {
    definition: AttentionDefinition,
    incarnation: Arc<()>,
    allowed: Mutex<BTreeSet<String>>,
    revoked: AtomicBool,
    read_calls: AtomicUsize,
    deny_final: AtomicBool,
    allow_sources: AtomicBool,
}
impl Authority {
    fn admit(&self, request: &AuthorizedAttentionCommand) {
        self.allowed.lock().expect("valid owned fixture").insert(
            authorized_request_digest(
                &content_digest(&self.definition).expect("valid owned fixture"),
                request,
            )
            .expect("valid owned fixture"),
        );
    }
}
impl AttentionAuthority for Authority {
    fn authorize_definition(&self, b: &AttentionDefinitionBinding<'_>) -> bool {
        !self.revoked.load(Ordering::SeqCst) && b.definition == &self.definition
    }
    fn authorize_command(
        &self,
        b: &AttentionCommandBinding<'_>,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        proof
            .downcast_ref::<PrivateProof>()
            .is_some_and(|p| Arc::ptr_eq(&p.0, &self.incarnation))
            && !self.revoked.load(Ordering::SeqCst)
            && b.request.binding.actor == self.definition.scope.recipients[0].principal_id
            && self
                .allowed
                .lock()
                .expect("valid owned fixture")
                .contains(b.request_digest)
    }
    fn authorize_delivery_context(
        &self,
        b: &AttentionCommandBinding<'_>,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        let call = self.read_calls.fetch_add(1, Ordering::SeqCst);
        self.authorize_command(b, proof)
            && (b.request.command.command_id == "take" || self.allow_sources.load(Ordering::SeqCst))
            && !(self.deny_final.load(Ordering::SeqCst) && call == 3)
    }
}
/// No read callback: current recording permission must not become a read grant.
struct RecordingOnly(Arc<Authority>);
impl AttentionAuthority for RecordingOnly {
    fn authorize_definition(&self, b: &AttentionDefinitionBinding<'_>) -> bool {
        self.0.authorize_definition(b)
    }
    fn authorize_command(
        &self,
        b: &AttentionCommandBinding<'_>,
        p: &(dyn Any + Send + Sync),
    ) -> bool {
        self.0.authorize_command(b, p)
    }
}

struct Fixture {
    store: Store,
    authority: Arc<Authority>,
    proof: AttentionProof,
    expected: SafeTurnExpectation,
    source_render: StoredContextRender,
    request_render: StoredContextRender,
    update: AttentionUpdate,
    _dir: tempfile::TempDir,
}
fn fixture(stale_contract: bool, recording_only: bool) -> Fixture {
    fixture_with_source_count(stale_contract, recording_only, 1)
}
fn fixture_with_source_count(
    stale_contract: bool,
    recording_only: bool,
    source_count: usize,
) -> Fixture {
    fixture_options(stale_contract, recording_only, source_count, true)
}
fn fixture_options(
    stale_contract: bool,
    recording_only: bool,
    source_count: usize,
    register_task: bool,
) -> Fixture {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("valid owned fixture");
    let project = ProjectId::new("project").expect("valid owned fixture");
    let root = RootId::new("root").expect("valid owned fixture");
    let principal = PrincipalId::new("commodore").expect("valid owned fixture");
    let recipient = Recipient {
        role: Role::Captain,
        principal_id: principal.clone(),
    };
    let definition = AttentionDefinition {
        scope: AttentionScope {
            journal_id: "journal".into(),
            project_id: project.clone(),
            root_id: root.clone(),
            epoch: "epoch".into(),
            recipients: vec![recipient.clone()],
        },
        config: GovernorConfig::default(),
    };
    let authority = Arc::new(Authority {
        definition: definition.clone(),
        incarnation: Arc::new(()),
        allowed: Mutex::new(BTreeSet::new()),
        revoked: AtomicBool::new(false),
        read_calls: AtomicUsize::new(0),
        deny_final: AtomicBool::new(false),
        allow_sources: AtomicBool::new(true),
    });
    let proof: AttentionProof = Arc::new(PrivateProof(authority.incarnation.clone()));
    let installed: Arc<dyn AttentionAuthority> = if recording_only {
        Arc::new(RecordingOnly(authority.clone()))
    } else {
        authority.clone()
    };
    let store = Store::open_with_startup_authorities(
        dir.path().join("store.sqlite"),
        StoreAuthorities {
            attention: Some(installed),
            ..StoreAuthorities::default()
        },
    )
    .expect("valid owned fixture");
    let contract: TaskContract = serde_json::from_value(json!({
        "schema_version":1,"project_id":"project","root_id":"root","task_id":"task","revision":1,
        "title":"Exact protected task","objective":"Do NOT publish; preserve 5 ms and 20 GiB",
        "non_goals":["Never claim unknown usage is zero"],"owned_paths":["src"],"forbidden_paths":["private"],"read_first":[],"dependencies":[],
        "criteria":[{"id":"C1","text":"Preserve exact facts","check_ids":["unit"]}],
        "checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":10}],
        "authority_ref":"grant:task","max_attempts":1,"simulated":true
    })).expect("valid owned fixture");
    let mut current = contract.clone();
    if stale_contract {
        current.objective = "Changed current contract without retained source update".into();
    }
    if register_task {
        store
            .append(
                SourceIdentity {
                    producer_id: "fixture".into(),
                    incarnation: "boot".into(),
                    sequence: "1".into(),
                },
                DomainEvent::TaskSubmitted { contract: current },
                "2026-10-19T04:30:00Z".into(),
            )
            .expect("valid owned fixture");
    }
    let mut facts = ProjectExceptionFacts {
        project_id: project.clone(),
        revision: 1,
        authority_ref: "grant:project".into(),
        source_ref: "original:source".into(),
        exceptions: vec![ProtectedTaskFacts {
            contract: contract.clone(),
            immutable_refs: vec![],
            candidate: FactObservation::Unknown {
                reason: "No observed candidate".into(),
            },
            check: CheckObservation::NotRun {
                reason: "No actual independent checks".into(),
            },
            evidence: vec!["evidence:original".into()],
            uncertainty: vec!["Do NOT assume acceptance".into()],
            budget: BudgetObservation::Unknown {
                reason: "Usage not measured".into(),
            },
            next_action: "Inspect 5 ms failure without publishing".into(),
        }],
        uncertainty: vec!["A source-only uncertainty".into()],
        next_action: "Resolve actual blocker".into(),
    };
    let scope = ContextScope {
        recipient: principal.clone(),
        role: RecipientRole::Commodore,
        namespace: ContextNamespace::Project {
            project_id: project,
            root_id: root,
        },
    };
    let render = |name: &str, facts: &ProjectExceptionFacts| {
        let capsule =
            compile_project_exceptions(facts, "epoch", 16384).expect("valid owned fixture");
        let receipt = store
            .record_context_render(RecordContextRender {
                scope: scope.clone(),
                key: RenderKey {
                    render_id: name.into(),
                    version: 1,
                },
                expected_epoch: "epoch".into(),
                expected_facts_digest: capsule.facts_digest.clone(),
                capsule: capsule.clone(),
            })
            .expect("valid owned fixture");
        let stored = store
            .context_render(scope.clone(), receipt.render.key.clone())
            .expect("valid owned fixture")
            .expect("valid owned fixture");
        (
            AttentionContextPin {
                scope: scope.clone(),
                render: receipt.render,
                facts_digest: capsule.facts_digest,
            },
            stored,
        )
    };
    let (source_pin, source_render) = render("source-a", &facts);
    facts.uncertainty = vec!["B request-context uncertainty must also survive".into()];
    let (request_pin, request_render) = render("request-b", &facts);
    store
        .register_attention(definition.clone())
        .expect("valid owned fixture");
    let update = AttentionUpdate {
        source: SourceIdentity {
            producer_id: "trusted-source".into(),
            incarnation: "source-boot".into(),
            sequence: "1".into(),
        },
        scope: Scope {
            project_id: definition.scope.project_id.clone(),
            task_id: contract.task_id.clone(),
            revision: 1,
        },
        recipient: recipient.clone(),
        authority_ref: contract.authority_ref.clone(),
        evidence_ref: source_pin.render.record_digest.clone(),
        update: UpdateKind::Blocked {
            blocker_id: "actual-failure-5ms".into(),
            state: BlockState::Raised,
        },
    };
    let policy_digest = content_digest(&"independent-fixture-policy").expect("valid owned fixture");
    let observe = AuthorizedAttentionCommand {
        command: AttentionCommand {
            scope: definition.scope.clone(),
            config_digest: content_digest(&definition.config).expect("valid owned fixture"),
            command_id: "observe".into(),
            expected_prior: None,
            logical_tick: Some(1),
            operation: AttentionOperation::Observe {
                update: Box::new(update.clone()),
            },
        },
        binding: AttentionRequestBinding {
            actor: principal.clone(),
            credential_ref: "fixture-credential".into(),
            policy_digest: policy_digest.clone(),
            context: source_pin.clone(),
        },
    };
    authority.admit(&observe);
    let observed = store
        .apply_attention_authorized(observe.clone(), proof.clone())
        .expect("valid owned fixture");
    let mut prior = observed.receipt.cursor;
    let mut original_updates = vec![update.clone()];
    for index in 2..=source_count {
        let mut next = observe.clone();
        next.command.command_id = format!("observe-{index}");
        next.command.expected_prior = Some(prior);
        let AttentionOperation::Observe { update } = &mut next.command.operation else {
            unreachable!()
        };
        update.source.sequence = index.to_string();
        update.update = UpdateKind::Blocked {
            blocker_id: format!("additional-blocker-{index}"),
            state: BlockState::Raised,
        };
        original_updates.push((**update).clone());
        authority.admit(&next);
        prior = store
            .apply_attention_authorized(next, proof.clone())
            .expect("valid owned fixture")
            .receipt
            .cursor;
    }
    let take = AuthorizedAttentionCommand {
        command: AttentionCommand {
            scope: definition.scope.clone(),
            config_digest: content_digest(&definition.config).expect("valid owned fixture"),
            command_id: "take".into(),
            expected_prior: Some(prior),
            logical_tick: Some(2),
            operation: AttentionOperation::TakeDelivery {
                recipient,
                intent: OutboxIntent {
                    idempotency_key: "wake".into(),
                    payload: json!({"kind":"owned-fixture"}),
                    not_before: 0,
                },
            },
        },
        binding: AttentionRequestBinding {
            actor: principal,
            credential_ref: "fixture-credential".into(),
            policy_digest,
            context: request_pin.clone(),
        },
    };
    authority.admit(&take);
    let receipt = store
        .apply_attention_authorized(take, proof.clone())
        .expect("valid owned fixture");
    let AttentionOutcome::Delivery {
        decision: admiralty_core::governor::DeliveryDecision::Wake { delivery },
    } = &receipt.receipt.outcome
    else {
        panic!("actual wake");
    };
    let expected = SafeTurnExpectation {
        scope: definition.scope,
        command_id: "take".into(),
        receipt_digest: receipt.receipt_digest,
        batch_digest: content_digest(delivery).expect("valid owned fixture"),
        request_context: request_pin,
        baseline: ExpectedBaseline::Absent,
        sources: delivery
            .refs
            .iter()
            .zip(&original_updates)
            .map(|(reference, update)| SafeTurnSourcePin {
                order: reference.order.clone(),
                update_digest: content_digest(update).expect("valid owned fixture"),
                context: source_pin.clone(),
                contract_digest: content_digest(&contract).expect("valid owned fixture"),
            })
            .collect(),
    };
    Fixture {
        store,
        authority,
        proof,
        expected,
        source_render,
        request_render,
        update,
        _dir: dir,
    }
}
fn compile(f: &Fixture) -> Result<CompiledSafeTurnContext, SafeTurnContextError> {
    compile_safe_turn_context(&f.store, &f.expected, f.proof.clone(), 65536)
}

#[test]
fn actual_journal_sources_and_complete_a_b_capsules_survive_without_grants_or_ack() {
    let f = fixture(false, false);
    let before = f.store.snapshot().expect("valid owned fixture").cursor;
    let output = compile(&f).expect("valid owned fixture");
    let document: serde_json::Value =
        serde_json::from_str(output.text()).expect("valid owned fixture");
    assert_eq!(
        document["evidence"]["sources"][0]["render"],
        serde_json::to_value(&f.source_render).expect("valid owned fixture")
    );
    assert_eq!(
        document["evidence"]["request_render"],
        serde_json::to_value(&f.request_render).expect("valid owned fixture")
    );
    assert_eq!(
        document["evidence"]["sources"][0]["observe_request"]["command"]["operation"]["update"],
        serde_json::to_value(&f.update).expect("valid owned fixture")
    );
    assert!(
        output
            .text()
            .contains("Do NOT publish; preserve 5 ms and 20 GiB")
    );
    assert!(output.text().contains("Usage not measured"));
    assert!(
        output
            .text()
            .contains("B request-context uncertainty must also survive")
    );
    for field in ["delivered", "application_acknowledged", "authority_granted"] {
        assert_eq!(document[field], false);
    }
    assert_eq!(
        f.store.snapshot().expect("valid owned fixture").cursor,
        before
    );
    assert!(
        f.store
            .context_baseline(f.expected.request_context.scope.clone())
            .expect("valid owned fixture")
            .is_none()
    );
    assert_eq!(output.estimated_tokens(), output.text().len().div_ceil(4));
    assert_eq!(
        output.rendered_digest(),
        format!("sha256:{:x}", Sha256::digest(output.text().as_bytes()))
    );
    assert_eq!(
        output.rendered_digest(),
        compile(&f).expect("valid owned fixture").rendered_digest()
    );
    assert_eq!(output.batch_digest(), f.expected.batch_digest);
    assert_eq!(output.receipt_digest(), f.expected.receipt_digest);
}

#[test]
fn recording_only_default_read_policy_opaque_proof_and_current_revocation_deny() {
    let f = fixture(false, true);
    assert!(matches!(
        compile(&f),
        Err(SafeTurnContextError::Store(StoreError::Attention(
            AttentionError::AccessDenied
        )))
    ));
    let f = fixture(false, false);
    assert!(matches!(
        compile_safe_turn_context(&f.store, &f.expected, Arc::new("fixture-credential"), 65536),
        Err(SafeTurnContextError::Store(StoreError::Attention(
            AttentionError::AccessDenied
        )))
    ));
    f.authority.allow_sources.store(false, Ordering::SeqCst);
    assert!(matches!(
        compile(&f),
        Err(SafeTurnContextError::Store(StoreError::Attention(
            AttentionError::AccessDenied
        )))
    ));
    f.authority.allow_sources.store(true, Ordering::SeqCst);
    f.authority.revoked.store(true, Ordering::SeqCst);
    assert!(compile(&f).is_err());
}

#[test]
fn final_read_reauthorization_denial_keeps_original_receipt_and_wake_retryable() {
    let f = fixture(false, false);
    f.authority.deny_final.store(true, Ordering::SeqCst);
    assert!(matches!(
        compile(&f),
        Err(SafeTurnContextError::Store(StoreError::Attention(
            AttentionError::AccessDenied
        )))
    ));
    assert_eq!(f.authority.read_calls.load(Ordering::SeqCst), 4);
    f.authority.deny_final.store(false, Ordering::SeqCst);
    let first = compile(&f).expect("valid owned fixture");
    assert_eq!(
        first.text(),
        compile(&f).expect("valid owned fixture").text()
    );
}

#[test]
fn exact_batch_mapping_scope_epoch_receipt_and_source_pins_are_required() {
    for change in 0..10 {
        let mut f = fixture(false, false);
        match change {
            0 => f.expected.scope.root_id = RootId::new("foreign").expect("valid owned fixture"),
            1 => f.expected.scope.epoch = "other-epoch".into(),
            2 => {
                f.expected.receipt_digest =
                    content_digest(&"wrong receipt").expect("valid owned fixture")
            }
            3 => {
                f.expected.batch_digest =
                    content_digest(&"wrong batch").expect("valid owned fixture")
            }
            4 => {
                f.expected.sources[0].context.facts_digest =
                    content_digest(&"changed facts").expect("valid owned fixture")
            }
            5 => {
                f.expected.sources[0].context.render.record_digest =
                    content_digest(&"changed bytes").expect("valid owned fixture")
            }
            6 => f.expected.sources[0].order = "2".into(),
            7 => {
                f.expected.sources[0].contract_digest =
                    content_digest(&"changed contract").expect("valid owned fixture")
            }
            8 => f.expected.request_context = f.expected.sources[0].context.clone(),
            _ => f.expected.command_id = "missing-original-wake".into(),
        }
        assert!(compile(&f).is_err(), "changed binding {change}");
    }
}

#[test]
fn full_current_contract_mismatch_is_not_replaced_by_old_render_or_matching_revision() {
    let f = fixture(true, false);
    assert!(matches!(
        compile(&f),
        Err(SafeTurnContextError::Store(StoreError::Attention(
            AttentionError::ContextBinding
        )))
    ));
}

#[test]
fn missing_actual_task_is_not_inferred_from_a_valid_protected_source_capsule() {
    let f = fixture_options(false, false, 1, false);
    assert!(matches!(
        compile(&f),
        Err(SafeTurnContextError::Store(StoreError::Attention(
            AttentionError::ContextBinding
        )))
    ));
}

#[test]
fn unknown_or_changed_baseline_refuses_but_exact_baseline_preserves_full_sources() {
    let mut f = fixture(false, false);
    f.expected.baseline = ExpectedBaseline::Unknown;
    assert!(matches!(
        compile(&f),
        Err(SafeTurnContextError::BaselineUnknown)
    ));
    f.expected.baseline = ExpectedBaseline::Absent;
    let baseline = f
        .store
        .advance_context_baseline(AdvanceContextBaseline {
            scope: f.expected.request_context.scope.clone(),
            command_id: "owned-app-ack".into(),
            expected_prior: None,
            render: f.expected.request_context.render.clone(),
            application_ack_ref: "fixture:observed-app-ack".into(),
            delivery_ref: None,
        })
        .expect("valid owned fixture")
        .baseline;
    assert!(matches!(
        compile(&f),
        Err(SafeTurnContextError::BindingMismatch)
    ));
    f.expected.baseline = ExpectedBaseline::Exact { baseline };
    let output = compile(&f).expect("valid owned fixture");
    assert!(output.text().contains("complete_protected_sources"));
    assert!(output.text().contains("A source-only uncertainty"));
}

#[test]
fn byte_and_count_bounds_refuse_without_truncation_or_effects() {
    let mut f = fixture(false, false);
    assert!(matches!(
        compile_safe_turn_context(&f.store, &f.expected, f.proof.clone(), 100),
        Err(SafeTurnContextError::BudgetExceeded)
    ));
    assert!(compile_safe_turn_context(&f.store, &f.expected, f.proof.clone(), 65537).is_err());
    f.expected.sources.clear();
    assert!(matches!(
        compile(&f),
        Err(SafeTurnContextError::InvalidExpectation)
    ));
    assert!(
        f.store
            .context_baseline(f.expected.request_context.scope.clone())
            .expect("valid owned fixture")
            .is_none()
    );
}

#[test]
fn every_actual_batch_source_is_preserved_and_cannot_be_swapped_or_omitted() {
    let mut f = fixture_with_source_count(false, false, 2);
    let output = compile(&f).expect("valid owned fixture");
    let document: serde_json::Value =
        serde_json::from_str(output.text()).expect("valid owned fixture");
    let sources = document["evidence"]["sources"]
        .as_array()
        .expect("valid owned fixture");
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0]["reference"]["order"], "1");
    assert_eq!(sources[1]["reference"]["order"], "2");
    assert!(output.text().contains("actual-failure-5ms"));
    assert!(output.text().contains("additional-blocker-2"));
    f.expected.sources.swap(0, 1);
    assert!(matches!(
        compile(&f),
        Err(SafeTurnContextError::BindingMismatch)
    ));
    f.expected.sources.swap(0, 1);
    f.expected.sources.pop();
    assert!(matches!(
        compile(&f),
        Err(SafeTurnContextError::BindingMismatch)
    ));
    while f.expected.sources.len() < 17 {
        let pin = &f.expected.sources[0];
        f.expected.sources.push(SafeTurnSourcePin {
            order: pin.order.clone(),
            update_digest: pin.update_digest.clone(),
            context: pin.context.clone(),
            contract_digest: pin.contract_digest.clone(),
        });
    }
    assert!(matches!(
        compile(&f),
        Err(SafeTurnContextError::InvalidExpectation)
    ));
}
