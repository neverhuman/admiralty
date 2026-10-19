//! Actual Store writer registration is required to run this integration target.
//! Raw SQLite connections below are adversarial fixture injection/inspection,
//! never the attention implementation's writer path or a delivery executor.
use admiralty_core::{
    PrincipalId, ProjectId, RootId, SourceIdentity, TaskId, content_digest,
    governor::{
        self, AttentionUpdate, BlockState, CandidateState, DeliveryDecision, GovernorConfig,
        QuestionFacts, QuestionState, Recipient, Role, UpdateKind, WakeReason,
    },
};
use admiralty_store::{Store as ActualStore, StoreAuthorities, StoreError, attention::*, outbox};
use rusqlite::Connection;
use serde_json::json;
use std::path::Path;

// Explicit private test issuer. These tests still exercise the actual sole
// Store writer, CAS, journal and outbox. No production permissive policy exists.
use admiralty_core::{
    AcceptanceCriterion, CheckSpec, TaskContract,
    coordination::context::{
        BudgetObservation, CheckObservation, FactObservation, ProjectExceptionFacts,
        ProtectedTaskFacts, RecipientRole, compile_project_exceptions,
    },
};
use admiralty_store::context_renders::{
    ContextNamespace, ContextScope, RecordContextRender, RenderKey,
};
use std::{
    any::Any,
    collections::BTreeSet,
    ops::Deref,
    sync::{Arc, Mutex},
};
#[derive(Clone)]
struct FixtureProof {
    principal: PrincipalId,
    nonce: Arc<()>,
}
struct FixtureAuthority {
    nonce: Arc<()>,
    definitions: Mutex<std::collections::BTreeMap<String, AttentionDefinition>>,
    requests: Mutex<BTreeSet<String>>,
    policy_digest: String,
}
impl AttentionAuthority for FixtureAuthority {
    fn authorize_definition(&self, b: &AttentionDefinitionBinding<'_>) -> bool {
        content_digest(b.definition).is_ok_and(|d| {
            d == b.definition_digest
                && self
                    .definitions
                    .lock()
                    .expect("explicit definitions")
                    .contains_key(&d)
        }) && content_digest(&b.definition.config).is_ok_and(|d| d == b.config_digest)
    }
    fn authorize_command(
        &self,
        b: &AttentionCommandBinding<'_>,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        let Some(proof) = proof.downcast_ref::<FixtureProof>() else {
            return false;
        };
        Arc::ptr_eq(&proof.nonce, &self.nonce)
            && proof.principal == b.request.binding.actor
            && b.request.binding.credential_ref == "private-fixture-credential"
            && b.request.binding.policy_digest == self.policy_digest
            && b.definition
                .scope
                .recipients
                .iter()
                .any(|r| r.principal_id == proof.principal)
            && self
                .requests
                .lock()
                .expect("exact admitted requests")
                .contains(b.request_digest)
            && authorized_request_digest(b.definition_digest, b.request)
                .is_ok_and(|d| d == b.request_digest)
            && content_digest(b.stored_context)
                .is_ok_and(|d| d == b.request.binding.context.render.record_digest)
    }
}
struct Store {
    inner: ActualStore,
    authority: Arc<FixtureAuthority>,
}
impl Deref for Store {
    type Target = ActualStore;
    fn deref(&self) -> &ActualStore {
        &self.inner
    }
}
impl Store {
    fn open(path: &Path) -> Result<Self, StoreError> {
        let authority = Arc::new(FixtureAuthority {
            nonce: Arc::new(()),
            definitions: Mutex::new(std::collections::BTreeMap::from([(
                content_digest(&definition()).expect("explicit original definition"),
                definition(),
            )])),
            requests: Mutex::new(BTreeSet::new()),
            policy_digest: content_digest(&(
                "private-explicit-journal-test-policy",
                "scoped-current-private-proof",
                "exact-request-digest",
            ))
            .expect("fixture policy identity"),
        });
        let inner = ActualStore::open_with_startup_authorities(
            path,
            StoreAuthorities {
                attention: Some(authority.clone()),
                ..StoreAuthorities::default()
            },
        )?;
        Ok(Self { inner, authority })
    }
    fn register_attention(
        &self,
        definition: AttentionDefinition,
    ) -> Result<AttentionRegistration, StoreError> {
        self.authority
            .definitions
            .lock()
            .expect("explicit test definition admission")
            .insert(content_digest(&definition)?, definition.clone());
        self.inner.register_attention(definition)
    }
    fn pin(
        &self,
        scope: &AttentionScope,
        recipient: &Recipient,
    ) -> Result<AttentionContextPin, StoreError> {
        let role = match recipient.role {
            Role::TaskWorker => RecipientRole::Worker,
            Role::Captain => RecipientRole::Commodore,
            Role::SuperCaptain => RecipientRole::Admiral,
        };
        if role != RecipientRole::Commodore {
            return Err(StoreError::Attention(AttentionError::Invalid));
        }
        let contract = TaskContract {
            schema_version: 1,
            project_id: scope.project_id.clone(),
            root_id: scope.root_id.clone(),
            task_id: TaskId::new("task-a").expect("task"),
            revision: 1,
            title: "Retained journal fixture task".into(),
            objective: "Inspect bounded state".into(),
            non_goals: vec!["Do NOT publish".into()],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec!["private".into()],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "scope".into(),
                text: "Preserve constraints".into(),
                check_ids: vec!["unit".into()],
            }],
            checks: vec![CheckSpec {
                id: "unit".into(),
                program: "cargo".into(),
                args: vec!["test".into()],
                cwd: ".".into(),
                timeout_secs: 30,
            }],
            authority_ref: "grant:caller-must-authenticate".into(),
            max_attempts: 1,
            simulated: true,
        };
        let facts = ProjectExceptionFacts {
            project_id: scope.project_id.clone(),
            revision: 1,
            authority_ref: "fixture:project".into(),
            source_ref: "fixture:source".into(),
            exceptions: vec![ProtectedTaskFacts {
                contract,
                immutable_refs: vec![],
                candidate: FactObservation::Unknown {
                    reason: "Not observed".into(),
                },
                check: CheckObservation::NotRun {
                    reason: "Not executed".into(),
                },
                evidence: vec![],
                uncertainty: vec!["Outcome unknown".into()],
                budget: BudgetObservation::Unknown {
                    reason: "Not measured".into(),
                },
                next_action: "Inspect current state".into(),
            }],
            uncertainty: vec![],
            next_action: "Inspect current state".into(),
        };
        let capsule = compile_project_exceptions(&facts, &scope.epoch, 8192)
            .expect("actual deterministic capsule");
        let context_scope = ContextScope {
            recipient: recipient.principal_id.clone(),
            role,
            namespace: ContextNamespace::Project {
                project_id: scope.project_id.clone(),
                root_id: scope.root_id.clone(),
            },
        };
        let record = self.inner.record_context_render(RecordContextRender {
            scope: context_scope.clone(),
            key: RenderKey {
                render_id: "actual-journal-fixture-capsule".into(),
                version: 1,
            },
            expected_epoch: scope.epoch.clone(),
            expected_facts_digest: capsule.facts_digest.clone(),
            capsule: capsule.clone(),
        })?;
        Ok(AttentionContextPin {
            scope: context_scope,
            render: record.render,
            facts_digest: capsule.facts_digest,
        })
    }
    fn apply_attention(
        &self,
        mut command: AttentionCommand,
    ) -> Result<AttentionResult, StoreError> {
        let recipient = match &command.operation {
            AttentionOperation::Observe { update } => update.recipient.clone(),
            AttentionOperation::TakeDelivery { recipient, .. } => recipient.clone(),
        };
        let pin = self.pin(&command.scope, &recipient)?;
        if let AttentionOperation::Observe { update } = &mut command.operation
            && update.evidence_ref == "fixture:actual-context"
        {
            update.evidence_ref = pin.render.record_digest.clone();
        }
        let request = AuthorizedAttentionCommand {
            command,
            binding: AttentionRequestBinding {
                actor: recipient.principal_id.clone(),
                credential_ref: "private-fixture-credential".into(),
                policy_digest: self.authority.policy_digest.clone(),
                context: pin,
            },
        };
        // Every allowed request is an explicit in-process test invocation,
        // pinned before queuing; JSON labels alone cannot construct this proof.
        let digest = authorized_request_digest(
            &content_digest(&AttentionDefinition {
                scope: request.command.scope.clone(),
                config: self.definition_config(&request.command)?,
            })?,
            &request,
        )?;
        self.authority
            .requests
            .lock()
            .expect("explicit exact request admission")
            .insert(digest);
        self.inner.apply_attention_authorized(
            request,
            Arc::new(FixtureProof {
                principal: recipient.principal_id,
                nonce: self.authority.nonce.clone(),
            }),
        )
    }
    fn definition_config(&self, command: &AttentionCommand) -> Result<GovernorConfig, StoreError> {
        let defs = self
            .authority
            .definitions
            .lock()
            .expect("explicit definitions");
        defs.values()
            .find(|definition| {
                definition.scope == command.scope
                    && content_digest(&definition.config).is_ok_and(|d| d == command.config_digest)
            })
            .map(|definition| definition.config.clone())
            .ok_or(StoreError::Attention(AttentionError::Collision))
    }
}

fn private_dir() -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("adm-attention-")
        .tempdir()
        .expect("tempdir");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).expect("0700");
    }
    dir
}
fn recipient(id: &str) -> Recipient {
    Recipient {
        role: Role::Captain,
        principal_id: PrincipalId::new(id).expect("principal"),
    }
}
fn definition() -> AttentionDefinition {
    let mut config = GovernorConfig::default();
    config.captain.ordinary_wakes = 1;
    config.captain.critical_reserve = 1;
    config.captain.max_delivery_refs = 1;
    AttentionDefinition {
        scope: AttentionScope {
            journal_id: "attention-a".into(),
            project_id: ProjectId::new("project-a").expect("project"),
            root_id: RootId::new("root-a").expect("root"),
            epoch: "epoch-1".into(),
            recipients: vec![recipient("captain-a"), recipient("captain-b")],
        },
        config,
    }
}
fn update(def: &AttentionDefinition, n: u64, who: &str, kind: UpdateKind) -> AttentionUpdate {
    AttentionUpdate {
        source: SourceIdentity {
            producer_id: "producer-a".into(),
            incarnation: "boot-a".into(),
            sequence: n.to_string(),
        },
        scope: governor::Scope {
            project_id: def.scope.project_id.clone(),
            task_id: TaskId::new("task-a").expect("task"),
            revision: 1,
        },
        recipient: recipient(who),
        authority_ref: "grant:caller-must-authenticate".into(),
        evidence_ref: "fixture:actual-context".into(),
        update: kind,
    }
}
fn candidate(def: &AttentionDefinition, n: u64, who: &str) -> AttentionUpdate {
    update(
        def,
        n,
        who,
        UpdateKind::Candidate {
            candidate_ref: format!("candidate:{n}"),
            state: CandidateState::Submitted,
        },
    )
}
fn command(
    store: &Store,
    def: &AttentionDefinition,
    id: &str,
    tick: u64,
    operation: AttentionOperation,
) -> AttentionCommand {
    AttentionCommand {
        scope: def.scope.clone(),
        config_digest: content_digest(&def.config).expect("digest"),
        command_id: id.into(),
        expected_prior: store
            .attention_status(def.scope.clone())
            .expect("status")
            .expect("registered")
            .cursor,
        logical_tick: Some(tick),
        operation,
    }
}
fn observe(
    store: &Store,
    def: &AttentionDefinition,
    id: &str,
    tick: u64,
    update: AttentionUpdate,
) -> AttentionResult {
    store
        .apply_attention(command(
            store,
            def,
            id,
            tick,
            AttentionOperation::Observe {
                update: Box::new(update),
            },
        ))
        .expect("observe")
}
fn take_command(
    store: &Store,
    def: &AttentionDefinition,
    id: &str,
    tick: u64,
    who: &str,
) -> AttentionCommand {
    command(
        store,
        def,
        id,
        tick,
        AttentionOperation::TakeDelivery {
            recipient: recipient(who),
            intent: outbox::OutboxIntent {
                idempotency_key: format!("attention:{id}"),
                payload: json!({"context_ref":"render:immutable","do_not_publish":true}),
                not_before: tick,
            },
        },
    )
}
fn take(
    store: &Store,
    def: &AttentionDefinition,
    id: &str,
    tick: u64,
    who: &str,
) -> AttentionResult {
    store
        .apply_attention(take_command(store, def, id, tick, who))
        .expect("take")
}
fn wake(result: &AttentionResult) -> &governor::DeliveryBatch {
    match &result.receipt.outcome {
        AttentionOutcome::Delivery {
            decision: DeliveryDecision::Wake { delivery },
        } => delivery,
        other => panic!("expected actual governor wake, got {other:?}"),
    }
}
fn count(path: &Path, table: &str) -> i64 {
    let allowed = ["attention_journals", "attention_commands", "outbox_intents"];
    assert!(allowed.contains(&table));
    Connection::open(path)
        .expect("inspect")
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .expect("count")
}

#[test]
fn consumed_budget_survives_reopen_rebuild_backup_and_exact_historical_replay() {
    let temp = private_dir();
    let path = temp.path().join("store.db");
    let def = definition();
    let store = Store::open(&path).expect("open");
    let registration = store.register_attention(def.clone()).expect("register");
    assert!(!registration.replayed && !registration.authority_granted);
    observe(
        &store,
        &def,
        "observe-1",
        1,
        candidate(&def, 1, "captain-a"),
    );
    let first = take_command(&store, &def, "wake-1", 1, "captain-a");
    let original = store.apply_attention(first.clone()).expect("ordinary");
    assert_eq!(
        wake(&original).budget_class,
        governor::BudgetClass::Ordinary
    );
    assert!(
        !original.receipt.delivered
            && !original.receipt.application_acknowledged
            && !original.receipt.authority_granted
    );
    let original_bytes = serde_json::to_vec(&original.receipt).expect("bytes");
    let retry = store
        .apply_attention(first.clone())
        .expect("lost-reply retry");
    assert!(retry.replayed);
    assert_eq!(retry.receipt_digest, original.receipt_digest);
    assert_eq!(
        serde_json::to_vec(&retry.receipt).expect("bytes"),
        original_bytes
    );
    observe(
        &store,
        &def,
        "observe-2",
        2,
        candidate(&def, 2, "captain-a"),
    );
    let reserve = take(&store, &def, "wake-2", 2, "captain-a");
    assert_eq!(
        wake(&reserve).budget_class,
        governor::BudgetClass::CriticalReserve
    );
    observe(
        &store,
        &def,
        "observe-3",
        3,
        candidate(&def, 3, "captain-a"),
    );
    let deferred = take(&store, &def, "defer-3", 3, "captain-a");
    assert!(matches!(
        deferred.receipt.outcome,
        AttentionOutcome::Delivery {
            decision: DeliveryDecision::Deferred {
                pending_refs: 1,
                ..
            }
        }
    ));
    assert!(deferred.receipt.effect.is_none());
    let before = store
        .attention_status(def.scope.clone())
        .expect("status")
        .expect("journal");
    let old = store
        .apply_attention(first.clone())
        .expect("old historical retry");
    assert_eq!(old.receipt, original.receipt);
    assert_eq!(
        store.attention_status(def.scope.clone()).expect("status"),
        Some(before.clone())
    );
    assert!(
        store
            .register_attention(def.clone())
            .expect("register replay")
            .replayed
    );
    assert_eq!(count(&path, "outbox_intents"), 2);
    store.rebuild().expect("rebuild validates both operations");
    let backup = temp.path().join("backup.db");
    store.backup(&backup).expect("backup validated");
    drop(store);
    let reopened = Store::open(&path).expect("reopen");
    assert_eq!(
        reopened
            .attention_status(def.scope.clone())
            .expect("status"),
        Some(before.clone())
    );
    let still = take(&reopened, &def, "still-deferred", 4, "captain-a");
    assert!(matches!(
        still.receipt.outcome,
        AttentionOutcome::Delivery {
            decision: DeliveryDecision::Deferred { .. }
        }
    ));
    assert_eq!(count(&path, "outbox_intents"), 2);
    let next = take(&reopened, &def, "next-window", 60, "captain-a");
    assert_eq!(wake(&next).budget_class, governor::BudgetClass::Ordinary);
    let prior = Store::open(&backup).expect("backup reopen");
    assert_eq!(
        prior.attention_status(def.scope.clone()).expect("status"),
        Some(before)
    );
    assert_eq!(
        prior.apply_attention(first).expect("backup replay").receipt,
        original.receipt
    );
}

#[test]
fn exact_cas_concurrency_source_collision_and_independent_scope_fail_closed() {
    let temp = private_dir();
    let path = temp.path().join("store.db");
    let store = Store::open(&path).expect("open");
    let def = definition();
    store.register_attention(def.clone()).expect("register");
    let a = command(
        &store,
        &def,
        "a",
        1,
        AttentionOperation::Observe {
            update: Box::new(candidate(&def, 1, "captain-a")),
        },
    );
    let mut b = a.clone();
    b.command_id = "b".into();
    if let AttentionOperation::Observe { update } = &mut b.operation {
        update.source.sequence = "2".into();
    }
    let results = std::thread::scope(|scope| {
        let l = scope.spawn(|| store.apply_attention(a.clone()));
        let r = scope.spawn(|| store.apply_attention(b.clone()));
        [l.join().expect("thread"), r.join().expect("thread")]
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(StoreError::Attention(AttentionError::StaleCursor))))
            .count(),
        1
    );
    let successful = if results[0].is_ok() { a } else { b };
    let mut collision = successful.clone();
    collision.logical_tick = Some(2);
    assert!(matches!(
        store.apply_attention(collision),
        Err(StoreError::Attention(AttentionError::Collision))
    ));
    let mut source = if let AttentionOperation::Observe { update } = &successful.operation {
        update.clone()
    } else {
        unreachable!()
    };
    source.update = UpdateKind::Candidate {
        candidate_ref: "candidate:conflicting".into(),
        state: CandidateState::Submitted,
    };
    let cmd = command(
        &store,
        &def,
        "source-collision",
        2,
        AttentionOperation::Observe { update: source },
    );
    assert!(matches!(
        store.apply_attention(cmd),
        Err(StoreError::Attention(AttentionError::Governor(
            governor::GovernorError::SourceCollision
        )))
    ));
    let baseline = store.attention_status(def.scope.clone()).expect("baseline");
    for changed in [0, 1, 2, 3] {
        let mut scope = def.scope.clone();
        match changed {
            0 => scope.root_id = RootId::new("root-b").expect("root"),
            1 => scope.project_id = ProjectId::new("project-b").expect("project"),
            2 => scope.epoch = "epoch-2".into(),
            _ => scope.recipients = vec![recipient("foreign")],
        }
        assert!(store.attention_status(scope.clone()).is_err());
        let mut changed_def = def.clone();
        changed_def.scope = scope;
        assert!(matches!(
            store.register_attention(changed_def),
            Err(StoreError::Attention(AttentionError::Collision))
        ));
    }
    let mut changed_config = def.clone();
    changed_config.config.captain.ordinary_wakes = 2;
    assert!(matches!(
        store.register_attention(changed_config),
        Err(StoreError::Attention(AttentionError::Collision))
    ));
    assert_eq!(
        store
            .attention_status(def.scope.clone())
            .expect("unchanged"),
        baseline
    );
    assert_eq!(count(&path, "attention_commands"), 1);
    let mut wrong = candidate(&def, 100, "captain-a");
    wrong.scope.project_id = ProjectId::new("foreign").expect("project");
    assert!(
        store
            .apply_attention(command(
                &store,
                &def,
                "wrong",
                2,
                AttentionOperation::Observe {
                    update: Box::new(wrong)
                }
            ))
            .is_err()
    );
}

#[test]
fn late_sql_failure_rolls_back_wake_budget_journal_and_outbox_together() {
    let temp = private_dir();
    let path = temp.path().join("store.db");
    let store = Store::open(&path).expect("open");
    let def = definition();
    store.register_attention(def.clone()).expect("register");
    observe(&store, &def, "observe", 1, candidate(&def, 1, "captain-a"));
    let before = store.attention_status(def.scope.clone()).expect("status");
    let request = take_command(&store, &def, "wake", 1, "captain-a");
    let injection = Connection::open(&path).expect("fault injector");
    injection.execute_batch("CREATE TRIGGER attention_test_late_abort BEFORE INSERT ON attention_commands WHEN NEW.command_id='wake' BEGIN SELECT RAISE(ABORT,'late journal fault'); END;").expect("trigger");
    assert!(store.apply_attention(request.clone()).is_err());
    assert_eq!(count(&path, "attention_commands"), 1);
    assert_eq!(count(&path, "outbox_intents"), 0);
    assert_eq!(
        store
            .attention_status(def.scope.clone())
            .expect("unchanged"),
        before
    );
    injection
        .execute_batch("DROP TRIGGER attention_test_late_abort")
        .expect("remove fault");
    let retried = store
        .apply_attention(request)
        .expect("unaccepted command retry");
    assert_eq!(wake(&retried).budget_class, governor::BudgetClass::Ordinary);
    assert_eq!(count(&path, "attention_commands"), 2);
    assert_eq!(count(&path, "outbox_intents"), 1);
}

#[test]
fn historical_retry_preserves_unknown_outbox_and_mutable_delivery_state() {
    let temp = private_dir();
    let path = temp.path().join("store.db");
    let store = Store::open(&path).expect("open");
    let def = definition();
    store.register_attention(def.clone()).expect("register");
    observe(&store, &def, "observe", 1, candidate(&def, 1, "captain-a"));
    let request = take_command(&store, &def, "wake", 1, "captain-a");
    let result = store.apply_attention(request.clone()).expect("wake");
    let claim = store
        .claim_outbox_key(
            Some(1),
            "transport-owner".into(),
            2,
            "attention:wake".into(),
        )
        .expect("claim")
        .expect("due");
    assert_eq!(
        claim.id,
        result.receipt.effect.as_ref().expect("effect").outbox_id
    );
    store
        .fail_outbox_unknown(claim.token, "lost transport response".into())
        .expect("unknown");
    assert_eq!(
        store
            .apply_attention(request.clone())
            .expect("replay")
            .receipt,
        result.receipt
    );
    assert!(
        store
            .claim_outbox_key(
                Some(4),
                "transport-owner".into(),
                2,
                "attention:wake".into()
            )
            .expect("no retry")
            .is_none()
    );
    store.rebuild().expect("rebuild retains unknown");
    drop(store);
    let reopened = Store::open(&path).expect("reopen");
    assert_eq!(
        reopened.apply_attention(request).expect("replay").receipt,
        result.receipt
    );
    let state: String = Connection::open(&path)
        .expect("inspect")
        .query_row("SELECT state FROM outbox_intents", [], |r| r.get(0))
        .expect("state");
    assert_eq!(state, "unknown");
    assert_eq!(count(&path, "outbox_intents"), 1);
}

#[test]
fn existing_exact_foreign_outbox_cannot_consume_a_new_wake_budget() {
    let temp = private_dir();
    let first_path = temp.path().join("first.db");
    let second_path = temp.path().join("second.db");
    let def = definition();
    let first = Store::open(&first_path).expect("open");
    first.register_attention(def.clone()).expect("register");
    observe(&first, &def, "observe", 1, candidate(&def, 1, "captain-a"));
    let request = take_command(&first, &def, "wake", 1, "captain-a");
    first
        .apply_attention(request.clone())
        .expect("reference exact wake");
    let raw: (String, String, i64) = Connection::open(&first_path)
        .expect("inspect")
        .query_row(
            "SELECT idempotency_key,payload,not_before FROM outbox_intents",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("reference row");
    let second = Store::open(&second_path).expect("open");
    second.register_attention(def.clone()).expect("register");
    observe(&second, &def, "observe", 1, candidate(&def, 1, "captain-a"));
    // Deliberate legacy/foreign row fixture: not associated with any attention command.
    outbox::enqueue(
        &mut Connection::open(&second_path).expect("fixture connection"),
        outbox::OutboxIntent {
            idempotency_key: raw.0,
            payload: serde_json::from_str(&raw.1).expect("json"),
            not_before: raw.2 as u64,
        },
    )
    .expect("foreign row");
    let before = second.attention_status(def.scope.clone()).expect("status");
    assert!(matches!(
        second.apply_attention(request),
        Err(StoreError::Attention(AttentionError::Collision))
    ));
    assert_eq!(
        second
            .attention_status(def.scope.clone())
            .expect("unchanged"),
        before
    );
    assert_eq!(count(&second_path, "attention_commands"), 1);
    assert_eq!(count(&second_path, "outbox_intents"), 1);
}

#[test]
fn progress_storms_coalesce_but_protected_transitions_and_recipient_reserves_survive() {
    let temp = private_dir();
    let path = temp.path().join("store.db");
    let store = Store::open(&path).expect("open");
    let mut def = definition();
    def.config.captain.ordinary_wakes = 2;
    def.config.captain.max_delivery_refs = 16;
    store.register_attention(def.clone()).expect("register");
    for n in 1..=32 {
        observe(
            &store,
            &def,
            &format!("progress-{n}"),
            1,
            update(
                &def,
                n,
                "captain-a",
                UpdateKind::Progress {
                    phase: "implementation".into(),
                    completed: n as u32,
                    total: 32,
                },
            ),
        );
    }
    assert_eq!(
        store
            .attention_status(def.scope.clone())
            .expect("status")
            .expect("journal")
            .pending_refs,
        0
    );
    assert!(matches!(
        take(&store, &def, "quiet", 1, "captain-a").receipt.outcome,
        AttentionOutcome::Delivery {
            decision: DeliveryDecision::Quiet
        }
    ));
    for (n, state) in [(33, BlockState::Raised), (34, BlockState::Cleared)] {
        observe(
            &store,
            &def,
            &format!("block-{n}"),
            2,
            update(
                &def,
                n,
                "captain-a",
                UpdateKind::Blocked {
                    blocker_id: "blocker-a".into(),
                    state,
                },
            ),
        );
    }
    let facts = QuestionFacts {
        question: "Do not publish; is 5 ms within the limit?".into(),
        options: vec!["No".into(), "Yes".into()],
        protected_constraints: vec!["Unknown is not zero; do not publish".into()],
        contract_digest: format!("sha256:{}", "1".repeat(64)),
    };
    for (n, state) in [(35, QuestionState::Open), (36, QuestionState::Answered)] {
        observe(
            &store,
            &def,
            &format!("question-{n}"),
            2,
            update(
                &def,
                n,
                "captain-a",
                UpdateKind::Question {
                    question_id: admiralty_core::MessageId::new("question-a").expect("question"),
                    state,
                    facts: facts.clone(),
                },
            ),
        );
    }
    let result = take(&store, &def, "protected", 2, "captain-a");
    let batch = wake(&result);
    assert!(batch.reasons.contains(&WakeReason::BlockedRaised));
    assert!(batch.reasons.contains(&WakeReason::BlockedCleared));
    assert!(batch.reasons.contains(&WakeReason::QuestionOpened));
    assert!(batch.reasons.contains(&WakeReason::QuestionAnswered));
    assert_eq!(
        batch
            .refs
            .iter()
            .map(|r| r.source.sequence.as_str())
            .collect::<Vec<_>>(),
        vec!["32", "33", "34", "35", "36"]
    );
    let mut exhausted = definition();
    exhausted.scope.journal_id = "reserves".into();
    store
        .register_attention(exhausted.clone())
        .expect("second journal");
    for (n, who, id) in [
        (1, "captain-a", "a-ordinary"),
        (2, "captain-a", "a-reserve"),
        (3, "captain-b", "b-ordinary"),
        (4, "captain-b", "b-reserve"),
    ] {
        observe(
            &store,
            &exhausted,
            &format!("observe-{id}"),
            3,
            candidate(&exhausted, n, who),
        );
        let batch = take(&store, &exhausted, id, 3, who);
        assert_eq!(
            wake(&batch).budget_class,
            if id.ends_with("ordinary") {
                governor::BudgetClass::Ordinary
            } else {
                governor::BudgetClass::CriticalReserve
            }
        );
    }
    observe(
        &store,
        &exhausted,
        "last",
        4,
        candidate(&exhausted, 5, "captain-a"),
    );
    assert!(matches!(
        take(&store, &exhausted, "full", 4, "captain-a")
            .receipt
            .outcome,
        AttentionOutcome::Delivery {
            decision: DeliveryDecision::Deferred {
                pending_refs: 1,
                ..
            }
        }
    ));
    store.rebuild().expect("all transitions replay");
}

#[test]
fn clock_and_governor_capacity_rejections_do_not_ack_evict_or_reset() {
    let temp = private_dir();
    let path = temp.path().join("store.db");
    let store = Store::open(&path).expect("open");
    let mut def = definition();
    def.config.max_source_ids = 1;
    store.register_attention(def.clone()).expect("register");
    let mut unknown = command(
        &store,
        &def,
        "unknown",
        10,
        AttentionOperation::Observe {
            update: Box::new(candidate(&def, 1, "captain-a")),
        },
    );
    unknown.logical_tick = None;
    assert!(matches!(
        store.apply_attention(unknown),
        Err(StoreError::Attention(AttentionError::Governor(
            governor::GovernorError::UnknownClock
        )))
    ));
    assert_eq!(count(&path, "attention_commands"), 0);
    observe(&store, &def, "first", 10, candidate(&def, 1, "captain-a"));
    let before = store.attention_status(def.scope.clone()).expect("status");
    let backwards = take_command(&store, &def, "backwards", 9, "captain-a");
    assert!(matches!(
        store.apply_attention(backwards),
        Err(StoreError::Attention(AttentionError::Governor(
            governor::GovernorError::BackwardClock
        )))
    ));
    let capacity = command(
        &store,
        &def,
        "capacity",
        11,
        AttentionOperation::Observe {
            update: Box::new(candidate(&def, 2, "captain-a")),
        },
    );
    assert!(matches!(
        store.apply_attention(capacity.clone()),
        Err(StoreError::Attention(AttentionError::Governor(
            governor::GovernorError::Capacity(_)
        )))
    ));
    assert_eq!(
        store
            .attention_status(def.scope.clone())
            .expect("unchanged"),
        before
    );
    take(&store, &def, "drain", 11, "captain-a");
    let mut retry = capacity;
    retry.expected_prior = store
        .attention_status(def.scope.clone())
        .expect("status")
        .expect("journal")
        .cursor;
    assert!(matches!(
        store.apply_attention(retry),
        Err(StoreError::Attention(AttentionError::Governor(
            governor::GovernorError::Capacity(_)
        )))
    ));
    assert_eq!(count(&path, "attention_commands"), 2);
    for n in 1..MAX_JOURNALS {
        let mut d = definition();
        d.scope.journal_id = format!("journal-{n}");
        store.register_attention(d).expect("bounded registration");
    }
    let mut overflow = definition();
    overflow.scope.journal_id = "overflow".into();
    assert!(matches!(
        store.register_attention(overflow),
        Err(StoreError::Attention(AttentionError::Capacity))
    ));
    assert_eq!(count(&path, "attention_journals"), MAX_JOURNALS as i64);
    store.rebuild().expect("no eviction");
}

#[test]
fn corrupted_history_oversized_and_wrong_type_fields_fail_open_rebuild_and_backup() {
    for corruption in [
        "command",
        "receipt",
        "head",
        "outbox",
        "oversized",
        "wrong-type",
    ] {
        let temp = private_dir();
        let path = temp.path().join("store.db");
        let store = Store::open(&path).expect("open");
        let def = definition();
        store.register_attention(def.clone()).expect("register");
        observe(&store, &def, "observe", 1, candidate(&def, 1, "captain-a"));
        take(&store, &def, "wake", 1, "captain-a");
        let injection = Connection::open(&path).expect("injector");
        match corruption {
            "command"=>injection.execute_batch("DROP TRIGGER attention_command_immutable_update;UPDATE attention_commands SET command_json=json_set(command_json,'$.request.command.logical_tick',2) WHERE command_id='observe';"),
            "receipt"=>injection.execute_batch("DROP TRIGGER attention_command_immutable_update;UPDATE attention_commands SET receipt_json=json_set(receipt_json,'$.delivered',json('true')) WHERE command_id='wake';"),
            "head"=>injection.execute_batch("UPDATE attention_journals SET head_sequence=1;"),
            "outbox"=>injection.execute_batch("DROP TRIGGER outbox_immutable_intent;UPDATE outbox_intents SET not_before=2;"),
            "oversized"=>injection.execute_batch("PRAGMA ignore_check_constraints=ON;DROP TRIGGER attention_command_immutable_update;UPDATE attention_commands SET receipt_json=printf('%.*c',40000,'x') WHERE command_id='wake';"),
            _=>injection.execute_batch("PRAGMA writable_schema=ON;UPDATE sqlite_schema SET sql=replace(sql,') STRICT;',');') WHERE name='attention_commands';PRAGMA writable_schema=RESET;")
        }.expect("fixture corrupt");
        if corruption == "wrong-type" {
            // SQLite STRICT may still reject a BLOB injection. A fresh non-STRICT
            // adversarial table fixture exercises Store's borrowed type check.
            injection.execute_batch("PRAGMA foreign_keys=OFF;DROP TRIGGER IF EXISTS attention_command_immutable_update;DROP TRIGGER IF EXISTS attention_command_immutable_delete;ALTER TABLE attention_commands RENAME TO old_attention_commands;CREATE TABLE attention_commands AS SELECT * FROM old_attention_commands;DROP TABLE old_attention_commands;UPDATE attention_commands SET receipt_json=CAST(receipt_json AS BLOB) WHERE command_id='wake';").expect("wrong type fixture");
        }
        assert!(store.rebuild().is_err(), "{corruption} rebuild must refuse");
        let backup = temp.path().join("should-not-publish.db");
        assert!(
            store.backup(&backup).is_err(),
            "{corruption} backup must refuse"
        );
        assert!(
            !backup.exists(),
            "{corruption} must fail before backup publication"
        );
        drop(store);
        drop(injection);
        assert!(
            Store::open(&path).is_err(),
            "{corruption} reopen must refuse"
        );
    }
}

#[test]
fn full_wake_envelope_capacity_is_checked_and_does_not_consume_budget() {
    let temp = private_dir();
    let path = temp.path().join("store.db");
    let store = Store::open(&path).expect("open");
    let mut def = definition();
    def.config.captain.max_delivery_refs = 128;
    def.config.captain.max_delivery_bytes = 16384;
    store.register_attention(def.clone()).expect("register");
    for n in 1..=36 {
        observe(
            &store,
            &def,
            &format!("observe-{n}"),
            1,
            candidate(&def, n, "captain-a"),
        );
    }
    let before = store.attention_status(def.scope.clone()).expect("status");
    let mut request = take_command(&store, &def, "oversized-wake", 1, "captain-a");
    if let AttentionOperation::TakeDelivery { intent, .. } = &mut request.operation {
        intent.payload = json!({"metadata":"x".repeat(4000)});
    }
    assert!(store.apply_attention(request).is_err());
    assert_eq!(
        store
            .attention_status(def.scope.clone())
            .expect("unchanged"),
        before
    );
    assert_eq!(count(&path, "outbox_intents"), 0);
    let result = take(&store, &def, "bounded-wake", 1, "captain-a");
    assert_eq!(wake(&result).budget_class, governor::BudgetClass::Ordinary);
    assert_eq!(wake(&result).refs.len(), 36);
}
