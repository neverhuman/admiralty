//! Authenticated, scoped attention recording at a caller-owned safe turn.
//!
//! Trusted startup installs the immutable journal definition and current action
//! policy. An inbox read grant alone never authorizes a mutation. This adapter
//! records observations/wake outbox intents only: it does not send provider
//! input, acknowledge delivery, advance context baselines or grant authority.
//! A wake's `request_context` is the caller's safe-turn context pin. The Store
//! separately retains observed evidence pins in `delivery.refs`; this adapter
//! does not prove those references were compiled into the request context.
//! Policy checks occur before enqueueing the Store command and are not
//! linearizable with its commit. Production activation requires writer-bound
//! current-policy authorization and batch-to-context compilation validation.
use crate::services::coordination::{
    AuthenticatedCoordinationActor, CoordinationCredentialAuthority, CoordinationService,
};
use admiralty_core::{
    PrincipalId, SourceIdentity, content_digest,
    coordination::context::{ProtectedTaskFacts, RecipientRole, RoleFacts, decode_role_capsule},
    governor::{AttentionUpdate, Recipient, Role},
};
use admiralty_store::{
    Store, StoreError,
    attention::{
        AttentionCommand, AttentionCursor, AttentionDefinition, AttentionOperation,
        AttentionResult, AttentionScope,
    },
    context_renders::{ContextNamespace, ContextScope, RenderRef},
    inbox::InboxScope,
    outbox::OutboxIntent,
};
use serde::Serialize;
use std::sync::Arc;

const MAX_REQUEST_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AttentionContextPin {
    pub scope: ContextScope,
    pub render: RenderRef,
    /// Independently retained compiler binding, checked against the real record.
    pub facts_digest: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AttentionRequest {
    pub command_id: String,
    pub expected_prior: Option<AttentionCursor>,
    /// A trusted policy tick, not a wall-clock value invented by this adapter.
    pub logical_tick: Option<u64>,
    pub context: AttentionContextPin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeAttentionAction {
    Observe,
    TakeAtSafeTurn,
}

/// Complete bounded request binding presented to an independently installed
/// policy. Implementors must check current actor/root/role/epoch membership,
/// source provenance, exact context/evidence custody and the whole request digest.
/// A true result is recording permission only, never execution/delivery authority.
pub struct NativeAttentionBinding<'a> {
    pub action: NativeAttentionAction,
    pub actor: &'a PrincipalId,
    pub scope: &'a AttentionScope,
    pub config_digest: &'a str,
    pub recipient: &'a Recipient,
    pub context: &'a AttentionContextPin,
    pub source: Option<&'a SourceIdentity>,
    pub evidence_ref: &'a str,
    pub request_digest: &'a str,
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

pub struct NativeAttentionService {
    store: Store,
    scope: AttentionScope,
    config_digest: String,
    policy: Option<Arc<dyn NativeAttentionPolicy>>,
}

impl NativeAttentionService {
    /// Trusted startup only: registers a bounded immutable policy journal.
    /// HTTP/request data must never install this policy or replace its definition.
    /// With no action policy installed, both mutation methods fail closed.
    pub fn configured(
        store: Store,
        definition: AttentionDefinition,
        policy: Option<Arc<dyn NativeAttentionPolicy>>,
    ) -> Result<Self, NativeAttentionError> {
        admiralty_core::limits::bounded_json_size(
            &definition,
            admiralty_store::attention::MAX_DEFINITION_BYTES,
        )
        .map_err(|_| NativeAttentionError::InvalidBinding)?;
        let registration = store
            .register_attention(definition.clone())
            .map_err(NativeAttentionError::Store)?;
        Ok(Self {
            store,
            scope: definition.scope,
            config_digest: registration.config_digest,
            policy,
        })
    }

    /// Record a source update only after independent recording permission and
    /// current authenticated inbox membership for the exact recipient/epoch.
    pub fn observe_authenticated(
        &self,
        actor: &AuthenticatedCoordinationActor,
        credentials: &CoordinationCredentialAuthority,
        request: AttentionRequest,
        update: AttentionUpdate,
    ) -> Result<AttentionResult, NativeAttentionError> {
        let digest = self.scoped_digest(&(&request, &update))?;
        update
            .validate()
            .map_err(|_| NativeAttentionError::InvalidBinding)?;
        if update.scope.project_id != self.scope.project_id
            || update.evidence_ref != request.context.render.record_digest
        {
            return Err(NativeAttentionError::InvalidBinding);
        }
        let facts = self.authorize(
            actor,
            credentials,
            NativeAttentionAction::Observe,
            &request.context,
            &update.recipient,
            Some(&update.source),
            &digest,
        )?;
        let matches = |task: &ProtectedTaskFacts| {
            task.contract.project_id == update.scope.project_id
                && task.contract.root_id == self.scope.root_id
                && task.contract.task_id == update.scope.task_id
                && task.contract.revision == update.scope.revision
                && task.contract.authority_ref == update.authority_ref
        };
        let in_context = match &facts {
            RoleFacts::Decision(facts) => matches(&facts.task),
            RoleFacts::Handoff(facts) => matches(&facts.task),
            RoleFacts::ProjectExceptions(facts) => facts.exceptions.iter().any(matches),
            RoleFacts::FleetExceptions(facts) => facts
                .projects
                .iter()
                .any(|p| p.exceptions.iter().any(matches)),
        };
        if !in_context {
            return Err(NativeAttentionError::InvalidBinding);
        }
        self.apply(
            request,
            AttentionOperation::Observe {
                update: Box::new(update),
            },
        )
    }

    /// Called by the authenticated recipient at an independently authorized safe
    /// turn. `request_context` identifies that request's actual stored capsule;
    /// it does not establish coverage of the separately stored `delivery.refs`.
    /// A recorded outbox intent remains undelivered and unacknowledged.
    #[allow(clippy::too_many_arguments)]
    pub fn take_at_safe_turn(
        &self,
        actor: &AuthenticatedCoordinationActor,
        credentials: &CoordinationCredentialAuthority,
        request: AttentionRequest,
        recipient: Recipient,
        idempotency_key: String,
        not_before: u64,
    ) -> Result<AttentionResult, NativeAttentionError> {
        let digest = self.scoped_digest(&(&request, &recipient, &idempotency_key, not_before))?;
        self.authorize(
            actor,
            credentials,
            NativeAttentionAction::TakeAtSafeTurn,
            &request.context,
            &recipient,
            None,
            &digest,
        )?;
        let payload = serde_json::json!({
            "kind":"native_attention_context", "request_context":request.context,
            "request_digest":digest, "delivered":false,
            "application_acknowledged":false, "authority_granted":false,
        });
        self.apply(
            request,
            AttentionOperation::TakeDelivery {
                recipient,
                intent: OutboxIntent {
                    idempotency_key,
                    payload,
                    not_before,
                },
            },
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn authorize(
        &self,
        actor: &AuthenticatedCoordinationActor,
        credentials: &CoordinationCredentialAuthority,
        action: NativeAttentionAction,
        context: &AttentionContextPin,
        recipient: &Recipient,
        source: Option<&SourceIdentity>,
        digest: &str,
    ) -> Result<RoleFacts, NativeAttentionError> {
        let policy = self
            .policy
            .as_ref()
            .ok_or(NativeAttentionError::AccessDenied)?;
        let role = match recipient.role {
            Role::TaskWorker => RecipientRole::Worker,
            Role::Captain => RecipientRole::Commodore,
            Role::SuperCaptain => RecipientRole::Admiral,
        };
        if actor.principal() != &recipient.principal_id
            || context.scope.recipient != recipient.principal_id
            || context.scope.role != role
            || !self.scope.recipients.contains(recipient)
        {
            return Err(NativeAttentionError::AccessDenied);
        }
        let mut roots = match &context.scope.namespace {
            ContextNamespace::Project {
                project_id,
                root_id,
            } => vec![(project_id, root_id)],
            ContextNamespace::Fleet { project_roots, .. } if project_roots.len() <= 16 => {
                project_roots
                    .iter()
                    .map(|p| (&p.project_id, &p.root_id))
                    .collect()
            }
            _ => return Err(NativeAttentionError::InvalidBinding),
        };
        if !roots.contains(&(&self.scope.project_id, &self.scope.root_id)) {
            return Err(NativeAttentionError::InvalidBinding);
        }
        roots.sort();
        if roots.windows(2).any(|w| w[0] == w[1]) {
            return Err(NativeAttentionError::InvalidBinding);
        }
        // Authenticate against CURRENT grants before any context/journal read.
        // Fleet contexts require independent inbox membership for every project;
        // the installed policy additionally binds all root and role membership.
        for (project_id, _) in roots {
            CoordinationService::new(&self.store)
                .pending_messages_authenticated(
                    actor,
                    credentials,
                    InboxScope {
                        project_id: project_id.clone(),
                        recipient: actor.principal().clone(),
                        context_epoch: self.scope.epoch.clone(),
                    },
                    None,
                    1,
                )
                .map_err(|_| NativeAttentionError::AccessDenied)?;
        }
        let binding = NativeAttentionBinding {
            action,
            actor: actor.principal(),
            scope: &self.scope,
            config_digest: &self.config_digest,
            recipient,
            context,
            source,
            evidence_ref: &context.render.record_digest,
            request_digest: digest,
        };
        if !policy.authorize_current(&binding) {
            return Err(NativeAttentionError::AccessDenied);
        }
        let stored = self
            .store
            .context_render(context.scope.clone(), context.render.key.clone())
            .map_err(NativeAttentionError::Store)?
            .ok_or(NativeAttentionError::ContextUnavailable)?;
        if stored.capsule.context_epoch != self.scope.epoch
            || stored.capsule.facts_digest != context.facts_digest
            || content_digest(&stored).map_err(|_| NativeAttentionError::InvalidBinding)?
                != context.render.record_digest
        {
            return Err(NativeAttentionError::ContextUnavailable);
        }
        let facts = decode_role_capsule(
            &stored.capsule.text,
            role,
            &self.scope.epoch,
            &context.facts_digest,
        )
        .map_err(|_| NativeAttentionError::ContextUnavailable)?;
        // Recheck after the read. This is deliberately not a claim that policy
        // revocation is linearized with the later Store command/commit.
        if !policy.authorize_current(&binding) {
            return Err(NativeAttentionError::AccessDenied);
        }
        Ok(facts)
    }

    fn apply(
        &self,
        request: AttentionRequest,
        operation: AttentionOperation,
    ) -> Result<AttentionResult, NativeAttentionError> {
        self.store
            .apply_attention(AttentionCommand {
                scope: self.scope.clone(),
                config_digest: self.config_digest.clone(),
                command_id: request.command_id,
                expected_prior: request.expected_prior,
                logical_tick: request.logical_tick,
                operation,
            })
            .map_err(NativeAttentionError::Store)
    }

    fn scoped_digest(&self, value: &impl Serialize) -> Result<String, NativeAttentionError> {
        request_digest(&(&self.scope, &self.config_digest, value))
    }
}

fn request_digest(value: &impl Serialize) -> Result<String, NativeAttentionError> {
    admiralty_core::limits::bounded_json_size(value, MAX_REQUEST_BYTES)
        .map_err(|_| NativeAttentionError::InvalidBinding)?;
    content_digest(value).map_err(|_| NativeAttentionError::InvalidBinding)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::services::coordination::{
        CoordinationAction, CoordinationCredential, CoordinationCredentialGrant,
    };
    use admiralty_core::{
        AcceptanceCriterion, CheckSpec, PrincipalId, ProjectId, RootId, TaskContract, TaskId,
        coordination::context::{
            BudgetObservation, CheckObservation, FactObservation, ProjectExceptionFacts,
            ProtectedTaskFacts, compile_project_exceptions,
        },
        governor::{BlockState, DeliveryDecision, GovernorConfig, Scope, UpdateKind},
    };
    use admiralty_store::{
        attention::AttentionOutcome,
        context_renders::{RecordContextRender, RenderKey},
    };
    use std::{
        collections::BTreeSet,
        sync::{
            Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    struct ExactPolicy {
        scope: AttentionScope,
        config_digest: String,
        context: AttentionContextPin,
        allowed: Mutex<BTreeSet<String>>,
        revoked: AtomicBool,
        calls: AtomicUsize,
        deny_after_first: AtomicBool,
    }
    impl ExactPolicy {
        fn admit(&self, value: &impl Serialize) {
            self.allowed.lock().expect("policy lock").insert(
                request_digest(&(&self.scope, &self.config_digest, value)).expect("bounded digest"),
            );
        }
    }
    impl NativeAttentionPolicy for ExactPolicy {
        fn authorize_current(&self, b: &NativeAttentionBinding<'_>) -> bool {
            let calls = self.calls.fetch_add(1, Ordering::SeqCst);
            !(self.revoked.load(Ordering::SeqCst)
                || self.deny_after_first.load(Ordering::SeqCst) && calls > 0)
                && b.scope == &self.scope
                && b.config_digest == self.config_digest
                && b.context == &self.context
                && b.actor == &self.context.scope.recipient
                && self.scope.recipients.contains(b.recipient)
                && b.evidence_ref == self.context.render.record_digest
                && match b.action {
                    NativeAttentionAction::Observe => b.source.is_some_and(|s| {
                        s.producer_id == "trusted-source" && s.incarnation == "trusted-boot"
                    }),
                    NativeAttentionAction::TakeAtSafeTurn => b.source.is_none(),
                }
                && self
                    .allowed
                    .lock()
                    .expect("policy lock")
                    .contains(b.request_digest)
        }
    }
    struct Fixture {
        store: Store,
        definition: AttentionDefinition,
        service: NativeAttentionService,
        credentials: CoordinationCredentialAuthority,
        actor: AuthenticatedCoordinationActor,
        policy: Arc<ExactPolicy>,
        request: AttentionRequest,
        update: AttentionUpdate,
        _dir: tempfile::TempDir,
    }
    fn fixture() -> Fixture {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .expect("private actual Store fixture");
        let store = Store::open(dir.path().join("attention.sqlite")).expect("Store");
        let project = ProjectId::new("project").expect("project");
        let root = RootId::new("root").expect("root");
        let principal = PrincipalId::new("commodore").expect("recipient");
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
            objective: "Carry a scoped context reference".into(),
            non_goals: vec!["Never grant authority".into()],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec!["private".into()],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "smoke".into(),
                text: "Scope retained".into(),
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
                next_action: "Inspect the blocker".into(),
            }],
            uncertainty: vec![],
            next_action: "Inspect the blocker".into(),
        };
        let capsule = compile_project_exceptions(&facts, "epoch", 8192).expect("real compiler");
        let context_scope = ContextScope {
            recipient: principal.clone(),
            role: RecipientRole::Commodore,
            namespace: ContextNamespace::Project {
                project_id: project.clone(),
                root_id: root.clone(),
            },
        };
        let recorded = store
            .record_context_render(RecordContextRender {
                scope: context_scope.clone(),
                key: RenderKey {
                    render_id: "render".into(),
                    version: 1,
                },
                expected_epoch: "epoch".into(),
                expected_facts_digest: capsule.facts_digest.clone(),
                capsule: capsule.clone(),
            })
            .expect("actual durable context record");
        let context = AttentionContextPin {
            scope: context_scope,
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
        let policy = Arc::new(ExactPolicy {
            scope: definition.scope.clone(),
            config_digest: content_digest(&definition.config).expect("config digest"),
            context: context.clone(),
            allowed: Mutex::new(BTreeSet::new()),
            revoked: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            deny_after_first: AtomicBool::new(false),
        });
        let credentials =
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
            .expect("actual configured credentials");
        let actor = credentials
            .authenticate("attention-fixture-token-01234567890123456789")
            .expect("authenticate actual credential");
        let service = NativeAttentionService::configured(
            store.clone(),
            definition.clone(),
            Some(policy.clone()),
        )
        .expect("configured service");
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
        policy.admit(&(&request, &update));
        Fixture {
            store,
            definition,
            service,
            credentials,
            actor,
            policy,
            request,
            update,
            _dir: dir,
        }
    }

    #[test]
    fn authenticated_safe_turn_records_exact_context_outbox_once_without_delivery() {
        let f = fixture();
        let observed = f
            .service
            .observe_authenticated(
                &f.actor,
                &f.credentials,
                f.request.clone(),
                f.update.clone(),
            )
            .expect("scoped observe");
        let mut request = f.request.clone();
        request.command_id = "take".into();
        request.expected_prior = Some(observed.receipt.cursor);
        request.logical_tick = Some(2);
        let key = "attention:wake".to_owned();
        f.policy
            .admit(&(&request, &f.update.recipient, &key, 0_u64));
        let result = f
            .service
            .take_at_safe_turn(
                &f.actor,
                &f.credentials,
                request.clone(),
                f.update.recipient.clone(),
                key.clone(),
                0,
            )
            .expect("actual atomic wake");
        assert!(matches!(
            result.receipt.outcome,
            AttentionOutcome::Delivery {
                decision: DeliveryDecision::Wake { .. }
            }
        ));
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
                &f.credentials,
                request,
                f.update.recipient.clone(),
                key.clone(),
                0,
            )
            .expect("lost response replay");
        assert!(replay.replayed);
        assert_eq!(replay.receipt, result.receipt);
        let claim = f
            .store
            .claim_outbox_key(Some(0), "dispatcher".into(), 10, key)
            .expect("claim own durable effect")
            .expect("one effect");
        assert_eq!(
            claim.payload["payload"]["request_context"]["render"]["record_digest"],
            f.request.context.render.record_digest
        );
        assert_eq!(claim.payload["payload"]["delivered"], false);
    }

    #[test]
    fn request_context_does_not_replace_recorded_batch_evidence() {
        let f = fixture();
        let observed = f
            .service
            .observe_authenticated(
                &f.actor,
                &f.credentials,
                f.request.clone(),
                f.update.clone(),
            )
            .expect("observe actual context A");
        let stored = f
            .store
            .context_render(
                f.request.context.scope.clone(),
                f.request.context.render.key.clone(),
            )
            .expect("read exact record")
            .expect("actual context A");
        let context_b = f
            .store
            .record_context_render(RecordContextRender {
                scope: stored.scope.clone(),
                key: RenderKey {
                    render_id: "safe-turn-context".into(),
                    version: 1,
                },
                expected_epoch: stored.capsule.context_epoch.clone(),
                expected_facts_digest: stored.capsule.facts_digest.clone(),
                capsule: stored.capsule,
            })
            .expect("record distinct actual context B");
        let mut request = f.request.clone();
        request.command_id = "take-context-b".into();
        request.expected_prior = Some(observed.receipt.cursor);
        request.logical_tick = Some(2);
        request.context.render = context_b.render;
        let policy = Arc::new(ExactPolicy {
            scope: f.definition.scope.clone(),
            config_digest: f.policy.config_digest.clone(),
            context: request.context.clone(),
            allowed: Mutex::new(BTreeSet::new()),
            revoked: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            deny_after_first: AtomicBool::new(false),
        });
        let key = "attention:context-b".to_owned();
        policy.admit(&(&request, &f.update.recipient, &key, 0_u64));
        let service =
            NativeAttentionService::configured(f.store.clone(), f.definition, Some(policy))
                .expect("same immutable journal, new exact policy");
        let result = service
            .take_at_safe_turn(
                &f.actor,
                &f.credentials,
                request.clone(),
                f.update.recipient,
                key.clone(),
                0,
            )
            .expect("record request B separately from observed evidence A");
        assert!(result.receipt.recorded_wake);
        let claim = f
            .store
            .claim_outbox_key(Some(0), "dispatcher".into(), 10, key)
            .expect("claim own effect")
            .expect("recorded effect");
        let request_pin = &claim.payload["payload"]["request_context"]["render"]["record_digest"];
        let observed_pin = &claim.payload["delivery"]["refs"][0]["evidence_ref"];
        assert_eq!(request_pin, &request.context.render.record_digest);
        assert_eq!(observed_pin, &f.update.evidence_ref);
        assert_ne!(request_pin, observed_pin);
        assert_eq!(claim.payload["payload"]["delivered"], false);
        assert!(!result.receipt.authority_granted);
    }

    #[test]
    fn read_grant_does_not_replace_action_policy_or_current_credential() {
        let mut f = fixture();
        let denied =
            NativeAttentionService::configured(f.store.clone(), f.definition.clone(), None)
                .expect("absent policy");
        assert!(matches!(
            denied.observe_authenticated(
                &f.actor,
                &f.credentials,
                f.request.clone(),
                f.update.clone()
            ),
            Err(NativeAttentionError::AccessDenied)
        ));
        f.credentials
            .revoke("credential")
            .expect("revoke actual credential");
        assert!(matches!(
            f.service.observe_authenticated(
                &f.actor,
                &f.credentials,
                f.request.clone(),
                f.update.clone()
            ),
            Err(NativeAttentionError::AccessDenied)
        ));
        assert!(
            f.store
                .attention_status(f.definition.scope)
                .expect("status")
                .expect("registered")
                .cursor
                .is_none()
        );
    }

    #[test]
    fn current_policy_context_and_protected_task_bindings_fail_closed() {
        let f = fixture();
        let mut request = f.request.clone();
        request.context.facts_digest = content_digest(&"wrong facts").expect("digest");
        f.policy.admit(&(&request, &f.update));
        assert!(
            f.service
                .observe_authenticated(&f.actor, &f.credentials, request, f.update.clone())
                .is_err()
        );
        let mut update = f.update.clone();
        update.scope.revision = 2;
        f.policy.admit(&(&f.request, &update));
        assert!(matches!(
            f.service
                .observe_authenticated(&f.actor, &f.credentials, f.request.clone(), update),
            Err(NativeAttentionError::InvalidBinding)
        ));
        f.policy.revoked.store(true, Ordering::SeqCst);
        assert!(matches!(
            f.service.observe_authenticated(
                &f.actor,
                &f.credentials,
                f.request.clone(),
                f.update.clone()
            ),
            Err(NativeAttentionError::AccessDenied)
        ));
        assert!(
            f.store
                .attention_status(f.definition.scope)
                .expect("status")
                .expect("registered")
                .cursor
                .is_none()
        );
    }

    #[test]
    fn policy_is_rechecked_after_exact_context_read_before_recording() {
        let f = fixture();
        f.policy.deny_after_first.store(true, Ordering::SeqCst);
        assert!(matches!(
            f.service
                .observe_authenticated(&f.actor, &f.credentials, f.request, f.update),
            Err(NativeAttentionError::AccessDenied)
        ));
        assert_eq!(f.policy.calls.load(Ordering::SeqCst), 2);
        assert!(
            f.store
                .attention_status(f.definition.scope)
                .expect("status")
                .expect("registered")
                .cursor
                .is_none()
        );
    }
}
