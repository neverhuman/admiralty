//! Scoped attention recording with writer-bound current authentication/policy.
//!
//! Trusted startup must install the same reviewed writer authority in Store and
//! this adapter. Inbox read grants alone never authorize a mutation. The Store
//! validates actual retained capsules on its sole writer; every command/retry
//! additionally checks the real opaque actor and current shared credential state.
//! This records observations/wake intents only. It does not send provider input,
//! acknowledge delivery, advance baselines, or prove pending delivery.refs were
//! compiled into the separately retained request_context. External policy changes
//! after a callback returns remain the issuer's synchronization responsibility.
use crate::services::coordination::{
    AuthenticatedCoordinationActor, CoordinationCredentialAuthority,
};
use admiralty_core::{
    PrincipalId, SourceIdentity, content_digest,
    governor::{AttentionUpdate, Recipient},
};
pub use admiralty_store::attention::AttentionContextPin;
use admiralty_store::{
    Store, StoreError,
    attention::{
        AttentionAuthority, AttentionCommand, AttentionCommandBinding, AttentionCursor,
        AttentionDefinition, AttentionDefinitionBinding, AttentionOperation, AttentionOutcome,
        AttentionRequestBinding, AttentionResult, AuthorizedAttentionCommand,
        authorized_request_digest,
    },
    outbox::OutboxIntent,
};
use serde::Serialize;
use std::{
    any::Any,
    sync::{Arc, RwLock},
};

const MAX_REQUEST_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AttentionRequest {
    pub command_id: String,
    pub expected_prior: Option<AttentionCursor>,
    /// Explicit caller-owned recorded logical time, never a synthesized clock.
    pub logical_tick: Option<u64>,
    pub context: AttentionContextPin,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeAttentionAction {
    Observe,
    TakeAtSafeTurn,
}

/// Current recording permission only. All references and digests remain data;
/// the policy independently owns scope/role/epoch/source/evidence membership.
/// It must be bounded and perform no I/O, Store calls or lock-taking side effects.
pub struct NativeAttentionBinding<'a> {
    pub action: NativeAttentionAction,
    pub actor: &'a PrincipalId,
    pub scope: &'a admiralty_store::attention::AttentionScope,
    pub config_digest: &'a str,
    pub policy_digest: &'a str,
    pub recipient: &'a Recipient,
    pub context: &'a AttentionContextPin,
    pub source: Option<&'a SourceIdentity>,
    pub evidence_ref: &'a str,
    pub command: &'a AttentionCommand,
    pub request_digest: &'a str,
    /// Actual computed outcome at the Store's final check, not a delivery ACK
    /// or proof pending references were compiled into request_context.
    pub outcome: Option<&'a AttentionOutcome>,
}
pub trait NativeAttentionPolicy: Send + Sync {
    fn authorize_current(&self, binding: &NativeAttentionBinding<'_>) -> bool;
}
#[derive(Debug)]
pub enum NativeAttentionError {
    AccessDenied,
    InvalidBinding,
    ContextUnavailable,
    Store(StoreError),
}
impl std::fmt::Display for NativeAttentionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::AccessDenied => "attention recording is not authorized",
            Self::InvalidBinding => "attention request exceeds its exact scope or bounds",
            Self::ContextUnavailable => "exact attention context is unavailable",
            Self::Store(_) => "attention storage operation failed",
        })
    }
}
impl std::error::Error for NativeAttentionError {}

/// Non-deserializable trusted startup composition. Definition permission is
/// separate from caller action permission. No token bytes or JSON proof are
/// copied into Store. The registry must be the actual current shared authority.
pub struct NativeAttentionWriterAuthority {
    definition: AttentionDefinition,
    definition_digest: String,
    config_digest: String,
    policy_digest: String,
    policy: Arc<dyn NativeAttentionPolicy>,
    credentials: Arc<RwLock<CoordinationCredentialAuthority>>,
}
impl NativeAttentionWriterAuthority {
    pub fn configured(
        definition: AttentionDefinition,
        policy_digest: String,
        policy: Arc<dyn NativeAttentionPolicy>,
        credentials: Arc<RwLock<CoordinationCredentialAuthority>>,
    ) -> Result<Self, NativeAttentionError> {
        admiralty_core::limits::bounded_json_size(
            &definition,
            admiralty_store::attention::MAX_DEFINITION_BYTES,
        )
        .map_err(|_| NativeAttentionError::InvalidBinding)?;
        admiralty_core::governor::AttentionGovernor::new(definition.config.clone())
            .map_err(|_| NativeAttentionError::InvalidBinding)?;
        if !is_digest(&policy_digest) {
            return Err(NativeAttentionError::InvalidBinding);
        }
        let definition_digest =
            content_digest(&definition).map_err(|_| NativeAttentionError::InvalidBinding)?;
        let config_digest =
            content_digest(&definition.config).map_err(|_| NativeAttentionError::InvalidBinding)?;
        Ok(Self {
            definition,
            definition_digest,
            config_digest,
            policy_digest,
            policy,
            credentials,
        })
    }
    fn active(&self, actor: &AuthenticatedCoordinationActor) -> bool {
        self.credentials
            .try_read()
            .is_ok_and(|registry| registry.is_active_actor(actor))
    }
    fn permits(
        &self,
        actor: &AuthenticatedCoordinationActor,
        request: &AuthorizedAttentionCommand,
        digest: &str,
        outcome: Option<&AttentionOutcome>,
    ) -> bool {
        if !self.active(actor)
            || actor.principal() != &request.binding.actor
            || actor.credential_id() != request.binding.credential_ref
            || request.binding.policy_digest != self.policy_digest
            || request.command.scope != self.definition.scope
            || request.command.config_digest != self.config_digest
            || !authorized_request_digest(&self.definition_digest, request)
                .is_ok_and(|actual| actual == digest)
        {
            return false;
        }
        let (action, recipient, source) = match &request.command.operation {
            AttentionOperation::Observe { update } => (
                NativeAttentionAction::Observe,
                &update.recipient,
                Some(&update.source),
            ),
            AttentionOperation::TakeDelivery { recipient, .. } => {
                (NativeAttentionAction::TakeAtSafeTurn, recipient, None)
            }
        };
        let binding = NativeAttentionBinding {
            action,
            actor: actor.principal(),
            scope: &request.command.scope,
            config_digest: &request.command.config_digest,
            policy_digest: &self.policy_digest,
            recipient,
            context: &request.binding.context,
            source,
            evidence_ref: &request.binding.context.render.record_digest,
            command: &request.command,
            request_digest: digest,
            outcome,
        };
        // Never hold a registry lock across an external policy callback or a
        // Store queue operation. Contention/poisoning denies, never waits.
        self.policy.authorize_current(&binding) && self.active(actor)
    }
}
impl AttentionAuthority for NativeAttentionWriterAuthority {
    fn authorize_definition(&self, b: &AttentionDefinitionBinding<'_>) -> bool {
        b.definition == &self.definition
            && b.definition_digest == self.definition_digest
            && b.config_digest == self.config_digest
    }
    fn authorize_command(
        &self,
        b: &AttentionCommandBinding<'_>,
        proof: &(dyn Any + Send + Sync),
    ) -> bool {
        let Some(actor) = proof.downcast_ref::<AuthenticatedCoordinationActor>() else {
            return false;
        };
        b.definition == &self.definition
            && b.definition_digest == self.definition_digest
            && b.stored_context.scope == b.request.binding.context.scope
            && content_digest(b.stored_context)
                .is_ok_and(|digest| digest == b.request.binding.context.render.record_digest)
            && self.permits(actor, b.request, b.request_digest, b.outcome)
    }
}

pub struct NativeAttentionService {
    store: Store,
    issuer: Arc<NativeAttentionWriterAuthority>,
}
impl NativeAttentionService {
    /// The issuer must already be installed in Store through trusted startup.
    /// Absence is denied at construction, even when a read credential exists.
    pub fn configured(
        store: Store,
        definition: AttentionDefinition,
        issuer: Option<Arc<NativeAttentionWriterAuthority>>,
    ) -> Result<Self, NativeAttentionError> {
        let issuer = issuer.ok_or(NativeAttentionError::AccessDenied)?;
        if definition != issuer.definition {
            return Err(NativeAttentionError::InvalidBinding);
        }
        let registration = store
            .register_attention(definition)
            .map_err(NativeAttentionError::Store)?;
        if registration.definition_digest != issuer.definition_digest
            || registration.config_digest != issuer.config_digest
        {
            return Err(NativeAttentionError::InvalidBinding);
        }
        Ok(Self { store, issuer })
    }
    pub fn observe_authenticated(
        &self,
        actor: &AuthenticatedCoordinationActor,
        request: AttentionRequest,
        update: AttentionUpdate,
    ) -> Result<AttentionResult, NativeAttentionError> {
        let bound = self.bind(
            actor,
            request,
            AttentionOperation::Observe {
                update: Box::new(update),
            },
        );
        self.record(actor, bound)
    }
    /// Records the safe-turn request separately from observed pending refs.
    /// Neither this call nor a recorded outbox intent proves model delivery.
    #[allow(clippy::too_many_arguments)]
    pub fn take_at_safe_turn(
        &self,
        actor: &AuthenticatedCoordinationActor,
        request: AttentionRequest,
        recipient: Recipient,
        idempotency_key: String,
        not_before: u64,
    ) -> Result<AttentionResult, NativeAttentionError> {
        let bound = self.bind_take(actor, request, recipient, idempotency_key, not_before)?;
        self.record(actor, bound)
    }
    fn bind(
        &self,
        actor: &AuthenticatedCoordinationActor,
        request: AttentionRequest,
        operation: AttentionOperation,
    ) -> AuthorizedAttentionCommand {
        AuthorizedAttentionCommand {
            command: AttentionCommand {
                scope: self.issuer.definition.scope.clone(),
                config_digest: self.issuer.config_digest.clone(),
                command_id: request.command_id,
                expected_prior: request.expected_prior,
                logical_tick: request.logical_tick,
                operation,
            },
            binding: AttentionRequestBinding {
                actor: actor.principal().clone(),
                credential_ref: actor.credential_id().to_owned(),
                policy_digest: self.issuer.policy_digest.clone(),
                context: request.context,
            },
        }
    }
    fn bind_take(
        &self,
        actor: &AuthenticatedCoordinationActor,
        request: AttentionRequest,
        recipient: Recipient,
        idempotency_key: String,
        not_before: u64,
    ) -> Result<AuthorizedAttentionCommand, NativeAttentionError> {
        admiralty_core::limits::bounded_json_size(&request, MAX_REQUEST_BYTES)
            .map_err(|_| NativeAttentionError::InvalidBinding)?;
        let payload = serde_json::json!({
            "kind":"native_attention_context","request_context":request.context,
            "delivered":false,"application_acknowledged":false,"authority_granted":false,
        });
        Ok(self.bind(
            actor,
            request,
            AttentionOperation::TakeDelivery {
                recipient,
                intent: OutboxIntent {
                    idempotency_key,
                    payload,
                    not_before,
                },
            },
        ))
    }
    fn record(
        &self,
        actor: &AuthenticatedCoordinationActor,
        bound: AuthorizedAttentionCommand,
    ) -> Result<AttentionResult, NativeAttentionError> {
        let digest = authorized_request_digest(&self.issuer.definition_digest, &bound)
            .map_err(|_| NativeAttentionError::InvalidBinding)?;
        if !self.issuer.permits(actor, &bound, &digest, None) {
            return Err(NativeAttentionError::AccessDenied);
        }
        self.store
            .apply_attention_authorized(bound, Arc::new(actor.clone()))
            .map_err(NativeAttentionError::Store)
    }
}
fn is_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::coordination::{
        CoordinationAction, CoordinationCredential, CoordinationCredentialGrant,
    };
    use admiralty_core::{
        AcceptanceCriterion, CheckSpec, ProjectId, RootId, TaskContract, TaskId,
        coordination::context::{
            BudgetObservation, CheckObservation, FactObservation, ProjectExceptionFacts,
            ProtectedTaskFacts, RecipientRole, compile_project_exceptions,
        },
        governor::{BlockState, GovernorConfig, Role, Scope, UpdateKind},
    };
    use admiralty_store::{
        StoreAuthorities,
        attention::{AttentionError, AttentionScope},
        context_renders::{ContextNamespace, ContextScope, RecordContextRender, RenderKey},
        inbox::InboxScope,
    };
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::{
            Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
        time::Duration,
    };
    struct Gate {
        entered: mpsc::SyncSender<()>,
        resume: mpsc::Receiver<()>,
    }
    struct ExactPolicy {
        scope: AttentionScope,
        config_digest: String,
        policy_digest: String,
        definition_digest: String,
        contexts: Mutex<BTreeMap<String, AttentionContextPin>>,
        allowed: Mutex<BTreeSet<String>>,
        revoked: AtomicBool,
        deny_final: AtomicBool,
        calls: AtomicUsize,
        gate: Mutex<Option<Gate>>,
    }
    impl ExactPolicy {
        fn admit(&self, request: &AuthorizedAttentionCommand) {
            self.allowed.lock().expect("exact requests").insert(
                authorized_request_digest(&self.definition_digest, request)
                    .expect("bounded exact request"),
            );
        }
    }
    impl NativeAttentionPolicy for ExactPolicy {
        fn authorize_current(&self, b: &NativeAttentionBinding<'_>) -> bool {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 1 {
                let gate = self.gate.lock().expect("gate").take();
                if let Some(gate) = gate
                    && (gate.entered.send(()).is_err()
                        || gate.resume.recv_timeout(Duration::from_secs(5)).is_err())
                {
                    return false;
                }
            }
            !(self.revoked.load(Ordering::SeqCst)
                || self.deny_final.load(Ordering::SeqCst) && b.outcome.is_some())
                && b.scope == &self.scope
                && b.command.scope == self.scope
                && b.config_digest == self.config_digest
                && b.policy_digest == self.policy_digest
                && b.actor == &b.recipient.principal_id
                && self.scope.recipients.contains(b.recipient)
                && self
                    .contexts
                    .lock()
                    .expect("actual context pins")
                    .get(b.evidence_ref)
                    == Some(b.context)
                && match b.action {
                    NativeAttentionAction::Observe => b.source.is_some_and(|s| {
                        s.producer_id == "trusted-source" && s.incarnation == "trusted-boot"
                    }),
                    NativeAttentionAction::TakeAtSafeTurn => b.source.is_none(),
                }
                && self
                    .allowed
                    .lock()
                    .expect("exact requests")
                    .contains(b.request_digest)
        }
    }
    struct Fixture {
        store: Store,
        definition: AttentionDefinition,
        service: NativeAttentionService,
        credentials: Arc<RwLock<CoordinationCredentialAuthority>>,
        actor: AuthenticatedCoordinationActor,
        policy: Arc<ExactPolicy>,
        issuer: Arc<NativeAttentionWriterAuthority>,
        request: AttentionRequest,
        update: AttentionUpdate,
        _dir: tempfile::TempDir,
    }
    fn fixture() -> Fixture {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("private fixture");
        let path = dir.path().join("attention.sqlite");
        let store = Store::open(&path).expect("context storage only, no attention authority");
        let project = ProjectId::new("project").expect("project");
        let root = RootId::new("root").expect("root");
        let principal = PrincipalId::new("commodore").expect("principal");
        let recipient = Recipient {
            role: Role::Captain,
            principal_id: principal.clone(),
        };
        let contract = TaskContract {
            schema_version: 1,
            project_id: project.clone(),
            root_id: root.clone(),
            task_id: TaskId::new("task").expect("task"),
            revision: 1,
            title: "Exact task".into(),
            objective: "Carry a scoped reference".into(),
            non_goals: vec!["Do NOT publish".into()],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec!["private".into()],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "scope".into(),
                text: "Exact scope".into(),
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
                    reason: "Not submitted".into(),
                },
                check: CheckObservation::NotRun {
                    reason: "Not executed".into(),
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
        let capsule = compile_project_exceptions(&facts, "epoch", 8192).expect("actual compiler");
        let scope = ContextScope {
            recipient: principal.clone(),
            role: RecipientRole::Commodore,
            namespace: ContextNamespace::Project {
                project_id: project.clone(),
                root_id: root.clone(),
            },
        };
        let recorded = store
            .record_context_render(RecordContextRender {
                scope: scope.clone(),
                key: RenderKey {
                    render_id: "render".into(),
                    version: 1,
                },
                expected_epoch: "epoch".into(),
                expected_facts_digest: capsule.facts_digest.clone(),
                capsule: capsule.clone(),
            })
            .expect("actual retained context");
        let context = AttentionContextPin {
            scope,
            render: recorded.render,
            facts_digest: capsule.facts_digest,
        };
        let definition = AttentionDefinition {
            scope: AttentionScope {
                journal_id: "journal".into(),
                project_id: project.clone(),
                root_id: root,
                epoch: "epoch".into(),
                recipients: vec![recipient.clone()],
            },
            config: GovernorConfig::default(),
        };
        let policy_digest = content_digest(&(
            "explicit-fixture-action-policy",
            &definition,
            "trusted-source",
            "trusted-boot",
        ))
        .expect("actual policy config identity");
        let policy = Arc::new(ExactPolicy {
            scope: definition.scope.clone(),
            config_digest: content_digest(&definition.config).expect("config"),
            policy_digest: policy_digest.clone(),
            definition_digest: content_digest(&definition).expect("definition"),
            contexts: Mutex::new(BTreeMap::from([(
                context.render.record_digest.clone(),
                context.clone(),
            )])),
            allowed: Mutex::new(BTreeSet::new()),
            revoked: AtomicBool::new(false),
            deny_final: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            gate: Mutex::new(None),
        });
        let credentials = Arc::new(RwLock::new(
            CoordinationCredentialAuthority::new(vec![CoordinationCredential::configured(
                "credential".into(),
                "attention-fixture-token-01234567890123456789".into(),
                principal.clone(),
                vec![CoordinationCredentialGrant {
                    scope: InboxScope {
                        project_id: project.clone(),
                        recipient: principal,
                        context_epoch: "epoch".into(),
                    },
                    action: CoordinationAction::ReadPending,
                    message_id: None,
                    source: None,
                }],
            )])
            .expect("configured real credential registry"),
        ));
        let actor = credentials
            .read()
            .expect("registry")
            .authenticate("attention-fixture-token-01234567890123456789")
            .expect("actual authentication");
        let issuer = Arc::new(
            NativeAttentionWriterAuthority::configured(
                definition.clone(),
                policy_digest,
                policy.clone(),
                credentials.clone(),
            )
            .expect("explicit reviewed writer issuer"),
        );
        drop(store);
        let store = Store::open_with_startup_authorities(
            path,
            StoreAuthorities {
                attention: Some(issuer.clone()),
                ..StoreAuthorities::default()
            },
        )
        .expect("actual installed writer authority");
        let service = NativeAttentionService::configured(
            store.clone(),
            definition.clone(),
            Some(issuer.clone()),
        )
        .expect("explicit protected bridge");
        let request = AttentionRequest {
            command_id: "observe".into(),
            expected_prior: None,
            logical_tick: Some(1),
            context,
        };
        let update = AttentionUpdate {
            source: SourceIdentity {
                producer_id: "trusted-source".into(),
                incarnation: "trusted-boot".into(),
                sequence: "1".into(),
            },
            scope: Scope {
                project_id: project,
                task_id: contract.task_id,
                revision: 1,
            },
            recipient,
            authority_ref: contract.authority_ref,
            evidence_ref: request.context.render.record_digest.clone(),
            update: UpdateKind::Blocked {
                blocker_id: "blocker".into(),
                state: BlockState::Raised,
            },
        };
        policy.admit(&service.bind(
            &actor,
            request.clone(),
            AttentionOperation::Observe {
                update: Box::new(update.clone()),
            },
        ));
        Fixture {
            store,
            definition,
            service,
            credentials,
            actor,
            policy,
            issuer,
            request,
            update,
            _dir: dir,
        }
    }
    fn take(
        f: &Fixture,
        prior: AttentionCursor,
        id: &str,
        context: AttentionContextPin,
    ) -> AttentionRequest {
        let request = AttentionRequest {
            command_id: id.into(),
            expected_prior: Some(prior),
            logical_tick: Some(2),
            context,
        };
        let bound = f
            .service
            .bind_take(
                &f.actor,
                request.clone(),
                f.update.recipient.clone(),
                id.into(),
                0,
            )
            .expect("bounded take");
        f.policy.admit(&bound);
        request
    }
    #[test]
    fn authenticated_safe_turn_records_exact_outbox_once_without_delivery() {
        let f = fixture();
        let observed = f
            .service
            .observe_authenticated(&f.actor, f.request.clone(), f.update.clone())
            .expect("actual protected observe");
        let request = take(
            &f,
            observed.receipt.cursor,
            "take",
            f.request.context.clone(),
        );
        let result = f
            .service
            .take_at_safe_turn(
                &f.actor,
                request.clone(),
                f.update.recipient.clone(),
                "take".into(),
                0,
            )
            .expect("protected wake");
        assert!(result.receipt.recorded_wake);
        assert!(
            !result.receipt.delivered
                && !result.receipt.application_acknowledged
                && !result.receipt.authority_granted
        );
        let replay = f
            .service
            .take_at_safe_turn(
                &f.actor,
                request,
                f.update.recipient.clone(),
                "take".into(),
                0,
            )
            .expect("exact retry");
        assert!(replay.replayed);
        assert_eq!(replay.receipt, result.receipt);
        let claim = f
            .store
            .claim_outbox_key(Some(0), "dispatcher".into(), 10, "take".into())
            .expect("claim")
            .expect("one exact effect");
        assert_eq!(
            claim.payload["payload"]["request_context"]["render"]["record_digest"],
            f.request.context.render.record_digest
        );
        assert_eq!(claim.payload["payload"]["delivered"], false);
    }
    #[test]
    fn safe_turn_context_b_does_not_replace_pending_evidence_a() {
        let f = fixture();
        let observed = f
            .service
            .observe_authenticated(&f.actor, f.request.clone(), f.update.clone())
            .expect("observe context A");
        let stored = f
            .store
            .context_render(
                f.request.context.scope.clone(),
                f.request.context.render.key.clone(),
            )
            .expect("context read")
            .expect("A");
        let record = f
            .store
            .record_context_render(RecordContextRender {
                scope: stored.scope.clone(),
                key: RenderKey {
                    render_id: "context-b".into(),
                    version: 1,
                },
                expected_epoch: stored.capsule.context_epoch.clone(),
                expected_facts_digest: stored.capsule.facts_digest.clone(),
                capsule: stored.capsule,
            })
            .expect("real context B");
        let mut context = f.request.context.clone();
        context.render = record.render;
        f.policy
            .contexts
            .lock()
            .expect("actual contexts")
            .insert(context.render.record_digest.clone(), context.clone());
        let request = take(&f, observed.receipt.cursor, "take-b", context.clone());
        f.service
            .take_at_safe_turn(&f.actor, request, f.update.recipient, "take-b".into(), 0)
            .expect("record context B request separately");
        let claim = f
            .store
            .claim_outbox_key(Some(0), "dispatcher".into(), 10, "take-b".into())
            .expect("claim")
            .expect("wake");
        assert_eq!(
            claim.payload["payload"]["request_context"]["render"]["record_digest"],
            context.render.record_digest
        );
        assert_eq!(
            claim.payload["delivery"]["refs"][0]["evidence_ref"],
            f.update.evidence_ref
        );
        assert_ne!(
            claim.payload["payload"]["request_context"]["render"]["record_digest"],
            claim.payload["delivery"]["refs"][0]["evidence_ref"]
        );
    }
    #[test]
    fn absent_issuer_and_read_grant_without_action_policy_never_allow_mutation() {
        let f = fixture();
        assert!(matches!(
            NativeAttentionService::configured(f.store.clone(), f.definition.clone(), None),
            Err(NativeAttentionError::AccessDenied)
        ));
        f.policy.revoked.store(true, Ordering::SeqCst);
        assert!(matches!(
            f.service
                .observe_authenticated(&f.actor, f.request.clone(), f.update.clone()),
            Err(NativeAttentionError::AccessDenied)
        ));
        assert!(
            f.store
                .attention_status(f.definition.scope)
                .expect("startup status only")
                .expect("journal")
                .cursor
                .is_none()
        );
    }
    #[test]
    fn actual_registry_revoke_foreign_actor_and_lock_contention_fail_closed() {
        let f = fixture();
        let foreign =
            CoordinationCredentialAuthority::new(vec![CoordinationCredential::configured(
                "credential".into(),
                "other-fixture-token-01234567890123456789".into(),
                f.actor.principal().clone(),
                vec![CoordinationCredentialGrant {
                    scope: InboxScope {
                        project_id: f.definition.scope.project_id.clone(),
                        recipient: f.actor.principal().clone(),
                        context_epoch: f.definition.scope.epoch.clone(),
                    },
                    action: CoordinationAction::ReadPending,
                    message_id: None,
                    source: None,
                }],
            )])
            .expect("other registry");
        let foreign_actor = foreign
            .authenticate("other-fixture-token-01234567890123456789")
            .expect("actual but foreign actor");
        assert!(matches!(
            f.service
                .observe_authenticated(&foreign_actor, f.request.clone(), f.update.clone()),
            Err(NativeAttentionError::AccessDenied)
        ));
        let mut guard = f.credentials.write().expect("current registry");
        assert!(matches!(
            f.service
                .observe_authenticated(&f.actor, f.request.clone(), f.update.clone()),
            Err(NativeAttentionError::AccessDenied)
        ));
        guard
            .revoke("credential")
            .expect("actual current credential revocation");
        drop(guard);
        assert!(matches!(
            f.service
                .observe_authenticated(&f.actor, f.request.clone(), f.update.clone()),
            Err(NativeAttentionError::AccessDenied)
        ));
    }
    #[test]
    fn writer_rechecks_current_registry_after_enqueue_without_caller_held_lock() {
        let f = fixture();
        let (entered, receive) = mpsc::sync_channel(1);
        let (resume, wait) = mpsc::sync_channel(1);
        *f.policy.gate.lock().expect("gate") = Some(Gate {
            entered,
            resume: wait,
        });
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                f.service
                    .observe_authenticated(&f.actor, f.request.clone(), f.update.clone())
            });
            receive
                .recv_timeout(Duration::from_secs(5))
                .expect("actual writer callback after service precheck");
            f.credentials
                .write()
                .expect("no caller-held read lock")
                .revoke("credential")
                .expect("revoke actual authenticated credential");
            resume.send(()).expect("resume policy");
            assert!(matches!(
                worker.join().expect("request"),
                Err(NativeAttentionError::Store(StoreError::Attention(
                    AttentionError::AccessDenied
                )))
            ));
        });
        assert!(
            f.store
                .attention_status(f.definition.scope)
                .expect("status")
                .expect("journal")
                .cursor
                .is_none()
        );
    }
    #[test]
    fn actual_capsule_task_binding_and_final_policy_check_are_enforced() {
        let f = fixture();
        let mut wrong = f.update.clone();
        wrong.scope.revision = 2;
        f.policy.admit(&f.service.bind(
            &f.actor,
            f.request.clone(),
            AttentionOperation::Observe {
                update: Box::new(wrong.clone()),
            },
        ));
        assert!(matches!(
            f.service
                .observe_authenticated(&f.actor, f.request.clone(), wrong),
            Err(NativeAttentionError::Store(StoreError::Attention(
                AttentionError::ContextBinding
            )))
        ));
        f.policy.deny_final.store(true, Ordering::SeqCst);
        assert!(matches!(
            f.service
                .observe_authenticated(&f.actor, f.request.clone(), f.update.clone()),
            Err(NativeAttentionError::Store(StoreError::Attention(
                AttentionError::AccessDenied
            )))
        ));
        assert!(f.policy.calls.load(Ordering::SeqCst) >= 3);
        assert!(
            f.store
                .attention_status(f.definition.scope.clone())
                .expect("status")
                .expect("journal")
                .cursor
                .is_none()
        );
        f.policy.deny_final.store(false, Ordering::SeqCst);
        f.service
            .observe_authenticated(&f.actor, f.request.clone(), f.update.clone())
            .expect("denial did not consume command");
    }
    #[test]
    fn writer_rejects_serialized_labels_and_missing_store_issuer() {
        let f = fixture();
        let bound = f.service.bind(
            &f.actor,
            f.request.clone(),
            AttentionOperation::Observe {
                update: Box::new(f.update.clone()),
            },
        );
        assert!(matches!(
            f.store
                .apply_attention_authorized(bound, Arc::new("credential")),
            Err(StoreError::Attention(AttentionError::AccessDenied))
        ));
        let path = f._dir.path().join("no-issuer.sqlite");
        let unprotected = Store::open(path).expect("default deny Store");
        assert!(matches!(
            NativeAttentionService::configured(unprotected, f.definition, Some(f.issuer)),
            Err(NativeAttentionError::Store(StoreError::Attention(
                AttentionError::AccessDenied
            )))
        ));
    }
}
