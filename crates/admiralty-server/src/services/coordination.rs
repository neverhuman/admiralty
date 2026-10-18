//! Authorized pending observations, scoped application ACKs and historical replay.
//!
//! This prototype does not authenticate callers, run a delivery loop, or confer
//! authority. A trusted service caller must inject current actor/scope grants.
use admiralty_core::{
    CoordinationMessage, DomainEvent, MessageId, PrincipalId, ProjectId, SourceIdentity,
    coordination::mailbox::{BoundedMailbox, DeliveryOutcome},
};
use admiralty_store::inbox::{InboxCursor, InboxScope, PendingInboxPage, validate_request};
use admiralty_store::{AppendReceipt, Cursor, Store, StoreError};
use std::collections::BTreeSet;

/// Bound both underlying durable events examined and returned message allocation.
pub const MAX_COORDINATION_SCAN_EVENTS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoordinationReadScope {
    pub project_id: ProjectId,
    pub recipient: PrincipalId,
    pub context_epoch: String,
}

/// Policy is evaluated on every page before the Store is queried. Implementors
/// must validate actual actor grants and epoch/scope, not merely compare labels.
pub trait CoordinationReadAuthorizer {
    fn authorize(&self, actor: &PrincipalId, scope: &CoordinationReadScope) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinationAction {
    ReadPending,
    AcknowledgeApplication,
}

/// Trusted injection boundary, not authentication inferred from request labels.
/// Implementors must check current actor grants, exact recipient/epoch membership
/// and, for an ACK, the supplied producer identity's binding to that actor.
/// Read access never implies permission to acknowledge application receipt.
pub trait CoordinationAuthorizer {
    fn authorize(
        &self,
        actor: &PrincipalId,
        action: CoordinationAction,
        scope: &InboxScope,
        message: Option<&MessageId>,
        source: Option<&SourceIdentity>,
    ) -> bool;
}

pub const MAX_COORDINATION_CREDENTIALS: usize = 128;
pub const MAX_COORDINATION_GRANTS_PER_CREDENTIAL: usize = 64;
pub const MAX_COORDINATION_GRANTS: usize = 1024;
pub const MAX_COORDINATION_TOKEN_BYTES: usize = 4096;
const MIN_COORDINATION_TOKEN_BYTES: usize = 32;
const MAX_COORDINATION_CREDENTIAL_ID_BYTES: usize = 128;

/// Exact service-local grant. Read grants carry no message or producer source;
/// ACK grants require both, so a read grant can never imply write authority.
#[derive(Clone, PartialEq, Eq)]
pub struct CoordinationCredentialGrant {
    pub scope: InboxScope,
    pub action: CoordinationAction,
    pub message_id: Option<MessageId>,
    pub source: Option<SourceIdentity>,
}

/// Configuration input for the in-memory credential authority. `token` is an
/// opaque secret supplied by the configured credential source; this type does
/// not generate, transform, serialize, or format it.
pub struct CoordinationCredential {
    pub credential_id: String,
    pub actor: PrincipalId,
    pub grants: Vec<CoordinationCredentialGrant>,
    token: String,
}

impl CoordinationCredential {
    pub fn configured(
        credential_id: String,
        token: String,
        actor: PrincipalId,
        grants: Vec<CoordinationCredentialGrant>,
    ) -> Self {
        Self {
            credential_id,
            actor,
            grants,
            token,
        }
    }
}

struct CredentialRecord {
    credential_id: String,
    token: String,
    actor: PrincipalId,
    grants: Vec<CoordinationCredentialGrant>,
    revoked: bool,
}

/// Bounded adapter from configured opaque credentials to exact coordination
/// grants. Keep this object in trusted server state; it deliberately has no
/// `Debug` implementation because it retains bearer credentials.
pub struct CoordinationCredentialAuthority {
    credentials: Vec<CredentialRecord>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinationCredentialError {
    InvalidInput,
    Unauthorized,
    Capacity,
}

/// Constructed only by successful token authentication. Request-supplied
/// principal labels cannot create this identity.
#[derive(Clone, PartialEq, Eq)]
pub struct AuthenticatedCoordinationActor {
    credential_id: String,
    principal: PrincipalId,
}

impl AuthenticatedCoordinationActor {
    pub fn principal(&self) -> &PrincipalId {
        &self.principal
    }
}

fn valid_opaque_token(token: &str) -> bool {
    (MIN_COORDINATION_TOKEN_BYTES..=MAX_COORDINATION_TOKEN_BYTES).contains(&token.len())
        && !token.chars().any(char::is_control)
        && !token.trim().is_empty()
}

fn valid_credential_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_COORDINATION_CREDENTIAL_ID_BYTES
        && id.trim() == id
        && !id.chars().any(char::is_control)
}

impl CoordinationCredentialAuthority {
    pub fn new(
        credentials: Vec<CoordinationCredential>,
    ) -> Result<Self, CoordinationCredentialError> {
        if credentials.len() > MAX_COORDINATION_CREDENTIALS {
            return Err(CoordinationCredentialError::Capacity);
        }
        let mut ids = BTreeSet::new();
        let mut tokens = BTreeSet::new();
        let mut grant_count = 0_usize;
        let mut records = Vec::with_capacity(credentials.len());
        for credential in credentials {
            if !valid_credential_id(&credential.credential_id)
                || !valid_opaque_token(&credential.token)
                || credential.grants.is_empty()
                || credential.grants.len() > MAX_COORDINATION_GRANTS_PER_CREDENTIAL
                || !ids.insert(credential.credential_id.clone())
                || !tokens.insert(credential.token.clone())
            {
                return Err(CoordinationCredentialError::InvalidInput);
            }
            grant_count = grant_count
                .checked_add(credential.grants.len())
                .ok_or(CoordinationCredentialError::Capacity)?;
            if grant_count > MAX_COORDINATION_GRANTS {
                return Err(CoordinationCredentialError::Capacity);
            }
            for grant in &credential.grants {
                grant
                    .scope
                    .validate()
                    .map_err(|_| CoordinationCredentialError::InvalidInput)?;
                match grant.action {
                    CoordinationAction::ReadPending
                        if grant.message_id.is_none() && grant.source.is_none() => {}
                    CoordinationAction::AcknowledgeApplication
                        if grant.message_id.is_some()
                            && grant.source.as_ref().is_some_and(valid_source_identity) => {}
                    _ => return Err(CoordinationCredentialError::InvalidInput),
                }
            }
            records.push(CredentialRecord {
                credential_id: credential.credential_id,
                token: credential.token,
                actor: credential.actor,
                grants: credential.grants,
                revoked: false,
            });
        }
        Ok(Self {
            credentials: records,
        })
    }

    /// Resolve a configured opaque token to its server-configured principal.
    /// The token itself is never copied into the returned actor or an error.
    pub fn authenticate(
        &self,
        token: &str,
    ) -> Result<AuthenticatedCoordinationActor, CoordinationCredentialError> {
        if !valid_opaque_token(token) {
            return Err(CoordinationCredentialError::InvalidInput);
        }
        self.credentials
            .iter()
            .find(|credential| !credential.revoked && credential.token == token)
            .map(|credential| AuthenticatedCoordinationActor {
                credential_id: credential.credential_id.clone(),
                principal: credential.actor.clone(),
            })
            .ok_or(CoordinationCredentialError::Unauthorized)
    }

    /// Revocation takes effect on the next service authorization check,
    /// including for already-authenticated actor handles.
    pub fn revoke(&mut self, credential_id: &str) -> Result<(), CoordinationCredentialError> {
        let credential = self
            .credentials
            .iter_mut()
            .find(|credential| credential.credential_id == credential_id)
            .ok_or(CoordinationCredentialError::Unauthorized)?;
        credential.revoked = true;
        Ok(())
    }

    fn permits(
        &self,
        authenticated: &AuthenticatedCoordinationActor,
        actor: &PrincipalId,
        action: CoordinationAction,
        scope: &InboxScope,
        message: Option<&MessageId>,
        source: Option<&SourceIdentity>,
    ) -> bool {
        if actor != &authenticated.principal {
            return false;
        }
        self.credentials
            .iter()
            .find(|credential| {
                credential.credential_id == authenticated.credential_id
                    && !credential.revoked
                    && credential.actor == authenticated.principal
            })
            .is_some_and(|credential| {
                credential.grants.iter().any(|grant| {
                    grant.action == action
                        && grant.scope == *scope
                        && grant.message_id.as_ref() == message
                        && grant.source.as_ref() == source
                })
            })
    }
}

fn valid_source_identity(source: &SourceIdentity) -> bool {
    source.producer_id.len() <= 256
        && source.incarnation.len() <= 256
        && source.sequence.len() <= 19
        && !source.producer_id.trim().is_empty()
        && !source.incarnation.trim().is_empty()
        && !source.producer_id.chars().any(char::is_control)
        && !source.incarnation.chars().any(char::is_control)
        && source.validate().is_ok()
}

struct AuthenticatedAuthorizer<'a> {
    credentials: &'a CoordinationCredentialAuthority,
    actor: &'a AuthenticatedCoordinationActor,
}

impl CoordinationAuthorizer for AuthenticatedAuthorizer<'_> {
    fn authorize(
        &self,
        actor: &PrincipalId,
        action: CoordinationAction,
        scope: &InboxScope,
        message: Option<&MessageId>,
        source: Option<&SourceIdentity>,
    ) -> bool {
        self.credentials
            .permits(self.actor, actor, action, scope, message, source)
    }
}

impl CoordinationReadAuthorizer for AuthenticatedAuthorizer<'_> {
    fn authorize(&self, actor: &PrincipalId, scope: &CoordinationReadScope) -> bool {
        let inbox_scope = InboxScope {
            project_id: scope.project_id.clone(),
            recipient: scope.recipient.clone(),
            context_epoch: scope.context_epoch.clone(),
        };
        self.credentials.permits(
            self.actor,
            actor,
            CoordinationAction::ReadPending,
            &inbox_scope,
            None,
            None,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedMessageBatch {
    /// Advances over the bounded scanned durable page, even if none match.
    pub cursor: Cursor,
    pub messages: Vec<CoordinationMessage>,
    pub scanned_events: usize,
}

pub struct CoordinationService<'a> {
    store: &'a Store,
}

impl<'a> CoordinationService<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Low-level trusted in-process storage seam; does not authenticate a sender.
    pub fn enqueue_durable(
        &self,
        source: SourceIdentity,
        message: CoordinationMessage,
        observed_utc: String,
    ) -> Result<AppendReceipt, StoreError> {
        self.store
            .append(source, DomainEvent::MessageQueued { message }, observed_utc)
    }

    /// Legacy trusted in-process append seam, without project authorization.
    /// Untrusted callers require `acknowledge_application` and a real policy.
    pub fn acknowledge_durable(
        &self,
        source: SourceIdentity,
        message_id: MessageId,
        recipient: PrincipalId,
        context_epoch: String,
        observed_utc: String,
    ) -> Result<AppendReceipt, StoreError> {
        self.store.append(
            source,
            DomainEvent::MessageAcknowledged {
                message_id,
                recipient,
                context_epoch,
            },
            observed_utc,
        )
    }

    pub fn route_to_mailbox(
        mailbox: &mut BoundedMailbox,
        seq: u64,
        message: CoordinationMessage,
    ) -> DeliveryOutcome {
        mailbox.route_sequenced(seq, message)
    }

    /// Observe currently unacknowledged envelopes in one exact scope. Reads do
    /// not ACK, mark processing progress, or prove delivery. A continuation is
    /// lexical live pagination only; reconnect/new passes start without one.
    pub fn pending_messages(
        &self,
        actor: &PrincipalId,
        authorizer: &impl CoordinationAuthorizer,
        scope: InboxScope,
        cursor: Option<InboxCursor>,
        limit: usize,
    ) -> Result<PendingInboxPage, StoreError> {
        validate_request(&scope, cursor.as_ref(), limit)?;
        if !authorizer.authorize(actor, CoordinationAction::ReadPending, &scope, None, None) {
            return Err(StoreError::Invalid(
                "coordination pending read is not authorized for this actor and scope".into(),
            ));
        }
        self.store.pending_messages(scope, cursor, limit)
    }

    /// Authenticate first, then authorize the exact inbox scope against the
    /// credential's current grants and revocation state before reading Store.
    pub fn pending_messages_authenticated(
        &self,
        actor: &AuthenticatedCoordinationActor,
        credentials: &CoordinationCredentialAuthority,
        scope: InboxScope,
        cursor: Option<InboxCursor>,
        limit: usize,
    ) -> Result<PendingInboxPage, StoreError> {
        let authorizer = AuthenticatedAuthorizer { credentials, actor };
        self.pending_messages(actor.principal(), &authorizer, scope, cursor, limit)
    }

    /// Record only the recipient's application acknowledgment. The Store writer
    /// rechecks exact message project/recipient/epoch even on source replay.
    /// This is neither a transport/provider ACK nor task acceptance.
    pub fn acknowledge_application(
        &self,
        actor: &PrincipalId,
        authorizer: &impl CoordinationAuthorizer,
        source: SourceIdentity,
        scope: InboxScope,
        message_id: MessageId,
        observed_utc: String,
    ) -> Result<AppendReceipt, StoreError> {
        scope.validate()?;
        // Bound source strings before parsing or presenting them to policy.
        // These are the existing Store append admission limits, not authority.
        if source.producer_id.len() > 256
            || source.incarnation.len() > 256
            || source.sequence.len() > 19
            || source.producer_id.chars().any(char::is_control)
            || source.incarnation.chars().any(char::is_control)
            || observed_utc.is_empty()
            || observed_utc.len() > 128
            || observed_utc.chars().any(char::is_control)
        {
            return Err(StoreError::Invalid(
                "producer identity or observation timestamp is outside bounds".into(),
            ));
        }
        let sequence = source.validate().map_err(StoreError::Invalid)?;
        i64::try_from(sequence).map_err(|_| {
            StoreError::Invalid("source sequence exceeds SQLite signed integer range".into())
        })?;
        if !authorizer.authorize(
            actor,
            CoordinationAction::AcknowledgeApplication,
            &scope,
            Some(&message_id),
            Some(&source),
        ) {
            return Err(StoreError::Invalid(
                "coordination application ACK is not authorized for this actor and scope".into(),
            ));
        }
        self.store
            .acknowledge_scoped_message(source, scope, message_id, observed_utc)
    }

    /// Token-authenticated wrapper for the recipient application ACK path.
    /// The credential grant must bind the exact message and complete source
    /// identity; read grants cannot authorize this operation.
    pub fn acknowledge_application_authenticated(
        &self,
        actor: &AuthenticatedCoordinationActor,
        credentials: &CoordinationCredentialAuthority,
        source: SourceIdentity,
        scope: InboxScope,
        message_id: MessageId,
        observed_utc: String,
    ) -> Result<AppendReceipt, StoreError> {
        let authorizer = AuthenticatedAuthorizer { credentials, actor };
        self.acknowledge_application(
            actor.principal(),
            &authorizer,
            source,
            scope,
            message_id,
            observed_utc,
        )
    }

    /// Replay matching queue observations without cloning the global projection.
    /// An ACKed message can still appear here: this is not a pending-delivery
    /// list, application ACK, delivery attempt or subscription activation.
    /// Recovery cursor validation remains atomic inside the Store writer.
    pub fn queued_messages_since(
        &self,
        actor: &PrincipalId,
        authorizer: &impl CoordinationReadAuthorizer,
        scope: &CoordinationReadScope,
        cursor: &Cursor,
        scan_limit: usize,
    ) -> Result<QueuedMessageBatch, StoreError> {
        if scan_limit == 0
            || scan_limit > MAX_COORDINATION_SCAN_EVENTS
            || scope.context_epoch.len() > 16_384
            || scope.context_epoch.trim().is_empty()
            || cursor.recovery_generation.is_empty()
            || cursor.recovery_generation.len() > 64
            || cursor.event_seq.len() > 19
            || !cursor
                .event_seq
                .parse::<i64>()
                .is_ok_and(|seq| seq >= 0 && seq.to_string() == cursor.event_seq)
        {
            return Err(StoreError::Invalid(
                "coordination page requires bounded scope, cursor and scan limit".into(),
            ));
        }
        if !authorizer.authorize(actor, scope) {
            return Err(StoreError::Invalid(
                "coordination read is not authorized for this actor and scope".into(),
            ));
        }
        let batch = self.store.events_since(cursor, scan_limit)?;
        let scanned_events = batch.events.len();
        let messages = batch
            .events
            .into_iter()
            .filter_map(|record| match record.event {
                DomainEvent::MessageQueued { message }
                    if message.project_id == scope.project_id
                        && message.recipient == scope.recipient
                        && message.context_epoch == scope.context_epoch =>
                {
                    Some(message)
                }
                _ => None,
            })
            .collect();
        Ok(QueuedMessageBatch {
            cursor: batch.cursor,
            messages,
            scanned_events,
        })
    }

    /// Token-authenticated wrapper for bounded historical reads.
    pub fn queued_messages_since_authenticated(
        &self,
        actor: &AuthenticatedCoordinationActor,
        credentials: &CoordinationCredentialAuthority,
        scope: &CoordinationReadScope,
        cursor: &Cursor,
        scan_limit: usize,
    ) -> Result<QueuedMessageBatch, StoreError> {
        let authorizer = AuthenticatedAuthorizer { credentials, actor };
        self.queued_messages_since(actor.principal(), &authorizer, scope, cursor, scan_limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use admiralty_core::{AcceptanceCriterion, CheckSpec, MessagePayload, TaskContract, TaskId};
    use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

    const TOKEN: &str = "configured-opaque-coordination-token-32-bytes";

    fn scope() -> InboxScope {
        InboxScope {
            project_id: ProjectId::new("project-a").expect("project ID"),
            recipient: PrincipalId::new("worker-a").expect("recipient ID"),
            context_epoch: "epoch-a".into(),
        }
    }

    fn source(sequence: &str) -> SourceIdentity {
        SourceIdentity {
            producer_id: "worker-producer".into(),
            incarnation: "worker-incarnation".into(),
            sequence: sequence.into(),
        }
    }

    fn message() -> CoordinationMessage {
        CoordinationMessage {
            schema_version: 1,
            message_id: MessageId::new("message-a").expect("message ID"),
            project_id: scope().project_id,
            sender: PrincipalId::new("captain").expect("sender ID"),
            recipient: scope().recipient,
            causation_id: None,
            context_epoch: scope().context_epoch,
            authority_ref: "grant:task-a".into(),
            payload: MessagePayload::Question {
                task_id: TaskId::new("task-a").expect("task ID"),
                requirement: "Review the bounded change".into(),
                question: "Does this match the exact scope?".into(),
                options: vec!["yes".into(), "no".into()],
            },
        }
    }

    fn task_contract() -> TaskContract {
        TaskContract {
            schema_version: 1,
            project_id: scope().project_id,
            root_id: admiralty_core::RootId::new("root-a").expect("root ID"),
            task_id: TaskId::new("task-a").expect("task ID"),
            revision: 1,
            title: "Coordination auth fixture".into(),
            objective: "Exercise authenticated inbox operations".into(),
            non_goals: vec![],
            owned_paths: vec!["src".into()],
            forbidden_paths: vec![],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "C1".into(),
                text: "The fixture task is available".into(),
                check_ids: vec!["check".into()],
            }],
            checks: vec![CheckSpec {
                id: "check".into(),
                program: "true".into(),
                args: vec![],
                cwd: ".".into(),
                timeout_secs: 1,
            }],
            authority_ref: "grant:task-a".into(),
            max_attempts: 1,
            simulated: false,
        }
    }

    fn credential(grants: Vec<CoordinationCredentialGrant>) -> CoordinationCredential {
        CoordinationCredential::configured(
            "worker-credential-1".into(),
            TOKEN.into(),
            scope().recipient,
            grants,
        )
    }

    fn read_grant(scope: InboxScope) -> CoordinationCredentialGrant {
        CoordinationCredentialGrant {
            scope,
            action: CoordinationAction::ReadPending,
            message_id: None,
            source: None,
        }
    }

    fn ack_grant(scope: InboxScope, source: SourceIdentity) -> CoordinationCredentialGrant {
        CoordinationCredentialGrant {
            scope,
            action: CoordinationAction::AcknowledgeApplication,
            message_id: Some(message().message_id),
            source: Some(source),
        }
    }

    fn store_path() -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "admiralty-coordination-auth-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&directory).expect("create private test directory");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("restrict test directory permissions");
        directory.join("store.sqlite3")
    }

    fn cleanup_store(path: &PathBuf) {
        let _ = fs::remove_file(path);
        let mut lock = path.as_os_str().to_os_string();
        lock.push(".writer.lock");
        let _ = fs::remove_file(PathBuf::from(lock));
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir(parent);
        }
    }

    #[test]
    fn authenticated_token_binds_actor_scope_epoch_action_and_exact_source() {
        let path = store_path();
        {
            let store = Store::open(&path).expect("open test store");
            let service = CoordinationService::new(&store);
            let queued = message();
            store
                .append(
                    SourceIdentity {
                        producer_id: "server".into(),
                        incarnation: "server-run".into(),
                        sequence: "1".into(),
                    },
                    DomainEvent::TaskSubmitted {
                        contract: task_contract(),
                    },
                    "2030-01-01T00:00:00Z".into(),
                )
                .expect("trusted in-process task fixture append");
            service
                .enqueue_durable(
                    SourceIdentity {
                        producer_id: "server".into(),
                        incarnation: "server-run".into(),
                        sequence: "2".into(),
                    },
                    queued.clone(),
                    "2030-01-01T00:00:01Z".into(),
                )
                .expect("trusted in-process message fixture append");

            let ack_source = source("1");
            let credentials = CoordinationCredentialAuthority::new(vec![credential(vec![
                read_grant(scope()),
                ack_grant(scope(), ack_source.clone()),
            ])])
            .expect("valid bounded grants");
            let actor = credentials
                .authenticate(TOKEN)
                .expect("configured token authenticates");
            assert_eq!(actor.principal(), &scope().recipient);

            let page = service
                .pending_messages_authenticated(&actor, &credentials, scope(), None, 8)
                .expect("exact read grant reaches Store");
            assert_eq!(page.messages, vec![queued]);

            let forged_actor = PrincipalId::new("captain").expect("valid forged label");
            let policy = AuthenticatedAuthorizer {
                credentials: &credentials,
                actor: &actor,
            };
            assert!(!CoordinationAuthorizer::authorize(
                &policy,
                &forged_actor,
                CoordinationAction::ReadPending,
                &scope(),
                None,
                None
            ));

            let mut wrong_project = scope();
            wrong_project.project_id = ProjectId::new("project-b").expect("foreign project");
            assert!(
                service
                    .pending_messages_authenticated(&actor, &credentials, wrong_project, None, 8)
                    .is_err()
            );
            let mut wrong_recipient = scope();
            wrong_recipient.recipient = PrincipalId::new("worker-b").expect("foreign recipient");
            assert!(
                service
                    .pending_messages_authenticated(&actor, &credentials, wrong_recipient, None, 8)
                    .is_err()
            );
            let mut wrong_epoch = scope();
            wrong_epoch.context_epoch = "epoch-b".into();
            assert!(
                service
                    .pending_messages_authenticated(&actor, &credentials, wrong_epoch, None, 8)
                    .is_err()
            );
            assert!(
                service
                    .acknowledge_application_authenticated(
                        &actor,
                        &credentials,
                        ack_source.clone(),
                        scope(),
                        message().message_id.clone(),
                        "2030-01-01T00:00:01Z".into(),
                    )
                    .is_ok()
            );

            let read_only =
                CoordinationCredentialAuthority::new(vec![credential(vec![read_grant(scope())])])
                    .expect("valid read-only credential");
            let read_only_actor = read_only
                .authenticate(TOKEN)
                .expect("read token authenticates");
            assert!(
                service
                    .acknowledge_application_authenticated(
                        &read_only_actor,
                        &read_only,
                        source("1"),
                        scope(),
                        message().message_id.clone(),
                        "2030-01-01T00:00:02Z".into(),
                    )
                    .is_err()
            );

            let wrong_source = source("2");
            assert!(
                service
                    .acknowledge_application_authenticated(
                        &actor,
                        &credentials,
                        wrong_source,
                        scope(),
                        message().message_id.clone(),
                        "2030-01-01T00:00:03Z".into(),
                    )
                    .is_err()
            );
        }
        cleanup_store(&path);
    }

    #[test]
    fn revocation_and_bounded_credential_inputs_fail_closed() {
        let mut credentials =
            CoordinationCredentialAuthority::new(vec![credential(vec![read_grant(scope())])])
                .expect("valid configured credential");
        let actor = credentials
            .authenticate(TOKEN)
            .expect("configured token authenticates");
        credentials
            .revoke("worker-credential-1")
            .expect("revoke exact credential");
        assert!(matches!(
            credentials.authenticate(TOKEN),
            Err(CoordinationCredentialError::Unauthorized)
        ));
        assert!(!credentials.permits(
            &actor,
            actor.principal(),
            CoordinationAction::ReadPending,
            &scope(),
            None,
            None
        ));
        assert!(matches!(
            CoordinationCredentialAuthority::new(vec![credential(vec![read_grant({
                let mut invalid = scope();
                invalid.context_epoch =
                    "e".repeat(admiralty_store::inbox::MAX_CONTEXT_EPOCH_BYTES + 1);
                invalid
            })])]),
            Err(CoordinationCredentialError::InvalidInput)
        ));
        assert!(matches!(
            CoordinationCredentialAuthority::new(vec![credential(vec![
                CoordinationCredentialGrant {
                    scope: scope(),
                    action: CoordinationAction::AcknowledgeApplication,
                    message_id: Some(message().message_id),
                    source: None,
                }
            ])]),
            Err(CoordinationCredentialError::InvalidInput)
        ));
        let oversized = "t".repeat(MAX_COORDINATION_TOKEN_BYTES + 1);
        assert!(matches!(
            credentials.authenticate(&oversized),
            Err(CoordinationCredentialError::InvalidInput)
        ));
        assert!(matches!(
            CoordinationCredentialAuthority::new(
                (0..=MAX_COORDINATION_CREDENTIALS)
                    .map(|index| CoordinationCredential::configured(
                        format!("worker-{index}"),
                        format!("{index:04}-{}", "x".repeat(32)),
                        scope().recipient.clone(),
                        vec![read_grant(scope())],
                    ))
                    .collect()
            ),
            Err(CoordinationCredentialError::Capacity)
        ));
    }
}
