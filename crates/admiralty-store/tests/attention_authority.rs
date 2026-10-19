use admiralty_core::{
    AcceptanceCriterion, CheckSpec, PrincipalId, ProjectId, RootId, SourceIdentity, TaskContract,
    TaskId, content_digest,
    coordination::context::{
        BudgetObservation, CheckObservation, FactObservation, ProjectExceptionFacts,
        ProtectedTaskFacts, RecipientRole, compile_project_exceptions,
    },
    governor::{AttentionUpdate, BlockState, GovernorConfig, Recipient, Role, Scope, UpdateKind},
};
use admiralty_store::{
    Store, StoreAuthorities, StoreError,
    attention::*,
    context_renders::{ContextNamespace, ContextScope, RecordContextRender, RenderKey},
    outbox::OutboxIntent,
};
use rusqlite::Connection;
use std::{
    any::Any,
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Clone)]
struct PrivateActorProof {
    actor: PrincipalId,
    incarnation: Arc<()>,
}
struct Gate {
    entered: mpsc::SyncSender<()>,
    resume: mpsc::Receiver<()>,
}
struct ExactAuthority {
    definition: AttentionDefinition,
    policy_digest: Mutex<String>,
    incarnation: Arc<()>,
    allowed: Mutex<BTreeSet<String>>,
    revoked: AtomicBool,
    deny_final: AtomicBool,
    calls: AtomicUsize,
    gate: Mutex<Option<Gate>>,
}
impl ExactAuthority {
    fn admit(&self, request: &AuthorizedAttentionCommand) {
        self.allowed.lock().expect("policy state").insert(
            authorized_request_digest(
                &content_digest(&self.definition).expect("definition identity"),
                request,
            )
            .expect("bounded actual request identity"),
        );
    }
    fn pause_next(&self) -> (mpsc::Receiver<()>, mpsc::SyncSender<()>) {
        let (entered, receive) = mpsc::sync_channel(1);
        let (resume, wait) = mpsc::sync_channel(1);
        *self.gate.lock().expect("gate") = Some(Gate {
            entered,
            resume: wait,
        });
        (receive, resume)
    }
}
impl AttentionAuthority for ExactAuthority {
    fn authorize_definition(&self, b: &AttentionDefinitionBinding<'_>) -> bool {
        !self.revoked.load(Ordering::SeqCst)
            && b.definition == &self.definition
            && content_digest(&self.definition).is_ok_and(|d| d == b.definition_digest)
            && content_digest(&self.definition.config).is_ok_and(|d| d == b.config_digest)
    }
    fn authorize_command(
        &self,
        b: &AttentionCommandBinding<'_>,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().expect("gate").take();
        if let Some(gate) = gate {
            // Deliberately bounded test-only barrier: the request is already on
            // the real writer when credential/policy changes are introduced.
            if gate.entered.send(()).is_err()
                || gate.resume.recv_timeout(Duration::from_secs(5)).is_err()
            {
                return false;
            }
        }
        let Some(actor) = proof.downcast_ref::<PrivateActorProof>() else {
            return false;
        };
        !(self.revoked.load(Ordering::SeqCst)
            || self.deny_final.load(Ordering::SeqCst) && b.outcome.is_some())
            && Arc::ptr_eq(&actor.incarnation, &self.incarnation)
            && actor.actor == b.request.binding.actor
            && b.request.binding.credential_ref == "authenticated-credential"
            && b.request.binding.policy_digest
                == *self.policy_digest.lock().expect("current policy")
            && b.definition == &self.definition
            && b.stored_context.scope == b.request.binding.context.scope
            && content_digest(b.stored_context)
                .is_ok_and(|d| d == b.request.binding.context.render.record_digest)
            && authorized_request_digest(b.definition_digest, b.request)
                .is_ok_and(|d| d == b.request_digest)
            && match &b.request.command.operation {
                AttentionOperation::Observe { update } => {
                    update.source.producer_id == "observed-source"
                        && update.source.incarnation == "observed-boot"
                }
                AttentionOperation::TakeDelivery { .. } => true,
            }
            && self
                .allowed
                .lock()
                .expect("allowed exact requests")
                .contains(b.request_digest)
    }
}

struct Fixture {
    store: Store,
    path: PathBuf,
    authority: Arc<ExactAuthority>,
    proof: AttentionProof,
    request: AuthorizedAttentionCommand,
    definition: AttentionDefinition,
    _dir: tempfile::TempDir,
}
fn fixture() -> Fixture {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .expect("private real Store fixture");
    let path = dir.path().join("attention.sqlite");
    let project = ProjectId::new("project").expect("project");
    let root = RootId::new("root").expect("root");
    let principal = PrincipalId::new("commodore").expect("principal");
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
    let policy_digest = content_digest(&(
        "private-fixture-policy",
        &definition,
        "authenticated-credential",
    ))
    .expect("actual policy config identity");
    let incarnation = Arc::new(());
    let authority = Arc::new(ExactAuthority {
        definition: definition.clone(),
        policy_digest: Mutex::new(policy_digest.clone()),
        incarnation: incarnation.clone(),
        allowed: Mutex::new(BTreeSet::new()),
        revoked: AtomicBool::new(false),
        deny_final: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        gate: Mutex::new(None),
    });
    let store = Store::open_with_startup_authorities(
        &path,
        StoreAuthorities {
            attention: Some(authority.clone()),
            ..StoreAuthorities::default()
        },
    )
    .expect("real sole writer with independently installed authority");
    let contract = TaskContract {
        schema_version: 1,
        project_id: project.clone(),
        root_id: root.clone(),
        task_id: TaskId::new("task").expect("task"),
        revision: 1,
        title: "Exact retained task".into(),
        objective: "Inspect blocker without publishing".into(),
        non_goals: vec!["Do NOT publish".into()],
        owned_paths: vec!["src".into()],
        forbidden_paths: vec!["private".into()],
        read_first: vec![],
        dependencies: vec![],
        criteria: vec![AcceptanceCriterion {
            id: "bounded".into(),
            text: "Keep exact scope".into(),
            check_ids: vec!["unit".into()],
        }],
        checks: vec![CheckSpec {
            id: "unit".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 30,
        }],
        authority_ref: "grant:task".into(),
        max_attempts: 1,
        simulated: true,
    };
    let facts = ProjectExceptionFacts {
        project_id: project.clone(),
        revision: 1,
        authority_ref: "grant:project".into(),
        source_ref: "source:project".into(),
        exceptions: vec![ProtectedTaskFacts {
            contract: contract.clone(),
            immutable_refs: vec![],
            candidate: FactObservation::Unknown {
                reason: "No candidate".into(),
            },
            check: CheckObservation::NotRun {
                reason: "No checks".into(),
            },
            evidence: vec![],
            uncertainty: vec!["Outcome unknown".into()],
            budget: BudgetObservation::Unknown {
                reason: "Not measured".into(),
            },
            next_action: "Inspect blocker".into(),
        }],
        uncertainty: vec![],
        next_action: "Inspect blocker".into(),
    };
    let capsule =
        compile_project_exceptions(&facts, "epoch", 8192).expect("actual deterministic compiler");
    let context_scope = ContextScope {
        recipient: principal.clone(),
        role: RecipientRole::Commodore,
        namespace: ContextNamespace::Project {
            project_id: project.clone(),
            root_id: root,
        },
    };
    let recorded = store
        .record_context_render(RecordContextRender {
            scope: context_scope.clone(),
            key: RenderKey {
                render_id: "actual-render".into(),
                version: 1,
            },
            expected_epoch: "epoch".into(),
            expected_facts_digest: capsule.facts_digest.clone(),
            capsule: capsule.clone(),
        })
        .expect("actual stored capsule");
    let registration = store
        .register_attention(definition.clone())
        .expect("exact startup definition");
    let request = AuthorizedAttentionCommand {
        command: AttentionCommand {
            scope: definition.scope.clone(),
            config_digest: registration.config_digest,
            command_id: "observe".into(),
            expected_prior: None,
            logical_tick: Some(1),
            operation: AttentionOperation::Observe {
                update: Box::new(AttentionUpdate {
                    source: SourceIdentity {
                        producer_id: "observed-source".into(),
                        incarnation: "observed-boot".into(),
                        sequence: "1".into(),
                    },
                    scope: Scope {
                        project_id: project,
                        task_id: contract.task_id,
                        revision: 1,
                    },
                    recipient,
                    authority_ref: contract.authority_ref,
                    evidence_ref: recorded.render.record_digest.clone(),
                    update: UpdateKind::Blocked {
                        blocker_id: "blocker".into(),
                        state: BlockState::Raised,
                    },
                }),
            },
        },
        binding: AttentionRequestBinding {
            actor: principal.clone(),
            credential_ref: "authenticated-credential".into(),
            policy_digest,
            context: AttentionContextPin {
                scope: context_scope,
                render: recorded.render,
                facts_digest: capsule.facts_digest,
            },
        },
    };
    authority.admit(&request);
    Fixture {
        store,
        path,
        authority,
        proof: Arc::new(PrivateActorProof {
            actor: principal,
            incarnation,
        }),
        request,
        definition,
        _dir: dir,
    }
}
fn denied(result: Result<AttentionResult, StoreError>) -> bool {
    matches!(
        result,
        Err(StoreError::Attention(AttentionError::AccessDenied))
    )
}
fn counts(path: &std::path::Path) -> (i64, i64) {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("read owned fixture");
    connection.query_row("SELECT (SELECT count(*) FROM attention_commands),(SELECT count(*) FROM outbox_intents)", [], |row| Ok((row.get(0)?, row.get(1)?))).expect("actual durable counts")
}
fn take(f: &Fixture, prior: AttentionCursor) -> AuthorizedAttentionCommand {
    let mut request = f.request.clone();
    request.command.command_id = "take".into();
    request.command.expected_prior = Some(prior);
    request.command.logical_tick = Some(2);
    let recipient = f.definition.scope.recipients[0].clone();
    request.command.operation = AttentionOperation::TakeDelivery {
        recipient,
        intent: OutboxIntent {
            idempotency_key: "owned-attention-wake".into(),
            payload: serde_json::json!({"request_context":request.binding.context,"delivered":false}),
            not_before: 0,
        },
    };
    f.authority.admit(&request);
    request
}

#[test]
fn absent_startup_authority_and_unbound_store_mutations_deny_without_effects() {
    let f = fixture();
    assert!(denied(f.store.apply_attention(f.request.command.clone())));
    assert_eq!(counts(&f.path), (0, 0));
    let other_path = f._dir.path().join("default-deny.sqlite");
    let absent = Store::open(&other_path).expect("default denied Store");
    assert!(matches!(
        absent.register_attention(f.definition.clone()),
        Err(StoreError::Attention(AttentionError::AccessDenied))
    ));
    assert!(denied(
        absent.apply_attention_authorized(f.request.clone(), f.proof.clone())
    ));
    assert_eq!(counts(&other_path), (0, 0));
}

#[test]
fn opaque_proof_and_current_policy_are_required_despite_matching_json_claims() {
    let f = fixture();
    assert!(denied(f.store.apply_attention_authorized(
        f.request.clone(),
        Arc::new("authenticated-credential")
    )));
    let cloned_labels = Arc::new(PrivateActorProof {
        actor: f.request.binding.actor.clone(),
        incarnation: Arc::new(()),
    });
    assert!(denied(
        f.store
            .apply_attention_authorized(f.request.clone(), cloned_labels)
    ));
    f.authority.revoked.store(true, Ordering::SeqCst);
    assert!(denied(
        f.store
            .apply_attention_authorized(f.request.clone(), f.proof.clone())
    ));
    assert_eq!(counts(&f.path), (0, 0));
}

#[test]
fn revoke_and_policy_drift_after_enqueue_are_checked_on_actual_writer_turn() {
    for drift in [false, true] {
        let f = fixture();
        let (entered, resume) = f.authority.pause_next();
        let store = f.store.clone();
        let request = f.request.clone();
        let proof = f.proof.clone();
        let worker = std::thread::spawn(move || store.apply_attention_authorized(request, proof));
        entered
            .recv_timeout(Duration::from_secs(5))
            .expect("request reached actual writer callback after enqueue");
        if drift {
            *f.authority.policy_digest.lock().expect("current policy") =
                content_digest(&"new independent policy revision").expect("policy identity");
        } else {
            f.authority.revoked.store(true, Ordering::SeqCst);
        }
        resume.send(()).expect("release bounded callback");
        assert!(denied(worker.join().expect("writer request returned")));
        assert_eq!(counts(&f.path), (0, 0));
    }
}

#[test]
fn final_writer_check_rolls_back_staged_wake_receipt_and_budget() {
    let f = fixture();
    let observed = f
        .store
        .apply_attention_authorized(f.request.clone(), f.proof.clone())
        .expect("authorized observe");
    let request = take(&f, observed.receipt.cursor.clone());
    f.authority.deny_final.store(true, Ordering::SeqCst);
    assert!(denied(
        f.store
            .apply_attention_authorized(request.clone(), f.proof.clone())
    ));
    assert_eq!(counts(&f.path), (1, 0));
    assert_eq!(
        f.store
            .attention_status(f.definition.scope.clone())
            .expect("status")
            .expect("journal")
            .cursor,
        Some(observed.receipt.cursor)
    );
    f.authority.deny_final.store(false, Ordering::SeqCst);
    let accepted = f
        .store
        .apply_attention_authorized(request, f.proof.clone())
        .expect("rollback did not spend budget or reserve key");
    assert!(accepted.receipt.recorded_wake);
    assert!(
        !accepted.receipt.delivered
            && !accepted.receipt.application_acknowledged
            && !accepted.receipt.authority_granted
    );
    assert_eq!(counts(&f.path), (2, 1));
}

#[test]
fn immutable_retry_after_later_command_preserves_unknown_outbox_and_head() {
    let f = fixture();
    let observed = f
        .store
        .apply_attention_authorized(f.request.clone(), f.proof.clone())
        .expect("observe");
    let request = take(&f, observed.receipt.cursor);
    let original = f
        .store
        .apply_attention_authorized(request.clone(), f.proof.clone())
        .expect("wake");
    let mut later = f.request.clone();
    later.command.command_id = "later".into();
    later.command.logical_tick = Some(3);
    later.command.expected_prior = Some(original.receipt.cursor.clone());
    if let AttentionOperation::Observe { update } = &mut later.command.operation {
        update.source.sequence = "2".into();
    }
    f.authority.admit(&later);
    let latest = f
        .store
        .apply_attention_authorized(later, f.proof.clone())
        .expect("later observation");
    let claim = f
        .store
        .claim_outbox_key(
            Some(0),
            "dispatcher".into(),
            1,
            "owned-attention-wake".into(),
        )
        .expect("claim")
        .expect("one wake");
    assert!(
        f.store
            .claim_outbox_key(
                Some(2),
                "dispatcher".into(),
                1,
                "owned-attention-wake".into()
            )
            .expect("expiry becomes unknown")
            .is_none()
    );
    let before = f.authority.calls.load(Ordering::SeqCst);
    let replay = f
        .store
        .apply_attention_authorized(request.clone(), f.proof.clone())
        .expect("lost reply exact replay");
    assert!(replay.replayed);
    assert_eq!(replay.receipt, original.receipt);
    assert_eq!(replay.receipt_digest, original.receipt_digest);
    assert!(f.authority.calls.load(Ordering::SeqCst) >= before + 2);
    assert_eq!(
        f.store
            .attention_status(f.definition.scope.clone())
            .expect("status")
            .expect("journal")
            .cursor,
        Some(latest.receipt.cursor)
    );
    assert_eq!(counts(&f.path), (3, 1));
    assert!(
        f.store
            .claim_outbox_key(
                Some(3),
                "dispatcher".into(),
                1,
                "owned-attention-wake".into()
            )
            .expect("unknown retained")
            .is_none()
    );
    f.authority.revoked.store(true, Ordering::SeqCst);
    assert!(denied(
        f.store.apply_attention_authorized(request, f.proof.clone())
    ));
    assert_eq!(counts(&f.path), (3, 1));
    assert!(!claim.token.owner.is_empty());
}

#[test]
fn changed_request_collides_and_wrong_context_scope_source_or_epoch_never_effects() {
    let f = fixture();
    let observed = f
        .store
        .apply_attention_authorized(f.request.clone(), f.proof.clone())
        .expect("observe");
    let mut conflicting = f.request.clone();
    conflicting.command.logical_tick = Some(2);
    f.authority.admit(&conflicting);
    assert!(matches!(
        f.store
            .apply_attention_authorized(conflicting, f.proof.clone()),
        Err(StoreError::Attention(AttentionError::Collision))
    ));
    for change in 0..6 {
        let mut request = take(&f, observed.receipt.cursor.clone());
        request.command.command_id = format!("bad-{change}");
        match change {
            0 => {
                request.binding.context.facts_digest =
                    content_digest(&"wrong actual facts").expect("digest")
            }
            1 => request.command.scope.epoch = "other-epoch".into(),
            2 => {
                request.binding.context.scope.recipient =
                    PrincipalId::new("foreign").expect("principal")
            }
            3 => request.command.scope.root_id = RootId::new("foreign").expect("root"),
            4 => {
                request.binding.context.render.record_digest =
                    content_digest(&"wrong record").expect("digest")
            }
            _ => request.binding.context.scope.role = RecipientRole::Admiral,
        }
        assert!(
            f.store
                .apply_attention_authorized(request, f.proof.clone())
                .is_err()
        );
    }
    let mut bad_source = f.request.clone();
    bad_source.command.command_id = "bad-source".into();
    bad_source.command.expected_prior = Some(observed.receipt.cursor.clone());
    if let AttentionOperation::Observe { update } = &mut bad_source.command.operation {
        update.source.producer_id = "untrusted-source".into();
    }
    f.authority.admit(&bad_source);
    assert!(denied(
        f.store
            .apply_attention_authorized(bad_source, f.proof.clone())
    ));
    let mut bad_task = f.request.clone();
    bad_task.command.command_id = "bad-task".into();
    bad_task.command.expected_prior = Some(observed.receipt.cursor);
    if let AttentionOperation::Observe { update } = &mut bad_task.command.operation {
        update.scope.revision = 2;
    }
    f.authority.admit(&bad_task);
    assert!(matches!(
        f.store
            .apply_attention_authorized(bad_task, f.proof.clone()),
        Err(StoreError::Attention(AttentionError::ContextBinding))
    ));
    assert_eq!(counts(&f.path), (1, 0));
}

#[test]
fn late_sql_abort_preserves_original_cursor_and_outbox_key_for_retry() {
    let f = fixture();
    let observed = f
        .store
        .apply_attention_authorized(f.request.clone(), f.proof.clone())
        .expect("observe");
    let request = take(&f, observed.receipt.cursor.clone());
    // Private fixture-only fault injection while the sole writer is idle; the
    // production implementation still has exactly one writer connection.
    let fault = Connection::open(&f.path).expect("owned fault fixture");
    fault.execute_batch("CREATE TRIGGER owned_attention_abort BEFORE INSERT ON attention_commands WHEN NEW.command_id='take' BEGIN SELECT RAISE(ABORT,'owned late fault'); END;").expect("fault trigger");
    assert!(
        f.store
            .apply_attention_authorized(request.clone(), f.proof.clone())
            .is_err()
    );
    assert_eq!(counts(&f.path), (1, 0));
    fault
        .execute_batch("DROP TRIGGER owned_attention_abort")
        .expect("remove owned fault");
    let result = f
        .store
        .apply_attention_authorized(request, f.proof.clone())
        .expect("atomic retry after fault");
    assert!(result.receipt.recorded_wake);
    assert_eq!(counts(&f.path), (2, 1));
}

#[test]
fn replay_reopen_rebuild_and_backup_keep_history_without_current_grants() {
    let f = fixture();
    let original = f
        .store
        .apply_attention_authorized(f.request.clone(), f.proof.clone())
        .expect("observe");
    f.authority.revoked.store(true, Ordering::SeqCst);
    f.store
        .rebuild()
        .expect("historical integrity rebuild does not require a current grant");
    let backup = f._dir.path().join("retained-backup.sqlite");
    f.store.backup(&backup).expect("bounded consistent backup");
    let reopened =
        Store::open(&backup).expect("valid historical records reopen with no authority installed");
    assert!(denied(
        reopened.apply_attention_authorized(f.request.clone(), f.proof.clone())
    ));
    assert_eq!(counts(&backup), (1, 0));
    drop(reopened);
    let reopened = Store::open_with_startup_authorities(
        &backup,
        StoreAuthorities {
            attention: Some(f.authority.clone()),
            ..StoreAuthorities::default()
        },
    )
    .expect("revoked authority still allows historical open");
    assert!(denied(
        reopened.apply_attention_authorized(f.request.clone(), f.proof.clone())
    ));
    f.authority.revoked.store(false, Ordering::SeqCst);
    let replay = reopened
        .apply_attention_authorized(f.request.clone(), f.proof.clone())
        .expect("current proof after reopen");
    assert!(replay.replayed);
    assert_eq!(replay.receipt, original.receipt);
}

#[test]
fn legacy_rows_keep_original_raw_bytes_and_receipt_formula_on_reopen() {
    let f = fixture();
    let legacy_path = f._dir.path().join("legacy.sqlite");
    let initialized = Store::open(&legacy_path).expect("schema only");
    let actual_context = f
        .store
        .context_render(
            f.request.binding.context.scope.clone(),
            f.request.binding.context.render.key.clone(),
        )
        .expect("real original render")
        .expect("stored render");
    let copied_reference = initialized
        .record_context_render(RecordContextRender {
            scope: actual_context.scope,
            key: actual_context.key,
            expected_epoch: actual_context.capsule.context_epoch.clone(),
            expected_facts_digest: actual_context.capsule.facts_digest.clone(),
            capsule: actual_context.capsule,
        })
        .expect("actual same immutable context in legacy fixture");
    assert_eq!(copied_reference.render, f.request.binding.context.render);
    drop(initialized);
    let mut raw = Connection::open(&legacy_path).expect("caller-owned legacy connection");
    register(&mut raw, f.definition.clone()).expect("legacy component registration");
    let result = apply(&mut raw, f.request.command.clone()).expect("legacy component observation");
    let original: (String, String) = raw
        .query_row(
            "SELECT command_json,receipt_json FROM attention_commands",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("legacy bytes");
    assert_eq!(
        original.0,
        serde_json::to_string(&f.request.command).expect("original v1 bytes")
    );
    drop(raw);
    let store = Store::open_with_startup_authorities(
        &legacy_path,
        StoreAuthorities {
            attention: Some(f.authority.clone()),
            ..StoreAuthorities::default()
        },
    )
    .expect("legacy history compatible");
    assert!(matches!(
        store.apply_attention_authorized(f.request.clone(), f.proof.clone()),
        Err(StoreError::Attention(AttentionError::Collision))
    ));
    store.rebuild().expect("legacy rebuild");
    assert_eq!(
        store
            .attention_status(f.definition.scope.clone())
            .expect("status")
            .expect("journal")
            .cursor,
        Some(result.receipt.cursor)
    );
    let read = Connection::open(&legacy_path).expect("read actual historical bytes");
    let retained: (String, String) = read
        .query_row(
            "SELECT command_json,receipt_json FROM attention_commands",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("retained bytes");
    assert_eq!(retained, original);
}

#[test]
fn definition_retries_and_status_require_current_startup_policy() {
    let f = fixture();
    assert!(
        f.store
            .register_attention(f.definition.clone())
            .expect("exact current definition retry")
            .replayed
    );
    f.authority.revoked.store(true, Ordering::SeqCst);
    assert!(matches!(
        f.store.register_attention(f.definition.clone()),
        Err(StoreError::Attention(AttentionError::AccessDenied))
    ));
    assert!(matches!(
        f.store.attention_status(f.definition.scope.clone()),
        Err(StoreError::Attention(AttentionError::AccessDenied))
    ));
    assert_eq!(counts(&f.path), (0, 0));
}

#[test]
fn completed_effect_is_not_reenqueued_or_promoted_to_provider_delivery() {
    let f = fixture();
    let observed = f
        .store
        .apply_attention_authorized(f.request.clone(), f.proof.clone())
        .expect("observe");
    let request = take(&f, observed.receipt.cursor);
    let original = f
        .store
        .apply_attention_authorized(request.clone(), f.proof.clone())
        .expect("wake");
    let claim = f
        .store
        .claim_outbox_key(
            Some(0),
            "owned-dispatcher".into(),
            10,
            "owned-attention-wake".into(),
        )
        .expect("claim")
        .expect("actual effect");
    f.store
        .complete_outbox(
            claim.token,
            "owned-deterministic-dispatcher-completion".into(),
            Some(1),
        )
        .expect("record owned dispatcher completion");
    let replay = f
        .store
        .apply_attention_authorized(request, f.proof.clone())
        .expect("immutable retry");
    assert_eq!(replay.receipt, original.receipt);
    assert!(
        !replay.receipt.delivered
            && !replay.receipt.application_acknowledged
            && !replay.receipt.authority_granted
    );
    assert_eq!(counts(&f.path), (2, 1));
    assert!(
        f.store
            .claim_outbox_key(
                Some(2),
                "owned-dispatcher".into(),
                10,
                "owned-attention-wake".into()
            )
            .expect("completed stays completed")
            .is_none()
    );
}

#[test]
fn bounded_binding_and_corrupted_durable_request_fail_closed() {
    let f = fixture();
    let mut huge = f.request.clone();
    huge.binding.credential_ref = "x".repeat(MAX_COMMAND_BYTES + 1);
    assert!(
        f.store
            .apply_attention_authorized(huge, f.proof.clone())
            .is_err()
    );
    assert_eq!(counts(&f.path), (0, 0));
    f.store
        .apply_attention_authorized(f.request.clone(), f.proof.clone())
        .expect("actual bound record");
    let backup = f._dir.path().join("owned-corruption.sqlite");
    f.store.backup(&backup).expect("owned consistent copy");
    let corrupt = Connection::open(&backup).expect("private corruption fixture");
    corrupt.execute_batch("DROP TRIGGER attention_command_immutable_update; UPDATE attention_commands SET command_json=json_set(command_json,'$.request.binding.credential_ref','forged-credential');").expect("controlled corruption");
    drop(corrupt);
    assert!(Store::open(&backup).is_err());
    assert_eq!(counts(&backup), (1, 0)); // Failed open did not repair/delete history.
}
