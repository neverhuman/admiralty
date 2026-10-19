//! Single-owner durable event store. Replay only applies the pure domain reducer.
pub mod artifacts;
pub mod attention;
pub mod context_renders;
pub mod custody;
pub mod inbox;
pub mod intake;
pub mod intake_capture;
pub mod outbox;
pub mod questions;
pub mod workplans;
use admiralty_core::{
    DomainEvent, LifecycleAction, LifecycleCommand, MessageId, ProjectId, Projection,
    SourceIdentity, TaskId, TaskView, TransitionError, content_digest, reduce,
};
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};

const QUEUE_CAPACITY: usize = 64;
const MAX_EVENT_BYTES: usize = 1024 * 1024;
const MAX_QUERY_EVENTS: usize = 1000;
pub type Result<T> = std::result::Result<T, StoreError>;

fn reject_raw_lifecycle(event: &DomainEvent) -> Result<()> {
    if matches!(event, DomainEvent::TaskLifecycle { .. }) {
        return Err(StoreError::Invalid(
            "native lifecycle requires the configured authority command path".into(),
        ));
    }
    Ok(())
}

fn validate_outbox_token_envelope(token: &outbox::ClaimToken) -> Result<()> {
    if [&token.intent_id, &token.fence]
        .iter()
        .any(|v| v.is_empty() || v.len() > 19 || !v.bytes().all(|b| b.is_ascii_digit()))
        || [&token.owner, &token.nonce]
            .iter()
            .any(|v| v.is_empty() || v.len() > 128 || v.chars().any(char::is_control))
    {
        return Err(StoreError::Invalid(
            "outbox claim token is outside bounds".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("event JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Transition(#[from] TransitionError),
    #[error("outbox storage initialization failed: {0}")]
    Outbox(#[from] outbox::OutboxError),
    #[error(transparent)]
    Workplan(#[from] workplans::WorkplanStoreError),
    #[error(transparent)]
    ContextRender(#[from] context_renders::ContextRenderError),
    #[error(transparent)]
    Attention(#[from] attention::AttentionError),
    #[error(transparent)]
    Intake(#[from] intake::IntakeStoreError),
    #[error(transparent)]
    IntakeCapture(#[from] intake_capture::CaptureError),
    #[error(transparent)]
    Question(#[from] questions::QuestionStoreError),
    #[error(transparent)]
    Inbox(#[from] inbox::InboxError),
    #[error("store already has an owner")]
    AlreadyOwned,
    #[error("store writer is unavailable")]
    Closed,
    #[error("invalid store request: {0}")]
    Invalid(String),
    #[error(
        "producer identity collision at {producer_id}/{incarnation}/{sequence}; incident recorded"
    )]
    SourceCollision {
        producer_id: String,
        incarnation: String,
        sequence: String,
    },
    #[error("event replay failed at sequence {sequence}: {reason}")]
    Replay { sequence: String, reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cursor {
    pub recovery_generation: String,
    pub event_seq: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub schema_version: u32,
    pub cursor: Cursor,
    pub projection: Projection,
    pub sqlite_version: String,
    pub integrity_incidents: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppendReceipt {
    pub event_seq: String,
    pub source_contiguous_seq: String,
    pub duplicate: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventRecord {
    pub seq: String,
    pub observed_utc: String,
    pub event: DomainEvent,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventBatch {
    /// Last returned event, rather than the database head, preserves pagination.
    pub cursor: Cursor,
    pub events: Vec<EventRecord>,
}

type Reply<T> = mpsc::Sender<Result<T>>;

/// Installed by the trusted service at Store startup, never supplied in a
/// network command. Called inside the writer turn, including on source replay.
/// Must be bounded/nonblocking and check current actor/scope/source grants,
/// revocation, independent verifier execution/custody or supervised closure as
/// appropriate, and the exact optional effect key/payload/schedule. Candidate
/// decisions require both independent checks and separate supervised closure.
/// A digest, provider claim or deserialized trust flag is not proof.
pub struct LifecycleEffect<'a> {
    pub idempotency_key: &'a str,
    pub payload: &'a str,
    pub payload_digest: &'a str,
    pub intent_digest: &'a str,
    pub not_before: i64,
}

pub trait LifecycleAuthority: Send + Sync {
    fn authorize(
        &self,
        source: &SourceIdentity,
        command: &LifecycleCommand,
        task: &TaskView,
        effect: Option<&LifecycleEffect<'_>>,
    ) -> bool;
}

/// Independently installed startup policies. This type is never deserialized
/// from a request; default construction installs no authority. Each policy is
/// rechecked by the sole writer on every corresponding command/replay.
#[derive(Default)]
pub struct StoreAuthorities {
    pub lifecycle: Option<Arc<dyn LifecycleAuthority>>,
    pub intake: Option<Arc<dyn intake::IntakeAuthority>>,
    pub intake_capture: Option<Arc<dyn intake_capture::IntakeCaptureAuthority>>,
    pub questions: Option<Arc<dyn questions::QuestionAuthority>>,
}

enum Request {
    CaptureIntake {
        request: Box<intake_capture::CaptureIntakeRequest>,
        reply: Reply<intake_capture::CaptureResult>,
    },
    CapturedIntake {
        scope: Box<admiralty_core::intake::AuthorizedIntakeScope>,
        id: String,
        revision: Option<u32>,
        reply: Reply<Option<intake_capture::CapturedIntake>>,
    },
    QuestionCommand {
        request: Box<questions::QuestionCommand>,
        reply: Reply<questions::QuestionReceipt>,
    },
    QuestionRead {
        request: Box<questions::QuestionRead>,
        reply: Reply<Option<questions::DurableQuestion>>,
    },
    RecordIntake {
        request: Box<admiralty_core::intake::RecordIntakeRequest>,
        reply: Reply<intake::IntakeRecordResult>,
    },
    GetWorkOrder {
        scope: Box<admiralty_core::intake::AuthorizedIntakeScope>,
        id: String,
        revision: Option<u32>,
        reply: Reply<Option<intake::RegisteredWorkOrder>>,
    },
    FindDuplicateCandidates {
        scope: Box<admiralty_core::intake::AuthorizedIntakeScope>,
        query: Box<admiralty_core::intake::DuplicateQuery>,
        reply: Reply<admiralty_core::intake::DuplicatePage>,
    },
    RegisterAttention {
        request: Box<attention::AttentionDefinition>,
        reply: Reply<attention::AttentionRegistration>,
    },
    ApplyAttention {
        request: Box<attention::AttentionCommand>,
        reply: Reply<attention::AttentionResult>,
    },
    AttentionStatus {
        scope: attention::AttentionScope,
        reply: Reply<Option<attention::AttentionStatus>>,
    },
    RecordContextRender {
        request: Box<context_renders::RecordContextRender>,
        reply: Reply<context_renders::ContextRenderReceipt>,
    },
    GetContextRender {
        scope: context_renders::ContextScope,
        key: context_renders::RenderKey,
        reply: Reply<Option<context_renders::StoredContextRender>>,
    },
    ContextRenderPage {
        scope: context_renders::ContextScope,
        cursor: Option<context_renders::RenderCursor>,
        limit: usize,
        reply: Reply<context_renders::ContextRenderPage>,
    },
    AdvanceContextBaseline {
        request: Box<context_renders::AdvanceContextBaseline>,
        reply: Reply<context_renders::BaselineReceipt>,
    },
    ContextBaseline {
        scope: context_renders::ContextScope,
        reply: Reply<Option<context_renders::ContextBaseline>>,
    },
    ClaimOutboxKey {
        now: Option<u64>,
        owner: String,
        lease_secs: u64,
        idempotency_key: String,
        reply: Reply<Option<outbox::Claim>>,
    },
    ClaimOutbox {
        now: Option<u64>,
        owner: String,
        lease_secs: u64,
        reply: Reply<Option<outbox::Claim>>,
    },
    CompleteOutbox {
        token: outbox::ClaimToken,
        receipt_ref: String,
        now: Option<u64>,
        reply: Reply<()>,
    },
    UnknownOutbox {
        token: outbox::ClaimToken,
        reason: String,
        reply: Reply<()>,
    },
    ExecuteLifecycle {
        input: Box<AppendInput>,
        reply: Reply<AppendReceipt>,
    },
    RegisterWorkplan {
        request: Box<workplans::RegisterWorkplanRequest>,
        reply: Reply<workplans::WorkplanReceipt>,
    },
    GetWorkplan {
        project: ProjectId,
        plan: String,
        revision: Option<u32>,
        reply: Reply<Option<workplans::RegisteredWorkplan>>,
    },
    ListWorkplans {
        project: ProjectId,
        after: Option<String>,
        limit: usize,
        reply: Reply<workplans::WorkplanPage>,
    },
    Append {
        source: SourceIdentity,
        source_seq: i64,
        event: Box<DomainEvent>,
        payload: String,
        digest: String,
        observed_utc: String,
        intent: Option<Box<outbox::PreparedIntent>>,
        reply: Reply<AppendReceipt>,
    },
    Snapshot(Reply<Snapshot>),
    Events {
        seq: i64,
        limit: usize,
        reply: Reply<Vec<EventRecord>>,
    },
    EventsSince {
        cursor: Cursor,
        limit: usize,
        reply: Reply<EventBatch>,
    },
    PendingMessages {
        scope: Box<inbox::InboxScope>,
        cursor: Option<inbox::InboxCursor>,
        limit: usize,
        reply: Reply<inbox::PendingInboxPage>,
    },
    ScopedMessageAck {
        scope: Box<inbox::InboxScope>,
        input: Box<AppendInput>,
        reply: Reply<AppendReceipt>,
    },
    Backup {
        destination: PathBuf,
        reply: Reply<Snapshot>,
    },
    Rebuild(Reply<Snapshot>),
    Shutdown,
}

struct AppendInput {
    source: SourceIdentity,
    source_seq: i64,
    event: DomainEvent,
    payload: String,
    digest: String,
    observed_utc: String,
    intent: Option<outbox::PreparedIntent>,
}

struct Inner {
    sender: mpsc::SyncSender<Request>,
    worker: Mutex<Option<JoinHandle<()>>>,
    changes: tokio::sync::watch::Sender<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Last Store clone drains every earlier request, then releases the lock.
        let _ = self.sender.send(Request::Shutdown);
        if let Ok(slot) = self.worker.get_mut()
            && let Some(worker) = slot.take()
        {
            let _ = worker.join();
        }
    }
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_startup_authorities(path, StoreAuthorities::default())
    }

    /// Trusted composition boundary. Default open denies native mutations.
    pub fn open_with_lifecycle_authority(
        path: impl AsRef<Path>,
        authority: Arc<dyn LifecycleAuthority>,
    ) -> Result<Self> {
        Self::open_with_startup_authorities(
            path,
            StoreAuthorities {
                lifecycle: Some(authority),
                ..Default::default()
            },
        )
    }

    /// Preserves the existing trusted lifecycle/intake startup composition.
    pub fn open_with_authorities(
        path: impl AsRef<Path>,
        lifecycle: Option<Arc<dyn LifecycleAuthority>>,
        intake: Option<Arc<dyn intake::IntakeAuthority>>,
    ) -> Result<Self> {
        Self::open_with_startup_authorities(
            path,
            StoreAuthorities {
                lifecycle,
                intake,
                ..Default::default()
            },
        )
    }

    /// Trusted startup-only raw capture; no default issuer or task admission.
    pub fn open_with_intake_capture_authority(
        path: impl AsRef<Path>,
        authority: Arc<dyn intake_capture::IntakeCaptureAuthority>,
    ) -> Result<Self> {
        Self::open_with_startup_authorities(
            path,
            StoreAuthorities {
                intake_capture: Some(authority),
                ..Default::default()
            },
        )
    }

    /// Trusted startup installs current question policy; default open denies it.
    pub fn open_with_question_authority(
        path: impl AsRef<Path>,
        authority: Arc<dyn questions::QuestionAuthority>,
    ) -> Result<Self> {
        Self::open_with_startup_authorities(
            path,
            StoreAuthorities {
                questions: Some(authority),
                ..Default::default()
            },
        )
    }

    /// Compose explicit independent policies without replacing any other policy.
    /// No production policy or request-issued authority is created here.
    pub fn open_with_startup_authorities(
        path: impl AsRef<Path>,
        authorities: StoreAuthorities,
    ) -> Result<Self> {
        Self::open_configured(path.as_ref(), authorities)
    }

    pub fn question_command(
        &self,
        request: questions::QuestionCommand,
    ) -> Result<questions::QuestionReceipt> {
        questions::validate_command(&request)?;
        self.ask(|reply| Request::QuestionCommand {
            request: Box::new(request),
            reply,
        })
    }

    pub fn question(
        &self,
        request: questions::QuestionRead,
    ) -> Result<Option<questions::DurableQuestion>> {
        questions::validate_read_request(&request)?;
        self.ask(|reply| Request::QuestionRead {
            request: Box::new(request),
            reply,
        })
    }

    fn open_configured(path: &Path, authorities: StoreAuthorities) -> Result<Self> {
        let path = canonical_database_path(path)?;
        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".writer.lock");
        let lock = private_file(&PathBuf::from(lock_path), false)?;
        lock.try_lock_exclusive().map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                StoreError::AlreadyOwned
            } else {
                StoreError::Io(error)
            }
        })?;
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (ready_tx, ready_rx) = mpsc::channel();
        let (changes, _) = tokio::sync::watch::channel(());
        let writer_changes = changes.clone();
        let worker = thread::Builder::new()
            .name("admiralty-store-writer".into())
            .spawn(move || match Writer::open(&path, lock) {
                Ok(mut writer) => {
                    writer.lifecycle_authority = authorities.lifecycle;
                    writer.intake_authority = authorities.intake;
                    writer.intake_capture_authority = authorities.intake_capture;
                    writer.question_authority = authorities.questions;
                    if ready_tx.send(Ok(())).is_ok() {
                        writer.run(receiver, writer_changes);
                    }
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            })?;
        match ready_rx.recv().map_err(|_| StoreError::Closed)? {
            Ok(()) => Ok(Self {
                inner: Arc::new(Inner {
                    sender,
                    worker: Mutex::new(Some(worker)),
                    changes,
                }),
            }),
            Err(error) => {
                let _ = worker.join();
                Err(error)
            }
        }
    }

    fn ask<T>(&self, make: impl FnOnce(Reply<T>) -> Request) -> Result<T> {
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(make(sender))
            .map_err(|_| StoreError::Closed)?;
        receiver.recv().map_err(|_| StoreError::Closed)?
    }

    pub fn capture_intake(
        &self,
        request: intake_capture::CaptureIntakeRequest,
    ) -> Result<intake_capture::CaptureResult> {
        intake_capture::validate_request(&request)?;
        self.ask(|reply| Request::CaptureIntake {
            request: Box::new(request),
            reply,
        })
    }
    /// Independently trusted scoped read; public snapshots confer no grant.
    pub fn captured_intake(
        &self,
        scope: admiralty_core::intake::AuthorizedIntakeScope,
        id: String,
        revision: Option<u32>,
    ) -> Result<Option<intake_capture::CapturedIntake>> {
        self.ask(|reply| Request::CapturedIntake {
            scope: Box::new(scope),
            id,
            revision,
            reply,
        })
    }

    pub fn record_intake(
        &self,
        request: admiralty_core::intake::RecordIntakeRequest,
    ) -> Result<intake::IntakeRecordResult> {
        intake::validate_record_request(&request)?;
        self.ask(|reply| Request::RecordIntake {
            request: Box::new(request),
            reply,
        })
    }
    /// Trusted scoped read seam; a public ScopeSnapshot grants nothing.
    pub fn get_work_order(
        &self,
        scope: admiralty_core::intake::AuthorizedIntakeScope,
        id: String,
        revision: Option<u32>,
    ) -> Result<Option<intake::RegisteredWorkOrder>> {
        self.ask(|reply| Request::GetWorkOrder {
            scope: Box::new(scope),
            id,
            revision,
            reply,
        })
    }
    pub fn find_duplicate_candidates(
        &self,
        scope: admiralty_core::intake::AuthorizedIntakeScope,
        query: admiralty_core::intake::DuplicateQuery,
    ) -> Result<admiralty_core::intake::DuplicatePage> {
        self.ask(|reply| Request::FindDuplicateCandidates {
            scope: Box::new(scope),
            query: Box::new(query),
            reply,
        })
    }

    pub fn append(
        &self,
        source: SourceIdentity,
        event: DomainEvent,
        observed_utc: String,
    ) -> Result<AppendReceipt> {
        reject_raw_lifecycle(&event)?;
        self.append_inner(source, event, observed_utc, None)
    }

    /// Atomically persist the event, immutable effect intent, and their association.
    /// This only records a proposal; it never claims or executes the effect.
    pub fn append_with_intent(
        &self,
        source: SourceIdentity,
        event: DomainEvent,
        observed_utc: String,
        intent: outbox::OutboxIntent,
    ) -> Result<AppendReceipt> {
        reject_raw_lifecycle(&event)?;
        let intent = outbox::prepare_intent(&intent)?;
        self.append_inner(source, event, observed_utc, Some(intent))
    }

    /// One authorized CAS and its optional effect intent commit atomically.
    /// Checks are not executed here. A configured trusted runtime adapter must
    /// authenticate their evidence; default-open Store denies every command.
    pub fn execute_lifecycle(
        &self,
        source: SourceIdentity,
        command: LifecycleCommand,
        observed_utc: String,
        intent: Option<outbox::OutboxIntent>,
    ) -> Result<AppendReceipt> {
        let intent = intent.as_ref().map(outbox::prepare_intent).transpose()?;
        let input = Self::prepare_append(
            source,
            DomainEvent::TaskLifecycle { command },
            observed_utc,
            intent,
        )?;
        self.ask(|reply| Request::ExecuteLifecycle {
            input: Box::new(input),
            reply,
        })
    }

    /// Trusted storage seam; authenticated source/delivery authority is external.
    pub fn register_attention(
        &self,
        request: attention::AttentionDefinition,
    ) -> Result<attention::AttentionRegistration> {
        attention::validate_definition(&request)?;
        self.ask(|reply| Request::RegisterAttention {
            request: Box::new(request),
            reply,
        })
    }
    pub fn apply_attention(
        &self,
        request: attention::AttentionCommand,
    ) -> Result<attention::AttentionResult> {
        attention::validate_command(&request)?;
        self.ask(|reply| Request::ApplyAttention {
            request: Box::new(request),
            reply,
        })
    }
    pub fn attention_status(
        &self,
        scope: attention::AttentionScope,
    ) -> Result<Option<attention::AttentionStatus>> {
        attention::validate_scope(&scope)?;
        self.ask(|reply| Request::AttentionStatus { scope, reply })
    }
    /// Trusted in-process storage seam. The caller must independently authorize
    /// this recipient and namespace. Recording is neither delivery nor authority.
    pub fn record_context_render(
        &self,
        request: context_renders::RecordContextRender,
    ) -> Result<context_renders::ContextRenderReceipt> {
        context_renders::validate_record_request(&request)?;
        self.ask(|reply| Request::RecordContextRender {
            request: Box::new(request),
            reply,
        })
    }

    /// Exact scoped retrieval; no recipient, project or fleet-wide fallback.
    pub fn context_render(
        &self,
        scope: context_renders::ContextScope,
        key: context_renders::RenderKey,
    ) -> Result<Option<context_renders::StoredContextRender>> {
        context_renders::validate_get_request(&scope, &key)?;
        self.ask(|reply| Request::GetContextRender { scope, key, reply })
    }

    /// Bounded immutable render history. The cursor is bound to this exact scope.
    pub fn context_renders_page(
        &self,
        scope: context_renders::ContextScope,
        cursor: Option<context_renders::RenderCursor>,
        limit: usize,
    ) -> Result<context_renders::ContextRenderPage> {
        context_renders::validate_page_request(&scope, cursor.as_ref(), limit)?;
        self.ask(|reply| Request::ContextRenderPage {
            scope,
            cursor,
            limit,
            reply,
        })
    }

    /// Explicit application-ACK observation with exact prior-baseline CAS.
    /// Callers authenticate observations separately; this never contacts a provider.
    pub fn advance_context_baseline(
        &self,
        request: context_renders::AdvanceContextBaseline,
    ) -> Result<context_renders::BaselineReceipt> {
        context_renders::validate_advance(&request)?;
        self.ask(|reply| Request::AdvanceContextBaseline {
            request: Box::new(request),
            reply,
        })
    }

    pub fn context_baseline(
        &self,
        scope: context_renders::ContextScope,
    ) -> Result<Option<context_renders::ContextBaseline>> {
        context_renders::validate_page_request(&scope, None, 1)?;
        self.ask(|reply| Request::ContextBaseline { scope, reply })
    }

    /// Trusted in-process dispatcher seam on the single writer connection.
    /// A claim is not execution authority. Admit the exact typed effect and
    /// logical clock independently; never expose this as a raw network tool.
    /// Expired claims remain unknown rather than automatically redispatched.
    pub fn claim_outbox_due(
        &self,
        now: Option<u64>,
        owner: String,
        lease_secs: u64,
    ) -> Result<Option<outbox::Claim>> {
        if owner.is_empty() || owner.len() > 128 || owner.chars().any(char::is_control) {
            return Err(StoreError::Invalid("outbox owner is outside bounds".into()));
        }
        self.ask(|reply| Request::ClaimOutbox {
            now,
            owner,
            lease_secs,
            reply,
        })
    }

    /// Claim one independently authorized effect on the sole writer. Selection
    /// and expiry affect only this exact key; no unrelated work is adopted.
    /// The monotonic trusted clock remains global. This is not a caller grant.
    pub fn claim_outbox_key(
        &self,
        now: Option<u64>,
        owner: String,
        lease_secs: u64,
        idempotency_key: String,
    ) -> Result<Option<outbox::Claim>> {
        outbox::validate_claim_key(&idempotency_key)?;
        if owner.is_empty() || owner.len() > 128 || owner.chars().any(char::is_control) {
            return Err(StoreError::Invalid("outbox owner is outside bounds".into()));
        }
        self.ask(|reply| Request::ClaimOutboxKey {
            now,
            owner,
            lease_secs,
            idempotency_key,
            reply,
        })
    }

    /// Effect receipt only, never task acceptance or permission to retry.
    pub fn complete_outbox(
        &self,
        token: outbox::ClaimToken,
        receipt_ref: String,
        now: Option<u64>,
    ) -> Result<()> {
        validate_outbox_token_envelope(&token)?;
        if receipt_ref.is_empty()
            || receipt_ref.len() > 1024
            || receipt_ref.chars().any(char::is_control)
        {
            return Err(StoreError::Invalid(
                "outbox receipt is outside bounds".into(),
            ));
        }
        self.ask(|reply| Request::CompleteOutbox {
            token,
            receipt_ref,
            now,
            reply,
        })
    }

    /// Preserve an ambiguous effect durably; it does not become runnable again.
    pub fn fail_outbox_unknown(&self, token: outbox::ClaimToken, reason: String) -> Result<()> {
        validate_outbox_token_envelope(&token)?;
        if reason.is_empty() || reason.len() > 1024 || reason.chars().any(char::is_control) {
            return Err(StoreError::Invalid(
                "outbox reason is outside bounds".into(),
            ));
        }
        self.ask(|reply| Request::UnknownOutbox {
            token,
            reason,
            reply,
        })
    }

    fn append_inner(
        &self,
        source: SourceIdentity,
        event: DomainEvent,
        observed_utc: String,
        intent: Option<outbox::PreparedIntent>,
    ) -> Result<AppendReceipt> {
        let AppendInput {
            source,
            source_seq,
            event,
            payload,
            digest,
            observed_utc,
            intent,
        } = Self::prepare_append(source, event, observed_utc, intent)?;
        self.ask(|reply| Request::Append {
            source,
            source_seq,
            event: Box::new(event),
            payload,
            digest,
            observed_utc,
            intent: intent.map(Box::new),
            reply,
        })
    }

    fn prepare_append(
        source: SourceIdentity,
        event: DomainEvent,
        observed_utc: String,
        intent: Option<outbox::PreparedIntent>,
    ) -> Result<AppendInput> {
        let sequence = source.validate().map_err(StoreError::Invalid)?;
        let source_seq = i64::try_from(sequence).map_err(|_| {
            StoreError::Invalid("source sequence exceeds SQLite signed integer range".into())
        })?;
        if source.producer_id.len() > 256
            || source.incarnation.len() > 256
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
        let payload = bounded_json(&event, MAX_EVENT_BYTES).map_err(|()| {
            StoreError::Invalid("event exceeds one MiB or cannot be serialized".into())
        })?;
        let digest = digest(&payload);
        Ok(AppendInput {
            source,
            source_seq,
            event,
            payload,
            digest,
            observed_utc,
            intent,
        })
    }

    /// Trusted storage observation, serialized with append/recovery. It records
    /// no delivery, consumer watermark or acknowledgment. Service callers must
    /// authorize the exact actor/scope before using this low-level seam.
    pub fn pending_messages(
        &self,
        scope: inbox::InboxScope,
        cursor: Option<inbox::InboxCursor>,
        limit: usize,
    ) -> Result<inbox::PendingInboxPage> {
        inbox::validate_request(&scope, cursor.as_ref(), limit)?;
        self.ask(|reply| Request::PendingMessages {
            scope: Box::new(scope),
            cursor,
            limit,
            reply,
        })
    }

    /// Low-level trusted application ACK. Exact project/recipient/epoch is
    /// checked in the same writer turn as append, even on a source replay.
    /// This neither authenticates an actor nor records task acceptance.
    pub fn acknowledge_scoped_message(
        &self,
        source: SourceIdentity,
        scope: inbox::InboxScope,
        message_id: MessageId,
        observed_utc: String,
    ) -> Result<AppendReceipt> {
        scope.validate()?;
        let event = DomainEvent::MessageAcknowledged {
            message_id,
            recipient: scope.recipient.clone(),
            context_epoch: scope.context_epoch.clone(),
        };
        let input = Self::prepare_append(source, event, observed_utc, None)?;
        self.ask(|reply| Request::ScopedMessageAck {
            scope: Box::new(scope),
            input: Box::new(input),
            reply,
        })
    }

    /// Coalesced wake hints only; callers must read durable state with their cursor.
    pub fn subscribe_changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.inner.changes.subscribe()
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        self.ask(Request::Snapshot)
    }

    /// Record an immutable offline plan through the existing single writer.
    /// Registration never submits tasks, emits domain events, or grants execution.
    pub fn register_workplan(
        &self,
        request: workplans::RegisterWorkplanRequest,
    ) -> Result<workplans::WorkplanReceipt> {
        // Bound memory before enqueueing. The registry performs full semantic
        // and digest validation inside the writer's serialized operation.
        if request.source.len() > admiralty_core::workplans::MAX_WORKPLAN_BYTES
            || request.command_id.len() > 128
            || request.plan_id.len() > 128
            || request.expected_current_revision.len() > 10
            || request.source_digest.len() != 71
            || request.canonical_digest.len() != 71
        {
            return Err(StoreError::Invalid(
                "workplan request exceeds admission bounds".into(),
            ));
        }
        self.ask(|reply| Request::RegisterWorkplan {
            request: Box::new(request),
            reply,
        })
    }

    pub fn workplan(
        &self,
        project: ProjectId,
        plan: String,
        revision: Option<u32>,
    ) -> Result<Option<workplans::RegisteredWorkplan>> {
        if plan.is_empty() || plan.len() > 128 {
            return Err(StoreError::Invalid(
                "workplan identifier exceeds admission bounds".into(),
            ));
        }
        TaskId::new(plan.clone()).map_err(|error| StoreError::Invalid(error.to_string()))?;
        self.ask(|reply| Request::GetWorkplan {
            project,
            plan,
            revision,
            reply,
        })
    }

    pub fn workplans(
        &self,
        project: ProjectId,
        after: Option<String>,
        limit: usize,
    ) -> Result<workplans::WorkplanPage> {
        if let Some(after) = &after {
            if after.is_empty() || after.len() > 128 {
                return Err(StoreError::Invalid(
                    "workplan cursor exceeds admission bounds".into(),
                ));
            }
            TaskId::new(after.clone()).map_err(|error| StoreError::Invalid(error.to_string()))?;
        }
        if limit == 0 || limit > workplans::MAX_LIST_PAGE {
            return Err(StoreError::Invalid(
                "workplan list limit must be between 1 and 100".into(),
            ));
        }
        self.ask(|reply| Request::ListWorkplans {
            project,
            after,
            limit,
            reply,
        })
    }

    pub fn events_after(&self, seq: u64, limit: usize) -> Result<Vec<EventRecord>> {
        let seq = i64::try_from(seq).map_err(|_| {
            StoreError::Invalid("event cursor exceeds SQLite signed integer range".into())
        })?;
        self.ask(|reply| Request::Events {
            seq,
            limit: limit.min(MAX_QUERY_EVENTS),
            reply,
        })
    }

    /// Validate generation and read its next bounded page in one writer turn.
    pub fn events_since(&self, cursor: &Cursor, limit: usize) -> Result<EventBatch> {
        self.ask(|reply| Request::EventsSince {
            cursor: cursor.clone(),
            limit: limit.min(MAX_QUERY_EVENTS),
            reply,
        })
    }

    pub fn backup(&self, destination: &Path) -> Result<Snapshot> {
        self.ask(|reply| Request::Backup {
            destination: destination.to_owned(),
            reply,
        })
    }

    /// Recompute the projection from durable events without running external effects.
    /// Successful explicit rebuild changes the cursor generation for subscribers.
    pub fn rebuild(&self) -> Result<Snapshot> {
        self.ask(Request::Rebuild)
    }
}

// Existing parents may be searchable, but never writable by another Unix user.
// Newly created data directories are private. SQLite inherits the0600 database
// mode for WAL/SHM; reject pre-existing unsafe sidecars before opening SQLite.
fn canonical_database_path(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(parent)?;
    let parent = parent.canonicalize()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if parent.metadata()?.permissions().mode() & 0o022 != 0 {
            return Err(StoreError::Invalid(
                "database parent must not be group/world writable".into(),
            ));
        }
    }
    let name = path
        .file_name()
        .ok_or_else(|| StoreError::Invalid("database path requires a filename".into()))?;
    let path = parent.join(name);
    drop(private_file(&path, false)?);
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        validate_private_file(&PathBuf::from(sidecar))?;
    }
    Ok(path)
}

fn validate_private_file(path: &Path) -> Result<()> {
    match path.symlink_metadata() {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(StoreError::Invalid(
                    "database files must be regular files, never symlinks".into(),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                if metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
                    return Err(StoreError::Invalid(
                        "database files require private0600 permissions and one link".into(),
                    ));
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn private_file(path: &Path, create_new: bool) -> Result<File> {
    validate_private_file(path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).truncate(false);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    validate_private_file(path)?;
    Ok(file)
}

fn digest(payload: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(payload.as_bytes()))
}

fn bounded_json(value: &impl serde::Serialize, maximum: usize) -> std::result::Result<String, ()> {
    admiralty_core::limits::bounded_json_size(value, maximum).map_err(|_| ())?;
    serde_json::to_string(value).map_err(|_| ())
}

/// Inspect SQLite's borrowed bytes before allocating any owned string.
pub(crate) fn stored_text(
    row: &rusqlite::Row<'_>,
    index: usize,
    maximum: usize,
) -> rusqlite::Result<String> {
    match row.get_ref(index)? {
        rusqlite::types::ValueRef::Text(bytes) if bytes.len() <= maximum => {
            std::str::from_utf8(bytes)
                .map(str::to_owned)
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        index,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
        }
        value => Err(rusqlite::Error::FromSqlConversionFailure(
            index,
            value.data_type(),
            Box::new(std::io::Error::other(
                "stored text is invalid or exceeds its byte budget",
            )),
        )),
    }
}

fn identity_digest(event_digest: &str, observed_utc: &str, intent_digest: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(event_digest.as_bytes());
    hash.update([0]);
    hash.update(observed_utc.as_bytes());
    hash.update([0]);
    hash.update(intent_digest.unwrap_or("plain-event").as_bytes());
    format!("sha256:{:x}", hash.finalize())
}

fn fixed_sqlite(version: i32) -> bool {
    version >= 3_051_003
        || (3_050_007..3_051_000).contains(&version)
        || (3_044_006..3_045_000).contains(&version)
}

fn source_collision(source: &SourceIdentity) -> StoreError {
    StoreError::SourceCollision {
        producer_id: source.producer_id.clone(),
        incarnation: source.incarnation.clone(),
        sequence: source.sequence.clone(),
    }
}

fn record_source_collision(
    tx: &Transaction<'_>,
    source: &SourceIdentity,
    source_seq: i64,
    original_digest: &str,
    conflicting_digest: &str,
    observed_utc: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO integrity_incidents(producer_id,incarnation,producer_seq,original_digest,conflicting_digest,observed_utc) VALUES(?1,?2,?3,?4,?5,?6)",
        params![source.producer_id, source.incarnation, source_seq, original_digest, conflicting_digest, observed_utc],
    )?;
    Ok(())
}

fn associated_intent(tx: &Connection, event_seq: i64) -> Result<Option<outbox::PreparedIntent>> {
    struct AssociationRow {
        association_id: i64,
        association_key: String,
        association_payload_digest: String,
        association_due: i64,
        association_intent_digest: String,
        intent_id: i64,
        key: String,
        payload: String,
        payload_digest: String,
        due: i64,
    }
    let row = tx.query_row(
        "SELECT a.outbox_intent_id,a.idempotency_key,a.payload_digest,a.not_before,a.intent_digest,o.id,o.idempotency_key,o.payload,o.payload_digest,o.not_before FROM event_outbox_associations a LEFT JOIN outbox_intents o ON o.id=a.outbox_intent_id WHERE a.event_seq=?1",
        [event_seq],
        |row| Ok(AssociationRow {
            association_id: row.get(0)?, association_key: stored_text(row,1,256)?,
            association_payload_digest: stored_text(row,2,71)?, association_due: row.get(3)?,
            association_intent_digest: stored_text(row,4,71)?, intent_id: row.get(5)?,
            key: stored_text(row,6,256)?, payload: stored_text(row,7,outbox::MAX_PAYLOAD_BYTES)?, payload_digest: stored_text(row,8,71)?, due: row.get(9)?,
        }),
    ).optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let calculated_payload_digest = digest(&row.payload);
    let calculated_intent_digest = outbox::prepared_intent_digest(&row.key, &row.payload, row.due);
    if row.association_id != row.intent_id
        || row.association_key != row.key
        || row.association_payload_digest != row.payload_digest
        || row.association_due != row.due
        || row.payload_digest != calculated_payload_digest
        || row.association_intent_digest != calculated_intent_digest
        || row.due < 0
        || row.key.is_empty()
        || row.key.chars().any(char::is_control)
    {
        return Err(StoreError::Replay {
            sequence: event_seq.to_string(),
            reason: "event/outbox association integrity mismatch".into(),
        });
    }
    Ok(Some(outbox::PreparedIntent {
        idempotency_key: row.key,
        payload: row.payload,
        payload_digest: row.payload_digest,
        intent_digest: row.association_intent_digest,
        not_before: row.due,
    }))
}

fn request_digest(
    source: &SourceIdentity,
    event_digest: &str,
    observed: &str,
    intent: Option<&str>,
) -> String {
    let mut hash = Sha256::new();
    for field in [
        &source.producer_id,
        &source.incarnation,
        &source.sequence,
        event_digest,
        observed,
        intent.unwrap_or("plain-event"),
    ] {
        hash.update(field.as_bytes());
        hash.update([0]);
    }
    format!("sha256:{:x}", hash.finalize())
}

struct CheckedEvent {
    event: DomainEvent,
    payload: String,
    digest: String,
    observed_utc: String,
    intent: Option<outbox::PreparedIntent>,
}

fn checked_event(connection: &Connection, sequence: i64) -> Result<CheckedEvent> {
    let fail = |reason: &str| StoreError::Replay {
        sequence: sequence.to_string(),
        reason: reason.into(),
    };
    let (producer,incarnation,producer_seq,expected,observed,payload): (String,String,i64,String,String,String) = connection.query_row(
        "SELECT producer_id,incarnation,producer_seq,digest,observed_utc,payload FROM domain_events WHERE seq=?1",[sequence],
        |row| Ok((stored_text(row,0,256)?,stored_text(row,1,256)?,row.get(2)?,stored_text(row,3,71)?,stored_text(row,4,128)?,stored_text(row,5,MAX_EVENT_BYTES)?)))?;
    let source = SourceIdentity {
        producer_id: producer,
        incarnation,
        sequence: producer_seq.to_string(),
    };
    if sequence <= 0
        || source.validate().is_err()
        || source.producer_id.chars().any(char::is_control)
        || source.incarnation.chars().any(char::is_control)
        || observed.is_empty()
        || observed.chars().any(char::is_control)
        || digest(&payload) != expected
    {
        return Err(fail(
            "stored event source, timestamp or payload digest mismatch",
        ));
    }
    let event: DomainEvent =
        serde_json::from_str(&payload).map_err(|error| fail(&error.to_string()))?;
    if bounded_json(&event, MAX_EVENT_BYTES).map_err(|()| fail("stored event exceeds bound"))?
        != payload
    {
        return Err(fail("stored event is not canonical"));
    }
    let intent = associated_intent(connection, sequence)?;
    let binding: Option<(String, Option<String>)> = connection
        .query_row(
            "SELECT request_digest,intent_digest FROM event_request_bindings WHERE event_seq=?1",
            [sequence],
            |row| {
                Ok((
                    stored_text(row, 0, 71)?,
                    if matches!(row.get_ref(1)?, rusqlite::types::ValueRef::Null) {
                        None
                    } else {
                        Some(stored_text(row, 1, 71)?)
                    },
                ))
            },
        )
        .optional()?;
    let intent_digest = intent.as_ref().map(|v| v.intent_digest.as_str());
    match binding {
        Some((request, bound_intent))
            if request == request_digest(&source, &expected, &observed, intent_digest)
                && bound_intent.as_deref() == intent_digest => {}
        Some(_) => return Err(fail("stored request/intent binding mismatch")),
        None => {
            let legacy: String = connection.query_row(
                "SELECT value FROM metadata WHERE key='atomic_effects_legacy_event_seq'",
                [],
                |row| stored_text(row, 0, 20),
            )?;
            let watermark = legacy
                .parse::<i64>()
                .ok()
                .filter(|n| *n >= 0 && n.to_string() == legacy)
                .ok_or_else(|| fail("invalid legacy event watermark"))?;
            if sequence > watermark {
                return Err(fail("event request binding missing"));
            }
        }
    }
    Ok(CheckedEvent {
        event,
        payload,
        digest: expected,
        observed_utc: observed,
        intent,
    })
}

struct Writer {
    connection: Connection,
    projection: Projection,
    pending_inbox: inbox::PendingInboxIndex,
    generation: String,
    lifecycle_authority: Option<Arc<dyn LifecycleAuthority>>,
    intake_authority: Option<Arc<dyn intake::IntakeAuthority>>,
    intake_capture_authority: Option<Arc<dyn intake_capture::IntakeCaptureAuthority>>,
    question_authority: Option<Arc<dyn questions::QuestionAuthority>>,
    _lock: File,
}

impl Writer {
    fn open(path: &Path, lock: File) -> Result<Self> {
        if !fixed_sqlite(rusqlite::version_number()) {
            return Err(StoreError::Invalid(format!(
                "SQLite {} lacks the required WAL fix",
                rusqlite::version()
            )));
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        let journal: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        let synchronous: i64 = connection.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
        let foreign_keys: i64 =
            connection.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        if journal != "wal" || synchronous != 2 || foreign_keys != 1 {
            return Err(StoreError::Invalid(
                "required SQLite durability pragmas were not applied".into(),
            ));
        }
        let schema: u32 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if schema > 1 {
            return Err(StoreError::Invalid(
                "database schema is newer than this executable".into(),
            ));
        }
        connection.execute_batch(include_str!("../migrations/0001_foundation.sql"))?;
        // Storage initialization only. Startup/replay never reserves or delivers
        // an outbox intent; explicit trusted-clock helpers remain separate.
        outbox::initialize(&connection)?;
        let migration = Transaction::new_unchecked(&connection, TransactionBehavior::Immediate)?;
        migration.execute_batch(include_str!("../migrations/0003_atomic_effects.sql"))?;
        migration.commit()?;
        workplans::initialize(&connection)?;
        context_renders::initialize(&connection)?;
        attention::initialize(&connection)?;
        intake::initialize(&connection)?;
        intake_capture::initialize(&connection)?;
        questions::initialize(&connection)?;
        let generation = connection
            .query_row(
                "SELECT value FROM metadata WHERE key='recovery_generation'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let generation = match generation {
            Some(value) => value,
            None => {
                let value = uuid::Uuid::new_v4().to_string();
                connection.execute(
                    "INSERT INTO metadata(key,value) VALUES('recovery_generation',?1)",
                    [&value],
                )?;
                value
            }
        };
        let projection = replay(&connection)?;
        questions::validate_all(&connection)?;
        let pending_inbox = inbox::PendingInboxIndex::from_projection(&projection)?;
        workplans::validate_task_contracts(
            &connection,
            projection.tasks.values().map(|task| &task.contract),
        )?;
        Ok(Self {
            connection,
            projection,
            pending_inbox,
            generation,
            lifecycle_authority: None,
            intake_authority: None,
            intake_capture_authority: None,
            question_authority: None,
            _lock: lock,
        })
    }

    fn run(&mut self, receiver: mpsc::Receiver<Request>, changes: tokio::sync::watch::Sender<()>) {
        while let Ok(request) = receiver.recv() {
            match request {
                Request::CaptureIntake { request, reply } => {
                    let result = self
                        .intake_capture_authority
                        .as_ref()
                        .ok_or(intake_capture::CaptureError::Denied)
                        .and_then(|authority| authority.authorize(&request))
                        .and_then(|context| {
                            intake_capture::record(&mut self.connection, *request, context)
                        })
                        .map_err(StoreError::from);
                    let _ = reply.send(result);
                }
                Request::CapturedIntake {
                    scope,
                    id,
                    revision,
                    reply,
                } => {
                    let _ = reply.send(
                        intake_capture::get(&mut self.connection, &scope, &id, revision)
                            .map_err(StoreError::from),
                    );
                }
                Request::RecordIntake { request, reply } => {
                    let result = self
                        .intake_authority
                        .as_ref()
                        .ok_or(intake::IntakeStoreError::Denied)
                        .and_then(|authority| authority.authorize(&request))
                        .and_then(|context| {
                            let existing = request
                                .selected_tasks
                                .iter()
                                .filter_map(|s| self.projection.tasks.get(&s.task.task_id))
                                .map(|task| {
                                    content_digest(&task.contract)
                                        .map(|digest| (task.contract.task_id.clone(), digest))
                                })
                                .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
                            intake::record(&mut self.connection, *request, context, &existing)
                        })
                        .map_err(StoreError::from);
                    let _ = reply.send(result);
                }
                Request::GetWorkOrder {
                    scope,
                    id,
                    revision,
                    reply,
                } => {
                    let _ = reply.send(
                        intake::get_work_order(&mut self.connection, &scope, &id, revision)
                            .map_err(StoreError::from),
                    );
                }
                Request::FindDuplicateCandidates {
                    scope,
                    query,
                    reply,
                } => {
                    let _ = reply.send(
                        intake::find_duplicate_candidates(
                            &mut self.connection,
                            &scope,
                            &query,
                            &self.projection,
                        )
                        .map_err(StoreError::from),
                    );
                }

                Request::QuestionCommand { request, reply } => {
                    let task = self.projection.tasks.get(&request.scope.question.task_id);
                    let _ = reply.send(
                        questions::append_authorized(
                            &mut self.connection,
                            &request,
                            self.question_authority.as_deref(),
                            task,
                        )
                        .map_err(StoreError::from),
                    );
                }
                Request::QuestionRead { request, reply } => {
                    let task = self.projection.tasks.get(&request.scope.question.task_id);
                    let _ = reply.send(
                        questions::read_authorized(
                            &self.connection,
                            &request,
                            self.question_authority.as_deref(),
                            task,
                        )
                        .map_err(StoreError::from),
                    );
                }
                Request::RegisterAttention { request, reply } => {
                    let _ = reply.send(
                        attention::register(&mut self.connection, *request)
                            .map_err(StoreError::from),
                    );
                }
                Request::ApplyAttention { request, reply } => {
                    let _ = reply.send(
                        attention::apply(&mut self.connection, *request).map_err(StoreError::from),
                    );
                }
                Request::AttentionStatus { scope, reply } => {
                    let _ = reply.send(
                        attention::status(&self.connection, &scope).map_err(StoreError::from),
                    );
                }
                Request::RecordContextRender { request, reply } => {
                    let _ = reply.send(
                        context_renders::record(&mut self.connection, *request)
                            .map_err(StoreError::from),
                    );
                }
                Request::GetContextRender { scope, key, reply } => {
                    let _ = reply.send(
                        context_renders::get(&self.connection, &scope, &key)
                            .map_err(StoreError::from),
                    );
                }
                Request::ContextRenderPage {
                    scope,
                    cursor,
                    limit,
                    reply,
                } => {
                    let _ = reply.send(
                        context_renders::page(&self.connection, &scope, cursor.as_ref(), limit)
                            .map_err(StoreError::from),
                    );
                }
                Request::AdvanceContextBaseline { request, reply } => {
                    let _ = reply.send(
                        context_renders::advance(&mut self.connection, *request)
                            .map_err(StoreError::from),
                    );
                }
                Request::ContextBaseline { scope, reply } => {
                    let _ = reply.send(
                        context_renders::baseline(&self.connection, &scope)
                            .map_err(StoreError::from),
                    );
                }
                Request::ClaimOutboxKey {
                    now,
                    owner,
                    lease_secs,
                    idempotency_key,
                    reply,
                } => {
                    let result = outbox::claim_key_due(
                        &mut self.connection,
                        now,
                        &owner,
                        lease_secs,
                        &idempotency_key,
                    )
                    .map_err(StoreError::from);
                    let _ = reply.send(result);
                }
                Request::RegisterWorkplan { request, reply } => {
                    // Compare only incoming identities (core caps this at 1000),
                    // rather than copying/hashing an arbitrarily large unrelated
                    // projection or denying historical retries as it grows.
                    let result = admiralty_core::workplans::preview_workplan(&request.source)
                        .map_err(workplans::WorkplanStoreError::from)
                        .map_err(StoreError::from)
                        .and_then(|preview| {
                            preview
                                .task_digests
                                .keys()
                                .filter_map(|id| {
                                    self.projection.tasks.get(id).map(|task| (id, task))
                                })
                                .map(|(id, task)| {
                                    content_digest(&task.contract)
                                        .map(|digest| (id.clone(), digest))
                                })
                                .collect::<std::result::Result<BTreeMap<_, _>, _>>()
                                .map_err(StoreError::from)
                        })
                        .and_then(|existing| {
                            workplans::register_with_existing(
                                &mut self.connection,
                                *request,
                                &existing,
                            )
                            .map_err(StoreError::from)
                        });
                    // No domain cursor or domain-change hint for registry-only writes.
                    let _ = reply.send(result);
                }
                Request::GetWorkplan {
                    project,
                    plan,
                    revision,
                    reply,
                } => {
                    let _ = reply.send(
                        workplans::get(&mut self.connection, &project, &plan, revision)
                            .map_err(StoreError::from),
                    );
                }
                Request::ListWorkplans {
                    project,
                    after,
                    limit,
                    reply,
                } => {
                    let _ = reply.send(
                        workplans::list(&mut self.connection, &project, after.as_deref(), limit)
                            .map_err(StoreError::from),
                    );
                }
                Request::ClaimOutbox {
                    now,
                    owner,
                    lease_secs,
                    reply,
                } => {
                    let result = outbox::claim_due(&mut self.connection, now, &owner, lease_secs)
                        .map_err(StoreError::from);
                    let _ = reply.send(result);
                }
                Request::CompleteOutbox {
                    token,
                    receipt_ref,
                    now,
                    reply,
                } => {
                    let result = outbox::complete(&mut self.connection, &token, &receipt_ref, now)
                        .map_err(StoreError::from);
                    let _ = reply.send(result);
                }
                Request::UnknownOutbox {
                    token,
                    reason,
                    reply,
                } => {
                    let result = outbox::fail_unknown(&mut self.connection, &token, &reason)
                        .map_err(StoreError::from);
                    let _ = reply.send(result);
                }
                Request::Append {
                    source,
                    source_seq,
                    event,
                    payload,
                    digest,
                    observed_utc,
                    intent,
                    reply,
                } => {
                    let result = reject_raw_lifecycle(&event).and_then(|()| {
                        self.append(AppendInput {
                            source,
                            source_seq,
                            event: *event,
                            payload,
                            digest,
                            observed_utc,
                            intent: intent.map(|intent| *intent),
                        })
                    });
                    if result.as_ref().is_ok_and(|receipt| !receipt.duplicate) {
                        changes.send_replace(());
                    }
                    let _ = reply.send(result);
                }
                Request::ExecuteLifecycle { input, reply } => {
                    let result = self
                        .authorize_lifecycle(&input)
                        .and_then(|()| self.append(*input));
                    if result.as_ref().is_ok_and(|receipt| !receipt.duplicate) {
                        changes.send_replace(());
                    }
                    let _ = reply.send(result);
                }
                Request::Snapshot(reply) => {
                    let _ = reply.send(self.snapshot());
                }
                Request::Events { seq, limit, reply } => {
                    let _ = reply.send(self.events_after(seq, limit));
                }
                Request::EventsSince {
                    cursor,
                    limit,
                    reply,
                } => {
                    let _ = reply.send(self.events_since(&cursor, limit));
                }
                Request::PendingMessages {
                    scope,
                    cursor,
                    limit,
                    reply,
                } => {
                    let result = inbox::pending_page(
                        &self.projection,
                        &self.pending_inbox,
                        &self.generation,
                        &scope,
                        cursor.as_ref(),
                        limit,
                    )
                    .map_err(StoreError::from);
                    let _ = reply.send(result);
                }
                Request::ScopedMessageAck {
                    scope,
                    input,
                    reply,
                } => {
                    let result = self
                        .validate_ack_scope(&scope, &input.event)
                        .and_then(|()| self.append(*input));
                    if result.as_ref().is_ok_and(|receipt| !receipt.duplicate) {
                        changes.send_replace(());
                    }
                    let _ = reply.send(result);
                }
                Request::Backup { destination, reply } => {
                    let _ = reply.send(self.backup(&destination));
                }
                Request::Rebuild(reply) => {
                    let result = self.rebuild();
                    if result.is_ok() {
                        changes.send_replace(());
                    }
                    let _ = reply.send(result);
                }
                Request::Shutdown => break,
            }
        }
        let _ = self
            .connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    }

    fn validate_ack_scope(&self, scope: &inbox::InboxScope, event: &DomainEvent) -> Result<()> {
        scope.validate()?;
        let DomainEvent::MessageAcknowledged {
            message_id,
            recipient,
            context_epoch,
        } = event
        else {
            return Err(StoreError::Invalid(
                "scoped acknowledgment requires an ACK event".into(),
            ));
        };
        let Some(message) = self.projection.messages.get(message_id) else {
            return Err(StoreError::Invalid(
                "scoped acknowledgment message is unavailable".into(),
            ));
        };
        if message.project_id != scope.project_id
            || message.recipient != scope.recipient
            || message.context_epoch != scope.context_epoch
            || *recipient != scope.recipient
            || *context_epoch != scope.context_epoch
        {
            return Err(StoreError::Invalid(
                "scoped acknowledgment does not match the message".into(),
            ));
        }
        Ok(())
    }

    fn authorize_lifecycle(&self, input: &AppendInput) -> Result<()> {
        let DomainEvent::TaskLifecycle { command } = &input.event else {
            return Err(StoreError::Invalid(
                "native lifecycle command required".into(),
            ));
        };
        let effect = input.intent.as_ref().map(|intent| LifecycleEffect {
            idempotency_key: &intent.idempotency_key,
            payload: &intent.payload,
            payload_digest: &intent.payload_digest,
            intent_digest: &intent.intent_digest,
            not_before: intent.not_before,
        });
        let allowed = self
            .projection
            .tasks
            .get(&command.task_id)
            .is_some_and(|task| {
                self.lifecycle_authority.as_ref().is_some_and(|authority| {
                    authority.authorize(&input.source, command, task, effect.as_ref())
                })
            });
        if !allowed {
            return Err(StoreError::Invalid(
                "native lifecycle command is not authorized".into(),
            ));
        }
        Ok(())
    }

    fn append(&mut self, input: AppendInput) -> Result<AppendReceipt> {
        let AppendInput {
            source,
            source_seq,
            event,
            payload,
            digest: incoming_digest,
            observed_utc,
            intent,
        } = input;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<i64> = tx.query_row(
            "SELECT seq FROM domain_events WHERE producer_id=?1 AND incarnation=?2 AND producer_seq=?3",
            params![source.producer_id, source.incarnation, source_seq], |row| row.get(0)).optional()?;
        if let Some(seq) = existing {
            let checked = checked_event(&tx, seq)?;
            let original_digest = checked.digest;
            let original_payload = checked.payload;
            let original_observed = checked.observed_utc;
            let original_intent = checked.intent;
            let intent_matches = match (&original_intent, &intent) {
                (None, None) => true,
                (Some(original), Some(incoming)) => {
                    original.idempotency_key == incoming.idempotency_key
                        && original.payload == incoming.payload
                        && original.payload_digest == incoming.payload_digest
                        && original.not_before == incoming.not_before
                        && original.intent_digest == incoming.intent_digest
                }
                _ => false,
            };
            if original_digest != incoming_digest || original_payload != payload || !intent_matches
            {
                let original_identity = identity_digest(
                    &original_digest,
                    &original_observed,
                    original_intent
                        .as_ref()
                        .map(|value| value.intent_digest.as_str()),
                );
                let conflicting_identity = identity_digest(
                    &incoming_digest,
                    &observed_utc,
                    intent.as_ref().map(|value| value.intent_digest.as_str()),
                );
                record_source_collision(
                    &tx,
                    &source,
                    source_seq,
                    &original_identity,
                    &conflicting_identity,
                    &observed_utc,
                )?;
                tx.commit()?;
                return Err(source_collision(&source));
            }
            let contiguous: i64 = tx.query_row("SELECT contiguous_seq FROM producer_watermarks WHERE producer_id=?1 AND incarnation=?2",
                params![source.producer_id, source.incarnation], |row| row.get(0)).optional()?.unwrap_or(0);
            return Ok(AppendReceipt {
                event_seq: seq.to_string(),
                source_contiguous_seq: contiguous.to_string(),
                duplicate: true,
            });
        }
        if let DomainEvent::TaskSubmitted { contract } = &event {
            workplans::validate_task_contract_in_transaction(&tx, contract)?;
        }
        if let DomainEvent::TaskLifecycle { command } = &event
            && let LifecycleAction::ReviseContract { contract } = &command.action
        {
            workplans::validate_task_contract_in_transaction(&tx, contract)?;
        }
        let next_projection = reduce(&self.projection, &event)?;
        let pending_update = inbox::PendingInboxIndex::prepare_update(&next_projection, &event)?;
        let mut associated_intent = None;
        if let Some(intent) = &intent {
            match outbox::enqueue_prepared_in_transaction(&tx, intent)? {
                outbox::EnqueueResult::Inserted { id } => associated_intent = Some((id, intent)),
                outbox::EnqueueResult::ExistingExact { .. } => {
                    tx.execute("INSERT INTO outbox_integrity_incidents(idempotency_key,original_digest,conflicting_digest) VALUES(?1,?2,?2)", params![intent.idempotency_key, intent.intent_digest])?;
                    tx.commit()?;
                    return Err(StoreError::Outbox(outbox::OutboxError::IdentityCollision));
                }
                outbox::EnqueueResult::Collision => {
                    tx.commit()?;
                    return Err(StoreError::Outbox(outbox::OutboxError::IdentityCollision));
                }
            }
        }
        tx.execute("INSERT INTO domain_events(producer_id,incarnation,producer_seq,digest,observed_utc,payload) VALUES(?1,?2,?3,?4,?5,?6)",
            params![source.producer_id, source.incarnation, source_seq, incoming_digest, observed_utc, payload])?;
        let seq = tx.last_insert_rowid();
        if let Some((intent_id, intent)) = associated_intent {
            tx.execute("INSERT INTO event_outbox_associations(event_seq,outbox_intent_id,idempotency_key,payload_digest,not_before,intent_digest) VALUES(?1,?2,?3,?4,?5,?6)",
                params![seq, intent_id, intent.idempotency_key, intent.payload_digest, intent.not_before, intent.intent_digest])?;
        }
        tx.execute("INSERT INTO event_request_bindings(event_seq,request_digest,intent_digest) VALUES(?1,?2,?3)",
            params![seq, request_digest(&source,&incoming_digest,&observed_utc,intent.as_ref().map(|v|v.intent_digest.as_str())),intent.as_ref().map(|v|v.intent_digest.as_str())])?;
        let mut contiguous: i64 = tx.query_row("SELECT contiguous_seq FROM producer_watermarks WHERE producer_id=?1 AND incarnation=?2",
            params![source.producer_id, source.incarnation], |row| row.get(0)).optional()?.unwrap_or(0);
        while let Some(next) = contiguous.checked_add(1) {
            let present: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM domain_events WHERE producer_id=?1 AND incarnation=?2 AND producer_seq=?3)",
                params![source.producer_id, source.incarnation, next], |row| row.get(0))?;
            if !present {
                break;
            }
            contiguous = next;
        }
        tx.execute("INSERT INTO producer_watermarks(producer_id,incarnation,contiguous_seq) VALUES(?1,?2,?3) ON CONFLICT(producer_id,incarnation) DO UPDATE SET contiguous_seq=excluded.contiguous_seq",
            params![source.producer_id, source.incarnation, contiguous])?;
        tx.commit()?;
        self.projection = next_projection;
        self.pending_inbox.apply_prepared(pending_update);
        Ok(AppendReceipt {
            event_seq: seq.to_string(),
            source_contiguous_seq: contiguous.to_string(),
            duplicate: false,
        })
    }

    fn snapshot(&self) -> Result<Snapshot> {
        let seq: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM domain_events",
            [],
            |row| row.get(0),
        )?;
        let incidents: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM integrity_incidents", [], |row| {
                    row.get(0)
                })?;
        Ok(Snapshot {
            schema_version: if self
                .projection
                .tasks
                .values()
                .any(|t| t.lifecycle.is_some())
            {
                2
            } else {
                1
            },
            cursor: Cursor {
                recovery_generation: self.generation.clone(),
                event_seq: seq.to_string(),
            },
            projection: self.projection.clone(),
            sqlite_version: rusqlite::version().to_owned(),
            integrity_incidents: incidents.to_string(),
        })
    }

    fn events_after(&self, seq: i64, limit: usize) -> Result<Vec<EventRecord>> {
        let mut query = self
            .connection
            .prepare("SELECT seq FROM domain_events WHERE seq>?1 ORDER BY seq LIMIT ?2")?;
        let mut rows = query.query(params![seq, limit as i64])?;
        let mut events = Vec::new();
        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            let checked = checked_event(&self.connection, seq)?;
            events.push(EventRecord {
                seq: seq.to_string(),
                observed_utc: checked.observed_utc,
                event: checked.event,
            });
        }
        Ok(events)
    }

    fn events_since(&self, cursor: &Cursor, limit: usize) -> Result<EventBatch> {
        let invalid = |reason: &str| StoreError::Invalid(format!("snapshot_required: {reason}"));
        if cursor.recovery_generation != self.generation {
            return Err(invalid("recovery generation changed"));
        }
        let seq = cursor
            .event_seq
            .parse::<i64>()
            .ok()
            .filter(|seq| *seq >= 0 && seq.to_string() == cursor.event_seq)
            .ok_or_else(|| invalid("invalid event sequence"))?;
        let head: i64 = self.connection.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM domain_events",
            [],
            |row| row.get(0),
        )?;
        if seq > head {
            return Err(invalid("event cursor is ahead of durable state"));
        }
        let events = self.events_after(seq, limit)?;
        let event_seq = events
            .last()
            .map(|event| event.seq.clone())
            .unwrap_or_else(|| cursor.event_seq.clone());
        Ok(EventBatch {
            cursor: Cursor {
                recovery_generation: self.generation.clone(),
                event_seq,
            },
            events,
        })
    }

    fn rebuild(&mut self) -> Result<Snapshot> {
        context_renders::validate_all(&self.connection)?;
        attention::validate_all(&self.connection)?;
        intake::validate_all(&self.connection)?;
        intake_capture::validate_all(&self.connection)?;
        questions::validate_all(&self.connection)?;
        let projection = replay(&self.connection)?;
        let pending_inbox = inbox::PendingInboxIndex::from_projection(&projection)?;
        workplans::validate_task_contracts(
            &self.connection,
            projection.tasks.values().map(|task| &task.contract),
        )?;
        let generation = uuid::Uuid::new_v4().to_string();
        self.connection.execute(
            "UPDATE metadata SET value=?1 WHERE key='recovery_generation'",
            [&generation],
        )?;
        self.projection = projection;
        self.pending_inbox = pending_inbox;
        self.generation = generation;
        self.snapshot()
    }

    fn backup(&self, destination: &Path) -> Result<Snapshot> {
        context_renders::validate_all(&self.connection)?;
        attention::validate_all(&self.connection)?;
        intake::validate_all(&self.connection)?;
        intake_capture::validate_all(&self.connection)?;
        questions::validate_all(&self.connection)?;
        // Hard-link publication is atomic and refuses an existing destination.
        // Staging and destination share a directory, so publication cannot cross filesystems.
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let stage = parent.join(format!(".admiralty-backup-{}.sqlite", uuid::Uuid::new_v4()));
        let stage_file = private_file(&stage, true)?;
        drop(stage_file);
        let result = (|| {
            if destination.symlink_metadata().is_ok() {
                return Err(StoreError::Invalid(
                    "backup destination already exists".into(),
                ));
            }
            let mut target = Connection::open(&stage)?;
            {
                let backup = rusqlite::backup::Backup::new(&self.connection, &mut target)?;
                backup.run_to_completion(128, Duration::from_millis(5), None)?;
            }
            let check: String = target.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
            if check != "ok" {
                return Err(StoreError::Invalid(
                    "backup SQLite quick_check failed".into(),
                ));
            }
            target
                .close()
                .map_err(|(_, error)| StoreError::Sqlite(error))?;
            File::open(&stage)?.sync_all()?;
            fs::hard_link(&stage, destination)?;
            File::open(parent)?.sync_all()?;
            self.snapshot()
        })();
        let _ = fs::remove_file(&stage);
        result
    }
}

fn replay(connection: &Connection) -> Result<Projection> {
    if connection.is_autocommit() {
        let tx = Transaction::new_unchecked(connection, TransactionBehavior::Deferred)?;
        let projection = replay_inner(&tx)?;
        tx.commit()?;
        Ok(projection)
    } else {
        replay_inner(connection)
    }
}

fn replay_inner(connection: &Connection) -> Result<Projection> {
    let orphaned: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM event_outbox_associations a LEFT JOIN domain_events d ON d.seq=a.event_seq LEFT JOIN outbox_intents o ON o.id=a.outbox_intent_id WHERE d.seq IS NULL OR o.id IS NULL) OR EXISTS(SELECT 1 FROM event_request_bindings b LEFT JOIN domain_events d ON d.seq=b.event_seq WHERE d.seq IS NULL)",[],|row|row.get(0))?;
    if orphaned {
        return Err(StoreError::Replay {
            sequence: "0".into(),
            reason: "orphaned event/effect binding".into(),
        });
    }
    let mut projection = Projection::default();
    let mut statement = connection.prepare("SELECT seq FROM domain_events ORDER BY seq")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get(0)?;
        let checked = checked_event(connection, seq)?;
        projection = reduce(&projection, &checked.event).map_err(|error| StoreError::Replay {
            sequence: seq.to_string(),
            reason: error.to_string(),
        })?;
    }
    Ok(projection)
}

#[cfg(test)]
mod tests {
    use super::{Store, fixed_sqlite, outbox};
    use admiralty_core::{DomainEvent, SourceIdentity, TaskContract, TaskId};
    use rusqlite::Connection;
    use serde_json::json;

    fn source(sequence: &str) -> SourceIdentity {
        SourceIdentity {
            producer_id: "test-producer".into(),
            incarnation: "incarnation-a".into(),
            sequence: sequence.into(),
        }
    }

    fn submitted(task: &str) -> DomainEvent {
        let contract: TaskContract = serde_json::from_value(json!({
            "schema_version":1,"project_id":"project-a","root_id":"root-a","task_id":task,
            "revision":1,"title":"Atomic storage fixture","objective":"Persist event with effect intent",
            "non_goals":[],"owned_paths":["src/lib.rs"],"forbidden_paths":[],"read_first":[],"dependencies":[],
            "criteria":[{"id":"C1","text":"Commit atomically","check_ids":["unit"]}],
            "checks":[{"id":"unit","program":"cargo","args":["test"],"cwd":".","timeout_secs":10}],
            "authority_ref":"fixture:bounded","max_attempts":1,"simulated":true
        })).expect("valid task contract fixture");
        DomainEvent::TaskSubmitted { contract }
    }

    fn intent(key: &str, payload: serde_json::Value, not_before: u64) -> outbox::OutboxIntent {
        outbox::OutboxIntent {
            idempotency_key: key.into(),
            payload,
            not_before,
        }
    }

    fn counts(path: &std::path::Path) -> (i64, i64, i64) {
        let connection = Connection::open(path).expect("inspection connection");
        let events = connection
            .query_row("SELECT COUNT(*) FROM domain_events", [], |row| row.get(0))
            .expect("event count");
        let effects = connection
            .query_row("SELECT COUNT(*) FROM outbox_intents", [], |row| row.get(0))
            .expect("intent count");
        let links = connection
            .query_row(
                "SELECT COUNT(*) FROM event_outbox_associations",
                [],
                |row| row.get(0),
            )
            .expect("association count");
        (events, effects, links)
    }

    fn private_tempdir() -> tempfile::TempDir {
        let directory = tempfile::Builder::new()
            .prefix("adm-store-")
            .tempdir()
            .expect("private fixture directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("restrict fixture directory permissions");
        }
        directory
    }
    #[test]
    fn wal_fix_version_gate() {
        for version in [3_044_006, 3_044_007, 3_050_007, 3_051_003, 3_052_000] {
            assert!(fixed_sqlite(version));
        }
        for version in [3_044_005, 3_045_000, 3_050_006, 3_051_002] {
            assert!(!fixed_sqlite(version));
        }
    }

    #[test]
    fn exact_key_writer_claims_only_owned_effect_and_reopen_preserves_foreign_rows() {
        let directory = private_tempdir();
        let path = directory.path().join("scoped-claims.sqlite");
        let store = Store::open(&path).expect("actual sole writer");
        for (sequence, task, key) in [
            ("1", "foreign-expired", "project-b:expired"),
            ("2", "foreign-pending", "project-b:pending"),
            ("3", "ours", "project-a:ours"),
        ] {
            let mut event = submitted(task);
            if let DomainEvent::TaskSubmitted { contract } = &mut event {
                contract.project_id = admiralty_core::ProjectId::new(if task == "ours" {
                    "project-a"
                } else {
                    "project-b"
                })
                .expect("project");
            }
            store
                .append_with_intent(
                    source(sequence),
                    event,
                    "recorded".into(),
                    intent(key, json!({"task":task}), 0),
                )
                .expect("atomic event/effect");
        }
        let foreign_claim = store
            .claim_outbox_key(
                Some(0),
                "project-b-owner".into(),
                1,
                "project-b:expired".into(),
            )
            .expect("foreign claim")
            .expect("claim");
        let owned = store
            .claim_outbox_key(
                Some(2),
                "project-a-owner".into(),
                5,
                "project-a:ours".into(),
            )
            .expect("scoped writer claim")
            .expect("ours");
        assert_eq!(owned.idempotency_key, "project-a:ours");
        assert!(
            store
                .claim_outbox_key(Some(2), "same-owner".into(), 5, "project-a:ours".into())
                .expect("held claim")
                .is_none()
        );
        assert!(
            store
                .claim_outbox_key(Some(2), "owner".into(), 5, "bad\nkey".into())
                .is_err()
        );
        store
            .complete_outbox(owned.token.clone(), "actual-receipt".into(), Some(3))
            .expect("owned completion");
        let before = store.snapshot().expect("domain unchanged by claims");
        drop(store);
        let connection = Connection::open(&path).expect("inspect closed writer");
        let foreign_state: String = connection
            .query_row(
                "SELECT state FROM outbox_intents WHERE idempotency_key='project-b:expired'",
                [],
                |r| r.get(0),
            )
            .expect("foreign expired state");
        assert_eq!(
            foreign_state, "claimed",
            "exact-key polling never expires another project's handle"
        );
        let pending: String = connection
            .query_row(
                "SELECT state FROM outbox_intents WHERE idempotency_key='project-b:pending'",
                [],
                |r| r.get(0),
            )
            .expect("foreign pending state");
        assert_eq!(pending, "pending");
        drop(connection);
        let reopened = Store::open(&path).expect("reopen actual writer");
        assert_eq!(reopened.snapshot().expect("same projection"), before);
        reopened
            .complete_outbox(owned.token, "actual-receipt".into(), Some(3))
            .expect("exact completion replay after reopen");
        assert!(
            reopened
                .claim_outbox_key(Some(3), "owner".into(), 5, "project-a:ours".into())
                .expect("completed never redispatched")
                .is_none()
        );
        assert!(
            reopened
                .claim_outbox_key(Some(3), "owner".into(), 5, "project-b:expired".into())
                .expect("only explicitly selected foreign expiry")
                .is_none()
        );
        assert!(matches!(
            reopened.complete_outbox(foreign_claim.token, "late".into(), Some(3)),
            Err(super::StoreError::Outbox(
                outbox::OutboxError::ReconciliationRequired
            ))
        ));
        assert_eq!(
            reopened
                .claim_outbox_key(
                    Some(3),
                    "project-b-owner".into(),
                    5,
                    "project-b:pending".into()
                )
                .expect("separate explicit effect")
                .expect("foreign remains runnable")
                .idempotency_key,
            "project-b:pending"
        );
    }

    #[test]
    fn store_open_initializes_outbox_and_reopen_replay_do_not_dispatch_it() {
        let directory = private_tempdir();
        let path = directory.path().join("store.sqlite");
        let store = Store::open(&path).expect("normal store initializes migrations");
        let initial = store.snapshot().expect("initial snapshot");
        drop(store);
        // The Store is closed before this explicit helper fixture writes rows.
        let mut fixture = rusqlite::Connection::open(&path).expect("fixture connection");
        let tables: i64 = fixture.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('outbox_clock','outbox_intents','outbox_integrity_incidents')", [], |row| row.get(0)).expect("outbox tables");
        assert_eq!(tables, 3);
        let atomic_table: i64 = fixture.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='event_outbox_associations'", [], |row| row.get(0)).expect("atomic association migration");
        assert_eq!(atomic_table, 1);
        let user_version: i64 = fixture
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("supported schema version");
        assert_eq!(
            user_version, 1,
            "component migration preserves integration-owned version"
        );
        let claimed_id = outbox::enqueue(
            &mut fixture,
            outbox::OutboxIntent {
                idempotency_key: "claimed".into(),
                payload: serde_json::json!({"reference":"first"}),
                not_before: 0,
            },
        )
        .expect("first intent");
        let pending_id = outbox::enqueue(
            &mut fixture,
            outbox::OutboxIntent {
                idempotency_key: "pending".into(),
                payload: serde_json::json!({"reference":"second"}),
                not_before: 0,
            },
        )
        .expect("second intent");
        let claim = outbox::claim_due(&mut fixture, Some(0), "fixture-owner", 5)
            .expect("explicit reservation")
            .expect("claim");
        assert_eq!(claim.id, claimed_id);
        drop(fixture);

        let reopened = Store::open(&path).expect("normal reopen reruns idempotent migrations");
        assert_eq!(reopened.snapshot().expect("reopened snapshot"), initial);
        let rebuilt = reopened.rebuild().expect("pure event replay");
        assert_eq!(rebuilt.projection, initial.projection);
        assert_eq!(rebuilt.cursor.event_seq, "0");
        drop(reopened);
        let mut fixture = rusqlite::Connection::open(&path).expect("inspect outbox after replay");
        let rows: Vec<(String, String)> = fixture
            .prepare("SELECT id,state FROM outbox_intents ORDER BY id")
            .expect("states query")
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?.to_string(), row.get(1)?))
            })
            .expect("states")
            .collect::<rusqlite::Result<_>>()
            .expect("state rows");
        assert_eq!(
            rows,
            vec![
                (claimed_id.clone(), "claimed".into()),
                (pending_id.clone(), "pending".into())
            ]
        );
        let observed: i64 = fixture
            .query_row(
                "SELECT last_observed FROM outbox_clock WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .expect("recorded clock");
        assert_eq!(observed, 0, "startup and rebuild never invent a clock");
        let next = outbox::claim_due(&mut fixture, Some(6), "restarted-owner", 5)
            .expect("explicit recovery observation")
            .expect("next due intent");
        assert_eq!(next.id, pending_id);
        let old_state: String = fixture
            .query_row(
                "SELECT state FROM outbox_intents WHERE id=?1",
                [&claimed_id],
                |row| row.get(0),
            )
            .expect("old claim state");
        assert_eq!(old_state, "unknown");
    }

    #[test]
    fn atomic_append_exact_replay_reopen_and_rebuild_preserve_one_pending_association() {
        let directory = private_tempdir();
        let path = directory.path().join("atomic.sqlite");
        let store = Store::open(&path).expect("open store");
        let mut changes = store.subscribe_changes();
        let event = submitted("task-one");
        let effect = intent("effect-one", json!({"z":2,"a":1}), 15);
        let appended = store
            .append_with_intent(
                source("1"),
                event.clone(),
                "2026-01-01T00:00:00Z".into(),
                effect.clone(),
            )
            .expect("atomic append");
        assert!(!appended.duplicate);
        assert!(changes.has_changed().expect("watch status"));
        changes.borrow_and_update();
        let duplicate = store
            .append_with_intent(
                source("1"),
                event.clone(),
                "2026-01-01T00:00:00Z".into(),
                effect.clone(),
            )
            .expect("exact event/effect replay");
        assert!(duplicate.duplicate);
        assert!(!changes.has_changed().expect("duplicate does not notify"));
        assert_eq!(counts(&path), (1, 1, 1));
        drop(store);

        let reopened = Store::open(&path).expect("reopen");
        assert_eq!(
            reopened
                .snapshot()
                .expect("snapshot")
                .projection
                .tasks
                .len(),
            1
        );
        let rebuilt = reopened.rebuild().expect("pure projection rebuild");
        assert_eq!(rebuilt.projection.tasks.len(), 1);
        assert_eq!(counts(&path), (1, 1, 1));
        drop(reopened);
        let connection = Connection::open(&path).expect("inspect outbox");
        assert!(
            connection
                .execute(
                    "UPDATE event_outbox_associations SET not_before=16 WHERE event_seq=1",
                    []
                )
                .is_err(),
            "association identity is immutable"
        );
        let state: String = connection
            .query_row(
                "SELECT state FROM outbox_intents WHERE idempotency_key='effect-one'",
                [],
                |row| row.get(0),
            )
            .expect("pending effect");
        assert_eq!(state, "pending", "reopen/rebuild never claims or delivers");
    }

    #[test]
    fn event_replay_rejects_changed_effect_key_schedule_and_plain_mismatch() {
        let directory = private_tempdir();
        let path = directory.path().join("replay.sqlite");
        let store = Store::open(&path).expect("open store");
        let event = submitted("task-one");
        let original = intent("effect-one", json!({"reference":"a"}), 10);
        store
            .append_with_intent(
                source("1"),
                event.clone(),
                "time-a".into(),
                original.clone(),
            )
            .expect("append");
        let changed_payload = intent("effect-one", json!({"reference":"b"}), 10);
        let changed_key = intent("effect-two", json!({"reference":"a"}), 10);
        let changed_time = intent("effect-one", json!({"reference":"a"}), 11);
        for changed in [changed_payload, changed_key, changed_time] {
            assert!(matches!(
                store.append_with_intent(source("1"), event.clone(), "time-a".into(), changed),
                Err(super::StoreError::SourceCollision { .. })
            ));
        }
        assert!(
            matches!(
                store.append(source("1"), event.clone(), "time-a".into()),
                Err(super::StoreError::SourceCollision { .. })
            ),
            "plain replay cannot detach effect"
        );
        assert!(
            matches!(
                store.append_with_intent(
                    source("1"),
                    submitted("different-task"),
                    "time-a".into(),
                    original.clone()
                ),
                Err(super::StoreError::SourceCollision { .. })
            ),
            "changed event bytes collide even when the effect is unchanged"
        );
        assert!(
            store
                .append_with_intent(source("1"), event, "time-b".into(), original)
                .expect("fresh receive time does not change the original admission")
                .duplicate
        );
        assert_eq!(counts(&path), (1, 1, 1));
        let incidents: i64 = Connection::open(&path)
            .expect("incident connection")
            .query_row("SELECT COUNT(*) FROM integrity_incidents", [], |row| {
                row.get(0)
            })
            .expect("incident count");
        assert_eq!(incidents, 5);
    }

    #[test]
    fn plain_event_cannot_be_upgraded_to_effect_and_intent_cannot_bind_twice() {
        let directory = private_tempdir();
        let path = directory.path().join("binding.sqlite");
        let store = Store::open(&path).expect("open store");
        let first = submitted("task-one");
        store
            .append(source("1"), first.clone(), "time-a".into())
            .expect("plain append");
        assert!(matches!(
            store.append_with_intent(
                source("1"),
                first,
                "time-a".into(),
                intent("effect-one", json!({"x":1}), 0)
            ),
            Err(super::StoreError::SourceCollision { .. })
        ));
        let other = submitted("task-two");
        store
            .append_with_intent(
                source("2"),
                other.clone(),
                "time-b".into(),
                intent("effect-two", json!({"x":2}), 0),
            )
            .expect("second event intent");
        assert!(matches!(
            store.append_with_intent(
                source("3"),
                submitted("task-three"),
                "time-c".into(),
                intent("effect-two", json!({"x":2}), 0)
            ),
            Err(super::StoreError::Outbox(
                outbox::OutboxError::IdentityCollision
            ))
        ));
        assert_eq!(counts(&path), (2, 1, 1));
        let incident_count: i64 = Connection::open(&path)
            .expect("incidents connection")
            .query_row(
                "SELECT COUNT(*) FROM outbox_integrity_incidents",
                [],
                |row| row.get(0),
            )
            .expect("outbox incident count");
        assert_eq!(
            incident_count, 1,
            "reusing an already associated exact intent is a durable collision"
        );
    }

    #[test]
    fn invalid_reducer_transition_and_sql_association_failure_leave_no_partial_state_or_watch() {
        let directory = private_tempdir();
        let path = directory.path().join("rollback.sqlite");
        let store = Store::open(&path).expect("open store");
        let before = store.snapshot().expect("initial projection");
        let changes = store.subscribe_changes();
        assert!(matches!(
            store.append_with_intent(
                source("1"),
                DomainEvent::TaskStarted {
                    task_id: TaskId::new("missing").expect("task fixture"),
                    attempt_id: admiralty_core::AttemptId::new("attempt").expect("attempt fixture")
                },
                "time-a".into(),
                intent("invalid", json!({"x":1}), 0)
            ),
            Err(super::StoreError::Transition(_))
        ));
        assert_eq!(
            store.snapshot().expect("unchanged snapshot").projection,
            before.projection
        );
        assert!(
            !changes
                .has_changed()
                .expect("invalid transition does not notify")
        );
        assert_eq!(counts(&path), (0, 0, 0));

        Connection::open(&path).expect("trigger connection").execute_batch("CREATE TRIGGER reject_event_outbox_link BEFORE INSERT ON event_outbox_associations BEGIN SELECT RAISE(ABORT,'injected link failure'); END;").expect("install association fault");
        assert!(
            store
                .append_with_intent(
                    source("2"),
                    submitted("task-two"),
                    "time-b".into(),
                    intent("rollback-effect", json!({"x":2}), 0)
                )
                .is_err()
        );
        assert_eq!(
            store
                .snapshot()
                .expect("failed append leaves projection")
                .projection,
            before.projection
        );
        assert!(
            !changes
                .has_changed()
                .expect("failed SQL transaction does not notify")
        );
        assert_eq!(store.events_after(0, 10).expect("event query").len(), 0);
        assert_eq!(
            counts(&path),
            (0, 0, 0),
            "event and outbox insert both roll back"
        );
    }

    #[test]
    fn bounded_inputs_reject_oversized_key_payload_time_event_and_sqlite_counter() {
        let directory = private_tempdir();
        let path = directory.path().join("bounds.sqlite");
        let store = Store::open(&path).expect("open store");
        let baseline = submitted("task-one");
        let oversized_key = intent(&"k".repeat(257), json!({"x":1}), 0);
        assert!(matches!(
            store.append_with_intent(source("1"), baseline.clone(), "time".into(), oversized_key),
            Err(super::StoreError::Outbox(outbox::OutboxError::Invalid(_)))
        ));
        let oversized_payload = intent(
            "large-payload",
            json!("x".repeat(outbox::MAX_PAYLOAD_BYTES)),
            0,
        );
        assert!(matches!(
            store.append_with_intent(
                source("1"),
                baseline.clone(),
                "time".into(),
                oversized_payload
            ),
            Err(super::StoreError::Outbox(outbox::OutboxError::Invalid(_)))
        ));
        assert!(matches!(
            store.append_with_intent(
                source("1"),
                baseline.clone(),
                "t".repeat(129),
                intent("long-time", json!({}), 0)
            ),
            Err(super::StoreError::Invalid(_))
        ));
        assert!(matches!(
            store.append_with_intent(
                source("1"),
                baseline.clone(),
                "time".into(),
                intent("counter-overflow", json!({}), u64::MAX)
            ),
            Err(super::StoreError::Outbox(outbox::OutboxError::Overflow))
        ));
        let oversized_event = DomainEvent::CandidateSubmitted {
            task_id: TaskId::new("task-one").expect("task fixture"),
            attempt_id: admiralty_core::AttemptId::new("attempt").expect("attempt fixture"),
            candidate_ref: "x".repeat(1024 * 1024),
        };
        assert!(matches!(
            store.append(source("1"), oversized_event, "time".into()),
            Err(super::StoreError::Invalid(_))
        ));
        assert!(
            matches!(
                store.append(source("9223372036854775808"), baseline, "time".into()),
                Err(super::StoreError::Invalid(_))
            ),
            "u64 source sequences outside SQLite's signed range are rejected"
        );
        assert_eq!(counts(&path), (0, 0, 0));
    }

    #[test]
    fn restart_and_cursor_reads_reject_source_or_effect_binding_tampering() {
        for mutation in [
            "DROP TRIGGER event_immutable_update; UPDATE domain_events SET observed_utc='changed'",
            "DROP TRIGGER event_immutable_update; UPDATE domain_events SET producer_id='other-producer'",
            "DROP TRIGGER event_immutable_update; UPDATE domain_events SET producer_seq=2",
            "DROP TRIGGER event_request_binding_immutable_delete; DELETE FROM event_request_bindings",
            "DROP TRIGGER event_outbox_association_immutable_delete; DELETE FROM event_outbox_associations",
            "DROP TRIGGER event_outbox_association_immutable_update; UPDATE event_outbox_associations SET not_before=99",
        ] {
            let directory = private_tempdir();
            let path = directory.path().join("tampered.sqlite");
            let store = Store::open(&path).expect("owned store");
            store
                .append_with_intent(
                    source("1"),
                    submitted("protected"),
                    "recorded".into(),
                    intent("effect", json!({"x":1}), 3),
                )
                .expect("atomic append");
            let observer = Connection::open(&path).expect("own fault connection");
            assert!(
                observer
                    .execute("UPDATE domain_events SET observed_utc='forged'", [])
                    .is_err(),
                "normal updates are immutable"
            );
            observer
                .execute_batch(mutation)
                .expect("explicit offline fault injection");
            assert!(
                store.events_after(0, 1).is_err(),
                "cursor read must validate source and effect bindings: {mutation}"
            );
            drop(observer);
            drop(store);
            assert!(
                Store::open(&path).is_err(),
                "startup must reject tampered evidence: {mutation}"
            );
        }
    }

    #[test]
    fn request_binding_insert_failure_rolls_back_every_half_and_watch() {
        let directory = private_tempdir();
        let path = directory.path().join("fault.sqlite");
        let store = Store::open(&path).expect("owned store");
        let before = store.snapshot().expect("before");
        let changes = store.subscribe_changes();
        let fixture = Connection::open(&path).expect("own fault connection");
        fixture.execute_batch("CREATE TRIGGER reject_request_binding BEFORE INSERT ON event_request_bindings BEGIN SELECT RAISE(ABORT,'binding failure'); END").expect("install own fault");
        assert!(
            store
                .append_with_intent(
                    source("1"),
                    submitted("rollback"),
                    "recorded".into(),
                    intent("effect", json!({"x":1}), 3)
                )
                .is_err()
        );
        assert_eq!(counts(&path), (0, 0, 0));
        assert_eq!(store.snapshot().expect("after rollback"), before);
        assert!(!changes.has_changed().expect("watch remains open"));
        let watermarks: i64 = fixture
            .query_row("SELECT count(*) FROM producer_watermarks", [], |row| {
                row.get(0)
            })
            .expect("watermark count");
        assert_eq!(watermarks, 0);
        fixture
            .execute_batch("DROP TRIGGER reject_request_binding")
            .expect("remove own fault");
        let receipt = store
            .append_with_intent(
                source("1"),
                submitted("rollback"),
                "recorded".into(),
                intent("effect", json!({"x":1}), 3),
            )
            .expect("same identity can commit after rollback");
        assert_eq!(receipt.event_seq, "1");
        assert_eq!(receipt.source_contiguous_seq, "1");
    }

    #[test]
    fn competing_atomic_appends_commit_only_one_event_for_one_effect() {
        let directory = private_tempdir();
        let path = directory.path().join("race.sqlite");
        let store = Store::open(&path).expect("owned store");
        let start = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let mut workers = Vec::new();
            for name in ["worker-a", "worker-b"] {
                let owned = store.clone();
                let start = &start;
                workers.push(scope.spawn(move || {
                    start.wait();
                    let mut identity = source("1");
                    identity.producer_id = name.into();
                    owned.append_with_intent(
                        identity,
                        submitted(name),
                        "recorded".into(),
                        intent("shared-effect", json!({"x":1}), 3),
                    )
                }));
            }
            workers
                .into_iter()
                .map(|worker| worker.join().expect("owned thread finishes"))
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(super::StoreError::Outbox(
                        outbox::OutboxError::IdentityCollision
                    ))
                ))
                .count(),
            1
        );
        assert_eq!(counts(&path), (1, 1, 1));
        let snapshot = store.snapshot().expect("committed snapshot");
        assert_eq!(snapshot.projection.tasks.len(), 1);
        drop(store);
        assert_eq!(
            Store::open(&path)
                .expect("reopen")
                .snapshot()
                .expect("snapshot")
                .projection,
            snapshot.projection
        );
    }

    #[test]
    fn legacy_events_have_an_explicit_upgrade_boundary_and_new_events_require_bindings() {
        let directory = private_tempdir();
        let path = directory.path().join("legacy.sqlite");
        let connection = Connection::open(&path).expect("owned legacy fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .expect("private legacy database");
        }
        connection
            .execute_batch(include_str!("../migrations/0001_foundation.sql"))
            .expect("legacy schema");
        let payload = serde_json::to_string(&submitted("legacy")).expect("legacy event");
        connection.execute("INSERT INTO domain_events(producer_id,incarnation,producer_seq,digest,observed_utc,payload) VALUES('test-producer','incarnation-a',1,?1,'legacy-time',?2)",rusqlite::params![super::digest(&payload),payload]).expect("legacy append");
        connection
            .execute(
                "INSERT INTO producer_watermarks VALUES('test-producer','incarnation-a',1)",
                [],
            )
            .expect("legacy watermark");
        drop(connection);
        let store = Store::open(&path).expect("upgrade original store");
        store
            .append(source("1"), submitted("legacy"), "legacy-time".into())
            .expect("legacy exact retry");
        store
            .append(source("2"), submitted("new"), "new-time".into())
            .expect("new bounded append");
        drop(store);
        let connection = Connection::open(&path).expect("inspect own upgrade");
        let boundary: String = connection
            .query_row(
                "SELECT value FROM metadata WHERE key='atomic_effects_legacy_event_seq'",
                [],
                |row| row.get(0),
            )
            .expect("recorded boundary");
        assert_eq!(boundary, "1");
        let count: i64 = connection
            .query_row("SELECT count(*) FROM event_request_bindings", [], |row| {
                row.get(0)
            })
            .expect("binding count");
        assert_eq!(count, 1);
        assert!(
            connection
                .execute(
                    "UPDATE metadata SET value='99' WHERE key='atomic_effects_legacy_event_seq'",
                    []
                )
                .is_err()
        );
        connection.execute_batch("DROP TRIGGER event_request_binding_immutable_delete;DELETE FROM event_request_bindings").expect("inject missing new binding");
        drop(connection);
        assert!(Store::open(&path).is_err());
    }

    #[test]
    fn fresh_receive_timestamps_replay_plain_and_effectful_events_without_rewriting_admission() {
        let directory = private_tempdir();
        let path = directory.path().join("receive-retry.sqlite");
        let store = Store::open(&path).expect("owned store");
        let plain = submitted("plain-retry");
        let effectful = submitted("effectful-retry");
        let effect = intent("retry-effect", json!({"work":"candidate-only"}), 7);
        let plain_receipt = store
            .append(source("1"), plain.clone(), "first-plain-receive".into())
            .expect("first plain admission");
        let effect_receipt = store
            .append_with_intent(
                source("2"),
                effectful.clone(),
                "first-effect-receive".into(),
                effect.clone(),
            )
            .expect("first effect admission");
        let before = store.snapshot().expect("original projection");
        let changes = store.subscribe_changes();
        let retried_plain = store
            .append(source("1"), plain.clone(), "later-plain-receive".into())
            .expect("exact plain retry with fresh transport observation");
        let retried_effect = store
            .append_with_intent(
                source("2"),
                effectful.clone(),
                "later-effect-receive".into(),
                effect.clone(),
            )
            .expect("exact effect retry with fresh transport observation");
        assert!(retried_plain.duplicate && retried_effect.duplicate);
        assert_eq!(retried_plain.event_seq, plain_receipt.event_seq);
        assert_eq!(retried_effect.event_seq, effect_receipt.event_seq);
        assert_eq!(store.snapshot().expect("after retries"), before);
        assert_eq!(counts(&path), (2, 1, 1));
        assert!(
            !changes
                .has_changed()
                .expect("duplicate emits no watch hint")
        );
        let observed: Vec<_> = store
            .events_after(0, 2)
            .expect("checked original records")
            .into_iter()
            .map(|record| record.observed_utc)
            .collect();
        assert_eq!(observed, ["first-plain-receive", "first-effect-receive"]);
        drop(store);
        let reopened = Store::open(&path)
            .expect("first-admission request bindings remain valid across reopen");
        let changes = reopened.subscribe_changes();
        assert!(
            reopened
                .append(source("1"), plain, "after-reopen-plain".into())
                .expect("plain retry after reopen")
                .duplicate
        );
        assert!(
            reopened
                .append_with_intent(source("2"), effectful, "after-reopen-effect".into(), effect)
                .expect("effect retry after reopen")
                .duplicate
        );
        assert_eq!(reopened.snapshot().expect("after reopened retries"), before);
        assert!(
            !changes
                .has_changed()
                .expect("reopened duplicate emits no watch hint")
        );
        assert_eq!(counts(&path), (2, 1, 1));
        let observed: Vec<_> = reopened
            .events_after(0, 2)
            .expect("stored timestamp and request hash still validate")
            .into_iter()
            .map(|record| record.observed_utc)
            .collect();
        assert_eq!(observed, ["first-plain-receive", "first-effect-receive"]);
    }
}
