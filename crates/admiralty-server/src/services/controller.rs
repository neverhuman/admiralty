//! Bounded, fail-closed admission into the durable launch-intent queue.
//!
//! The controller records claims and typed outbox intents, then dispatches due
//! intents through the Store writer's fenced claim API. Runtime launch remains
//! injected so the server must supply its actual resource and execution gates.

use admiralty_core::{
    AttemptId, DomainEvent, LifecycleCommand, PrincipalId, SchedulingState, SourceIdentity, TaskId,
    TaskView, content_digest,
};
use admiralty_store::{LifecycleAuthority, Store, StoreError, outbox::OutboxIntent};
use serde::{Deserialize, Serialize};

const MAX_IDENTITY_BYTES: usize = 128;
const MAX_REFERENCE_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchRequest {
    pub source: SourceIdentity,
    pub task_id: TaskId,
    pub expected_contract_digest: String,
    pub attempt_id: AttemptId,
    pub owner: PrincipalId,
    pub run_id: String,
    pub context_digest: String,
    pub policy_digest: String,
    pub provider: String,
    pub model: String,
    pub observed_utc: String,
    pub not_before: u64,
}

/// A trusted in-process adapter checks current scoped admission and the live
/// resource reservation. Implementations must not derive this decision from a
/// worker payload, provider exit status, or deserialized grant.
pub trait CurrentAdmission {
    fn authorize_current(&self, task: &TaskView, request: &LaunchRequest) -> Result<(), String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DurableIntent {
    pub event_seq: String,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub outbox_key: String,
    pub duplicate: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DispatchIntent {
    pub schema_version: u32,
    pub task_id: String,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub attempt_id: String,
    pub owner: String,
    pub run_id: String,
    pub context_digest: String,
    pub policy_digest: String,
    pub provider: String,
    pub model: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeDisposition {
    Candidate { candidate_digest: String },
    Failed { evidence_ref: String },
    Unknown { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeReceipt {
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub run_id: String,
    pub receipt_ref: String,
    pub disposition: RuntimeDisposition,
}

/// The implementation must use the configured P13 resource gate and P08/P14
/// trusted execution boundary. This trait is process-local and is not a grant.
pub trait NativeRuntimeExecutor {
    fn execute(&self, task: &TaskView, intent: &DispatchIntent) -> Result<RuntimeReceipt, String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DispatchOutcome {
    Idle,
    Completed {
        task_id: TaskId,
        attempt_id: AttemptId,
        receipt_ref: String,
    },
    Failed {
        task_id: TaskId,
        attempt_id: AttemptId,
        receipt_ref: String,
    },
    Unknown {
        reason: String,
    },
}

#[derive(Debug)]
pub enum ControllerError {
    Store(StoreError),
    UnknownTask,
    StaleContract,
    TaskAlreadyOwned,
    AdmissionDenied(String),
    InvalidRequest,
    Digest,
    DispatchInvalid,
}

/// Apply a frozen I1 lifecycle command through the Store's configured writer
/// authority. The writer checks current grants on every call, including replay,
/// and binds an optional effect to its exact key, payload, digest, and schedule.
/// Candidate decisions carry their own closure evidence; attempt closure remains
/// a separate command and is never synthesized from process exit or check output.
pub fn execute_lifecycle(
    store: &Store,
    source: SourceIdentity,
    command: LifecycleCommand,
    observed_utc: String,
    effect: Option<OutboxIntent>,
) -> Result<admiralty_store::AppendReceipt, ControllerError> {
    store
        .execute_lifecycle(source, command, observed_utc, effect)
        .map_err(ControllerError::Store)
}

/// Construct the Store with its process-local, trusted lifecycle authority.
/// This is a composition hook for the server owner; there is no permissive
/// default authority in this module.
pub fn open_authoritative_store(
    path: impl AsRef<std::path::Path>,
    authority: std::sync::Arc<dyn LifecycleAuthority>,
) -> Result<Store, ControllerError> {
    Store::open_with_lifecycle_authority(path, authority).map_err(ControllerError::Store)
}

impl std::fmt::Display for ControllerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "store operation failed: {error}"),
            Self::UnknownTask => {
                formatter.write_str("task is absent from the current durable projection")
            }
            Self::StaleContract => {
                formatter.write_str("task contract changed since the request was prepared")
            }
            Self::TaskAlreadyOwned => {
                formatter.write_str("task has a live attempt other than this exact replay")
            }
            Self::AdmissionDenied(reason) => {
                write!(formatter, "current scoped admission denied: {reason}")
            }
            Self::InvalidRequest => {
                formatter.write_str("launch request is invalid or exceeds controller bounds")
            }
            Self::Digest => formatter.write_str("content digest could not be produced"),
            Self::DispatchInvalid => {
                formatter.write_str("claimed launch intent failed current binding checks")
            }
        }
    }
}

impl std::error::Error for ControllerError {}

impl From<StoreError> for ControllerError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// Claim and execute at most one due launch. The claim token fences both
/// completion and unknown outcomes. All malformed, stale, denied, or ambiguous
/// claimed intents become durable `unknown` and are never silently retried.
pub fn dispatch_one(
    store: &Store,
    now: u64,
    owner: String,
    lease_secs: u64,
    admission: &impl DispatchAdmission,
    runtime: &impl NativeRuntimeExecutor,
) -> Result<DispatchOutcome, ControllerError> {
    let Some(claim) = store.claim_outbox_due(Some(now), owner, lease_secs)? else {
        return Ok(DispatchOutcome::Idle);
    };
    let claimed_unknown = |reason: String| -> Result<DispatchOutcome, ControllerError> {
        store.fail_outbox_unknown(claim.token.clone(), reason.clone())?;
        Ok(DispatchOutcome::Unknown { reason })
    };
    let intent: DispatchIntent = match serde_json::from_value(claim.payload.clone()) {
        Ok(intent) => intent,
        Err(_) => return claimed_unknown("malformed launch intent".into()),
    };
    if content_digest(&claim.payload).ok().as_deref() != Some(claim.payload_digest.as_str()) {
        return claimed_unknown("claimed payload digest mismatch".into());
    }
    let task_id = match TaskId::new(intent.task_id.clone()) {
        Ok(task_id) => task_id,
        Err(_) => return claimed_unknown("invalid task identity in launch intent".into()),
    };
    let attempt_id = match AttemptId::new(intent.attempt_id.clone()) {
        Ok(attempt_id) => attempt_id,
        Err(_) => return claimed_unknown("invalid attempt identity in launch intent".into()),
    };
    if intent.schema_version != 1
        || claim.idempotency_key != format!("launch/{task_id}/{attempt_id}")
        || !valid_sha256(&intent.contract_digest)
        || !valid_sha256(&intent.context_digest)
        || !valid_sha256(&intent.policy_digest)
        || !valid_identity(&intent.owner, MAX_IDENTITY_BYTES)
        || !valid_identity(&intent.run_id, MAX_IDENTITY_BYTES)
        || !valid_identity(&intent.provider, MAX_IDENTITY_BYTES)
        || !valid_identity(&intent.model, MAX_IDENTITY_BYTES)
    {
        return claimed_unknown("launch intent identity or schema mismatch".into());
    }
    let snapshot = store.snapshot()?;
    let Some(task) = snapshot.projection.tasks.get(&task_id) else {
        return claimed_unknown("task no longer exists".into());
    };
    let digest = content_digest(&task.contract).map_err(|_| ControllerError::Digest)?;
    if task.state != SchedulingState::Claimed
        || task.attempt_id.as_ref().is_none_or(|id| id != &attempt_id)
        || task
            .owner
            .as_ref()
            .is_none_or(|owner| owner.as_str() != intent.owner)
        || task.contract.revision != intent.contract_revision
        || digest != intent.contract_digest
        || task.contract.task_id != task_id
    {
        return claimed_unknown("current task projection no longer matches launch intent".into());
    }
    if let Err(reason) = admission.authorize_dispatch(task, &intent) {
        return claimed_unknown(format!("current dispatch admission denied: {reason}"));
    }
    let receipt = match runtime.execute(task, &intent) {
        Ok(receipt) => receipt,
        Err(reason) => {
            return claimed_unknown(format!("runtime outcome requires reconciliation: {reason}"));
        }
    };
    if receipt.task_id != task_id
        || receipt.attempt_id != attempt_id
        || receipt.run_id != intent.run_id
        || !valid_identity(&receipt.receipt_ref, 1024)
    {
        return claimed_unknown("runtime receipt binding mismatch".into());
    }
    match receipt.disposition {
        RuntimeDisposition::Candidate { candidate_digest } => {
            if !valid_sha256(&candidate_digest) {
                return claimed_unknown("runtime candidate digest is invalid".into());
            }
            store.complete_outbox(claim.token, receipt.receipt_ref.clone(), Some(now))?;
            Ok(DispatchOutcome::Completed {
                task_id,
                attempt_id,
                receipt_ref: receipt.receipt_ref,
            })
        }
        RuntimeDisposition::Failed { evidence_ref } => {
            if !valid_identity(&evidence_ref, 1024) {
                return claimed_unknown("runtime failure evidence reference is invalid".into());
            }
            store.complete_outbox(claim.token, receipt.receipt_ref.clone(), Some(now))?;
            Ok(DispatchOutcome::Failed {
                task_id,
                attempt_id,
                receipt_ref: receipt.receipt_ref,
            })
        }
        RuntimeDisposition::Unknown { reason } => {
            claimed_unknown(format!("runtime outcome unknown: {reason}"))
        }
    }
}

pub trait DispatchAdmission {
    fn authorize_dispatch(&self, task: &TaskView, intent: &DispatchIntent) -> Result<(), String>;
}

fn valid_identity(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..].bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct LaunchIntentPayload<'a> {
    schema_version: u32,
    task_id: &'a str,
    contract_revision: u32,
    contract_digest: &'a str,
    attempt_id: &'a str,
    owner: &'a str,
    run_id: &'a str,
    context_digest: &'a str,
    policy_digest: &'a str,
    provider: &'a str,
    model: &'a str,
}

/// Perform one bounded admission tick. The Store reducer rechecks that the
/// task is still ready in the same transaction that persists TaskClaimed and
/// its idempotent outbox intent. This stage only queues work; it never spawns.
pub fn admit_one(
    store: &Store,
    admission: &impl CurrentAdmission,
    request: &LaunchRequest,
) -> Result<DurableIntent, ControllerError> {
    validate_request(request)?;
    let snapshot = store.snapshot()?;
    let task = snapshot
        .projection
        .tasks
        .get(&request.task_id)
        .ok_or(ControllerError::UnknownTask)?;
    let digest = content_digest(&task.contract).map_err(|_| ControllerError::Digest)?;
    if digest != request.expected_contract_digest {
        return Err(ControllerError::StaleContract);
    }
    let exact_replay = task.state == SchedulingState::Claimed
        && task.attempt_id.as_ref() == Some(&request.attempt_id)
        && task.owner.as_ref() == Some(&request.owner);
    if task.state != SchedulingState::Ready && !exact_replay {
        return Err(ControllerError::TaskAlreadyOwned);
    }
    admission
        .authorize_current(task, request)
        .map_err(ControllerError::AdmissionDenied)?;

    let key = format!("launch/{}/{}", request.task_id, request.attempt_id);
    let payload = serde_json::to_value(LaunchIntentPayload {
        schema_version: 1,
        task_id: request.task_id.as_str(),
        contract_revision: task.contract.revision,
        contract_digest: &digest,
        attempt_id: request.attempt_id.as_str(),
        owner: request.owner.as_str(),
        run_id: &request.run_id,
        context_digest: &request.context_digest,
        policy_digest: &request.policy_digest,
        provider: &request.provider,
        model: &request.model,
    })
    .map_err(|_| ControllerError::InvalidRequest)?;
    let receipt = store.append_with_intent(
        request.source.clone(),
        DomainEvent::TaskClaimed {
            task_id: request.task_id.clone(),
            revision: task.contract.revision,
            attempt_id: request.attempt_id.clone(),
            owner: request.owner.clone(),
        },
        request.observed_utc.clone(),
        OutboxIntent {
            idempotency_key: key.clone(),
            payload,
            not_before: request.not_before,
        },
    )?;
    Ok(DurableIntent {
        event_seq: receipt.event_seq,
        task_id: request.task_id.clone(),
        attempt_id: request.attempt_id.clone(),
        outbox_key: key,
        duplicate: receipt.duplicate,
    })
}

fn validate_request(request: &LaunchRequest) -> Result<(), ControllerError> {
    let nonempty_bounded = |value: &str, max: usize| {
        !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
    };
    if request.source.validate().is_err()
        || !nonempty_bounded(&request.expected_contract_digest, 71)
        || request.expected_contract_digest.len() != 71
        || !request.expected_contract_digest.starts_with("sha256:")
        || !nonempty_bounded(&request.run_id, MAX_IDENTITY_BYTES)
        || !nonempty_bounded(&request.context_digest, 71)
        || request.context_digest.len() != 71
        || !request.context_digest.starts_with("sha256:")
        || !nonempty_bounded(&request.policy_digest, 71)
        || request.policy_digest.len() != 71
        || !request.policy_digest.starts_with("sha256:")
        || !nonempty_bounded(&request.provider, MAX_IDENTITY_BYTES)
        || !nonempty_bounded(&request.model, MAX_IDENTITY_BYTES)
        || !nonempty_bounded(&request.observed_utc, MAX_REFERENCE_BYTES)
    {
        return Err(ControllerError::InvalidRequest);
    }
    Ok(())
}

/// Explicit default-deny adapter for deployments without a qualified P14 and
/// resource-gate composition. Installing it guarantees that no task is queued.
pub struct DenyAdmission;

impl CurrentAdmission for DenyAdmission {
    fn authorize_current(&self, _task: &TaskView, _request: &LaunchRequest) -> Result<(), String> {
        Err("no trusted admission and resource adapter is configured".into())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use admiralty_core::{
        AcceptanceCriterion, CheckSpec, LifecycleAction, LifecycleCommand, ProjectId, RootId,
        TaskContract,
    };
    use admiralty_store::{LifecycleAuthority, LifecycleEffect};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::{fs, path::PathBuf};

    struct PermitAdmission;
    impl CurrentAdmission for PermitAdmission {
        fn authorize_current(
            &self,
            task: &TaskView,
            request: &LaunchRequest,
        ) -> Result<(), String> {
            if task.contract.project_id.as_str() == "controller-project"
                && task.contract.root_id.as_str() == "controller-root"
                && task.contract.task_id == request.task_id
                && request.run_id == "round22-fake-child-run"
            {
                Ok(())
            } else {
                Err("scope mismatch".into())
            }
        }
    }

    struct DispatchPermit;
    impl DispatchAdmission for DispatchPermit {
        fn authorize_dispatch(
            &self,
            task: &TaskView,
            intent: &DispatchIntent,
        ) -> Result<(), String> {
            if task.contract.task_id.as_str() == intent.task_id
                && task.contract.project_id.as_str() == "controller-project"
                && intent.provider == "fake"
            {
                Ok(())
            } else {
                Err("scope mismatch".into())
            }
        }
    }

    struct DenyDispatch;
    impl DispatchAdmission for DenyDispatch {
        fn authorize_dispatch(
            &self,
            _task: &TaskView,
            _intent: &DispatchIntent,
        ) -> Result<(), String> {
            Err("no live permit".into())
        }
    }

    struct FakeRuntime {
        disposition: RuntimeDisposition,
    }
    impl NativeRuntimeExecutor for FakeRuntime {
        fn execute(
            &self,
            _task: &TaskView,
            intent: &DispatchIntent,
        ) -> Result<RuntimeReceipt, String> {
            Ok(RuntimeReceipt {
                task_id: TaskId::new(intent.task_id.clone()).map_err(|error| error.to_string())?,
                attempt_id: AttemptId::new(intent.attempt_id.clone())
                    .map_err(|error| error.to_string())?,
                run_id: intent.run_id.clone(),
                receipt_ref: "receipt:fake-child-1".into(),
                disposition: self.disposition.clone(),
            })
        }
    }

    struct ExactCloseAuthority;
    impl LifecycleAuthority for ExactCloseAuthority {
        fn authorize(
            &self,
            source: &SourceIdentity,
            command: &LifecycleCommand,
            task: &TaskView,
            effect: Option<&LifecycleEffect<'_>>,
        ) -> bool {
            source.producer_id == "controller-test"
                && task.contract.task_id == command.task_id
                && matches!(
                    &command.action,
                    LifecycleAction::CloseAttempt {
                        attempt_id,
                        closure_digest,
                        reason,
                        cancelled: false,
                    } if attempt_id.as_str() == "controller-attempt"
                        && closure_digest == &format!("sha256:{}", "d".repeat(64))
                        && reason == "supervisor confirmed closed"
                )
                && effect.is_some_and(|effect| {
                    effect.idempotency_key == "controller/close-followup"
                        && effect.payload == r#"{"audit":"closed"}"#
                        && effect.payload_digest.starts_with("sha256:")
                        && effect.intent_digest.starts_with("sha256:")
                        && effect.not_before == 25
                })
        }
    }

    fn contract() -> TaskContract {
        TaskContract {
            schema_version: 1,
            project_id: ProjectId::new("controller-project").expect("project ID"),
            root_id: RootId::new("controller-root").expect("root ID"),
            task_id: TaskId::new("controller-task").expect("task ID"),
            revision: 1,
            title: "Durable controller fixture".into(),
            objective: "Persist one bounded launch intent".into(),
            non_goals: vec![],
            owned_paths: vec!["src/lib.rs".into()],
            forbidden_paths: vec![],
            read_first: vec![],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "C1".into(),
                text: "Intent is durable".into(),
                check_ids: vec!["unit".into()],
            }],
            checks: vec![CheckSpec {
                id: "unit".into(),
                program: "cargo".into(),
                args: vec!["test".into()],
                cwd: ".".into(),
                timeout_secs: 10,
            }],
            authority_ref: "grant:test-candidate".into(),
            max_attempts: 1,
            simulated: true,
        }
    }

    fn request() -> LaunchRequest {
        LaunchRequest {
            source: SourceIdentity {
                producer_id: "controller-test".into(),
                incarnation: "fixed-incarnation".into(),
                sequence: "2".into(),
            },
            task_id: TaskId::new("controller-task").expect("task ID"),
            expected_contract_digest: content_digest(&contract()).expect("contract digest"),
            attempt_id: AttemptId::new("controller-attempt").expect("attempt ID"),
            owner: PrincipalId::new("controller-owner").expect("owner ID"),
            run_id: "round22-fake-child-run".into(),
            context_digest: format!("sha256:{}", "a".repeat(64)),
            policy_digest: format!("sha256:{}", "b".repeat(64)),
            provider: "fake".into(),
            model: "deterministic".into(),
            observed_utc: "2026-10-18T20:00:00Z".into(),
            not_before: 10,
        }
    }

    fn database() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("admiralty-controller-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).expect("create private fixture directory");
        #[cfg(unix)]
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("secure fixture directory");
        root.join("controller.sqlite")
    }

    fn store_with_task(path: &PathBuf) -> Store {
        let store = Store::open(path).expect("open temporary controller store");
        store
            .append(
                SourceIdentity {
                    producer_id: "controller-test".into(),
                    incarnation: "fixed-incarnation".into(),
                    sequence: "1".into(),
                },
                DomainEvent::TaskSubmitted {
                    contract: contract(),
                },
                "2026-10-18T20:00:00Z".into(),
            )
            .expect("submit controller fixture");
        store
    }

    fn remove_database(path: &PathBuf) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("sqlite.writer.lock"));
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn actual_store_records_one_pending_intent_and_exact_replay_is_idempotent() {
        let path = database();
        let store = store_with_task(&path);
        let request = request();
        let first = admit_one(&store, &PermitAdmission, &request).expect("durable admission");
        assert!(!first.duplicate);
        let replay = admit_one(&store, &PermitAdmission, &request).expect("exact replay");
        assert!(replay.duplicate);
        assert_eq!(first.event_seq, replay.event_seq);
        assert_eq!(first.outbox_key, replay.outbox_key);
        assert_eq!(
            &store
                .snapshot()
                .expect("current durable state")
                .projection
                .tasks
                .get(&request.task_id)
                .expect("fixture task")
                .state,
            &SchedulingState::Claimed
        );
        drop(store);
        let reopened = Store::open(&path).expect("reopen durable controller store");
        assert_eq!(
            reopened
                .snapshot()
                .expect("replayed durable state")
                .projection
                .tasks
                .get(&request.task_id)
                .expect("reopened task")
                .attempt_id
                .as_ref(),
            Some(&request.attempt_id)
        );
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn default_deny_and_stale_contract_never_record_a_claim_or_intent() {
        let path = database();
        let store = store_with_task(&path);
        let mut request = request();
        assert!(matches!(
            admit_one(&store, &DenyAdmission, &request),
            Err(ControllerError::AdmissionDenied(_))
        ));
        request.expected_contract_digest = format!("sha256:{}", "c".repeat(64));
        assert!(matches!(
            admit_one(&store, &PermitAdmission, &request),
            Err(ControllerError::StaleContract)
        ));
        let state = store.snapshot().expect("unchanged durable state");
        assert!(matches!(
            state
                .projection
                .tasks
                .get(&request.task_id)
                .expect("task")
                .state,
            SchedulingState::Ready
        ));
        drop(store);
        let reopened = Store::open(&path).expect("reopen denied store");
        let reopened_state = reopened.snapshot().expect("denial left state unchanged");
        assert!(matches!(
            reopened_state
                .projection
                .tasks
                .get(&request.task_id)
                .expect("reopened task")
                .state,
            SchedulingState::Ready
        ));
        drop(reopened);
        remove_database(&path);
    }

    #[test]
    fn writer_claim_dispatches_one_runtime_candidate_without_accepting_task() {
        let path = database();
        let store = store_with_task(&path);
        let request = request();
        admit_one(&store, &PermitAdmission, &request).expect("durable admission");
        let result = dispatch_one(
            &store,
            10,
            "controller-worker".into(),
            30,
            &DispatchPermit,
            &FakeRuntime {
                disposition: RuntimeDisposition::Candidate {
                    candidate_digest: format!("sha256:{}", "e".repeat(64)),
                },
            },
        )
        .expect("dispatch");
        assert_eq!(
            result,
            DispatchOutcome::Completed {
                task_id: request.task_id.clone(),
                attempt_id: request.attempt_id.clone(),
                receipt_ref: "receipt:fake-child-1".into(),
            }
        );
        let task = store
            .snapshot()
            .expect("projection")
            .projection
            .tasks
            .get(&request.task_id)
            .expect("task")
            .clone();
        assert_eq!(
            task.state,
            SchedulingState::Claimed,
            "runtime candidate is not task acceptance"
        );
        assert_eq!(
            dispatch_one(
                &store,
                11,
                "next-worker".into(),
                30,
                &DispatchPermit,
                &FakeRuntime {
                    disposition: RuntimeDisposition::Candidate {
                        candidate_digest: format!("sha256:{}", "e".repeat(64))
                    }
                }
            )
            .expect("completed intent is not redelivered"),
            DispatchOutcome::Idle,
        );
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn denied_or_unknown_dispatch_is_durably_held_without_retry() {
        let path = database();
        let store = store_with_task(&path);
        let request = request();
        admit_one(&store, &PermitAdmission, &request).expect("durable admission");
        let result = dispatch_one(
            &store,
            10,
            "controller-worker".into(),
            30,
            &DenyDispatch,
            &FakeRuntime {
                disposition: RuntimeDisposition::Candidate {
                    candidate_digest: format!("sha256:{}", "e".repeat(64)),
                },
            },
        )
        .expect("denial becomes explicit unknown");
        assert!(matches!(result, DispatchOutcome::Unknown { .. }));
        assert_eq!(
            dispatch_one(
                &store,
                11,
                "next-worker".into(),
                30,
                &DispatchPermit,
                &FakeRuntime {
                    disposition: RuntimeDisposition::Unknown {
                        reason: "should not run".into()
                    }
                }
            )
            .expect("unknown claim is held"),
            DispatchOutcome::Idle
        );
        assert_eq!(
            store
                .snapshot()
                .expect("domain projection")
                .projection
                .tasks
                .get(&request.task_id)
                .expect("task")
                .state,
            SchedulingState::Claimed
        );
        drop(store);
        remove_database(&path);
    }

    #[test]
    fn frozen_lifecycle_writer_binds_exact_effect_and_separate_closure() {
        let path = database();
        let store = open_authoritative_store(&path, Arc::new(ExactCloseAuthority))
            .expect("open with trusted lifecycle authority");
        store
            .append(
                SourceIdentity {
                    producer_id: "controller-test".into(),
                    incarnation: "fixed-incarnation".into(),
                    sequence: "1".into(),
                },
                DomainEvent::TaskSubmitted {
                    contract: contract(),
                },
                "2026-10-18T20:00:00Z".into(),
            )
            .expect("submit lifecycle fixture");
        let request = request();
        admit_one(&store, &PermitAdmission, &request).expect("claim lifecycle fixture");
        let task = store
            .snapshot()
            .expect("current task")
            .projection
            .tasks
            .get(&request.task_id)
            .expect("task exists")
            .clone();
        let command = LifecycleCommand {
            protocol_version: 2,
            task_id: request.task_id.clone(),
            expected_task_digest: content_digest(&task).expect("I1 task view digest"),
            actor: request.owner.clone(),
            action: LifecycleAction::CloseAttempt {
                attempt_id: request.attempt_id.clone(),
                closure_digest: format!("sha256:{}", "d".repeat(64)),
                reason: "supervisor confirmed closed".into(),
                cancelled: false,
            },
        };
        let receipt = execute_lifecycle(
            &store,
            SourceIdentity {
                producer_id: "controller-test".into(),
                incarnation: "fixed-incarnation".into(),
                sequence: "3".into(),
            },
            command,
            "2026-10-18T20:00:01Z".into(),
            Some(OutboxIntent {
                idempotency_key: "controller/close-followup".into(),
                payload: serde_json::json!({"audit":"closed"}),
                not_before: 25,
            }),
        )
        .expect("I1 writer accepts exact authorized effect");
        assert!(!receipt.duplicate);
        assert_eq!(
            store
                .snapshot()
                .expect("closed lifecycle state")
                .projection
                .tasks
                .get(&request.task_id)
                .expect("closed task")
                .state,
            SchedulingState::Failed
        );
        drop(store);
        remove_database(&path);
    }
}
