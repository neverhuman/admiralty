//! Fail-closed adapter from admitted, prepared work to the durable session host.
//!
//! This boundary deliberately does not infer authority from a provider command,
//! workspace registration, or budget reservation. A caller must supply a current
//! policy validator that checks those bindings and the host resource permit.
use crate::{
    reservations::Reservation,
    session::{CaptureState, LaunchSpec, SessionError, SessionHost, SessionStatus},
    workspace::registry::{ValidatedWorkspace, WorkspaceSpec},
};
use admiralty_core::{AttemptId, ProjectId, RootId, SourceIdentity, TaskId};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io,
    path::PathBuf,
    sync::{Mutex, TryLockError},
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

/// Default observation budget, not authority to terminate or accept a task.
const DEFAULT_OBSERVATION_BUDGET: Duration = Duration::from_secs(30);
const OBSERVATION_POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ExecutionIdentity {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub contract_digest: String,
    pub context_digest: String,
    pub candidate_digest: String,
    pub verification_policy_digest: String,
    pub evidence_source: SourceIdentity,
    pub workspace: PathBuf,
    pub provider: String,
    pub model: String,
    pub run_id: String,
    pub resource_permit_ref: String,
}

/// Prepared argv plus its complete declared identity. This is not itself authority.
#[derive(Clone)]
pub struct PreparedCommand {
    identity: ExecutionIdentity,
    spec: LaunchSpec,
    environment: Vec<(OsString, OsString)>,
}
impl PreparedCommand {
    pub fn new(
        identity: ExecutionIdentity,
        spec: LaunchSpec,
        environment: Vec<(OsString, OsString)>,
    ) -> Result<Self, ExecutionError> {
        if identity.run_id != spec.run_id
            || identity.workspace != spec.cwd
            || identity.provider.trim().is_empty()
            || identity.model.trim().is_empty()
            || identity.resource_permit_ref.trim().is_empty()
            || [
                &identity.contract_digest,
                &identity.context_digest,
                &identity.candidate_digest,
                &identity.verification_policy_digest,
            ]
            .iter()
            .any(|s| s.trim().is_empty())
        {
            return Err(ExecutionError::InvalidBinding);
        }
        identity
            .evidence_source
            .validate()
            .map_err(|_| ExecutionError::InvalidBinding)?;
        crate::session::validate_launch_envelope(&spec)?;
        Ok(Self {
            identity,
            spec,
            environment,
        })
    }

    pub fn identity(&self) -> &ExecutionIdentity {
        &self.identity
    }

    pub fn run_id(&self) -> &str {
        &self.spec.run_id
    }

    pub fn launch_spec(&self) -> &LaunchSpec {
        &self.spec
    }

    /// Environment access is intended for the injected trusted admission check;
    /// values are never written to the session manifest or formatted in errors.
    pub fn environment(&self) -> &[(OsString, OsString)] {
        &self.environment
    }
}

/// Validation is intentionally injected: this crate has no authoritative live
/// policy/lease or CPU permit service to consult. Implementations must reject stale
/// or mismatched workspace, reservation, provider/model, and resource permit state.
pub trait ExecutionAdmissionValidator {
    fn validate_current(&self, request: &ExecutionRequest<'_>) -> Result<(), String>;
}

/// Request data is borrowed and cannot be deserialized into an authority grant.
pub struct ExecutionRequest<'a> {
    pub command: &'a PreparedCommand,
    pub workspace: &'a ValidatedWorkspace,
    pub reservation: &'a Reservation,
}

/// Opaque, process-local result of successful validation. It carries the exact
/// request binding and is neither serializable nor constructible by callers.
pub struct AdmissionProof {
    identity_digest: String,
    run_id: String,
}

#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("execution identity does not match the prepared command")]
    InvalidBinding,
    #[error("run identity has already been submitted to this adapter")]
    DuplicateAttempt,
    #[error("no in-process launch identity exists; prior runs cannot be adopted")]
    UnknownIdentity,
    #[error("execution adapter state is poisoned")]
    Internal,
    #[error("workspace validation failed: {0}")]
    Workspace(String),
    #[error("current execution admission denied: {0}")]
    Denied(String),
    #[error("session launch failed: {0}")]
    Session(#[from] SessionError),
    #[error("session output could not be read: {0}")]
    Io(#[from] io::Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionOutcome {
    CandidateSuccess {
        exit_code: Option<i32>,
        output_digest: String,
    },
    Failed {
        exit_code: Option<i32>,
        output_digest: String,
    },
    Unknown {
        reason: &'static str,
    },
}

/// Runtime evidence is bound to the candidate, policy, source, and task identity.
/// It never represents trusted-check success or lifecycle acceptance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeEvidence {
    identity: ExecutionIdentity,
    outcome: ExecutionOutcome,
}
impl RuntimeEvidence {
    pub fn identity(&self) -> &ExecutionIdentity {
        &self.identity
    }

    pub fn outcome(&self) -> &ExecutionOutcome {
        &self.outcome
    }
}

pub struct ExecutionAdapter<V> {
    host: SessionHost,
    validator: V,
    max_output_bytes: usize,
    launched: Mutex<BTreeMap<String, LaunchRecord>>,
}

#[derive(Clone)]
struct LaunchRecord {
    identity: ExecutionIdentity,
    state: LaunchState,
}

#[derive(Clone, Copy)]
enum LaunchState {
    Starting,
    Running,
    Unknown(&'static str),
}
impl<V: ExecutionAdmissionValidator> ExecutionAdapter<V> {
    pub fn new(validator: V, max_output_bytes: usize) -> Self {
        Self {
            host: SessionHost::new(),
            validator,
            max_output_bytes,
            launched: Mutex::new(BTreeMap::new()),
        }
    }

    /// Validates every current binding before the first spawn. The session host's
    /// durable run reservation makes retries with the same run token fail closed.
    pub fn launch(
        &self,
        command: &PreparedCommand,
        workspace_spec: &WorkspaceSpec,
        reservation: &Reservation,
    ) -> Result<(), ExecutionError> {
        self.launch_with_host(command, workspace_spec, reservation, || {
            self.host
                .launch_with_environment(command.spec.clone(), &command.environment)
        })
    }

    fn launch_with_host(
        &self,
        command: &PreparedCommand,
        workspace_spec: &WorkspaceSpec,
        reservation: &Reservation,
        launch_host: impl FnOnce() -> Result<(), SessionError>,
    ) -> Result<(), ExecutionError> {
        if command.spec.run_id != command.identity.run_id
            || workspace_spec.workspace != command.identity.workspace
            || reservation.scope.project_id != command.identity.project_id
            || reservation.scope.root_id != command.identity.root_id
            || reservation.scope.task_id != command.identity.task_id
            || reservation.scope.attempt_id != command.identity.attempt_id
            || reservation.scope.contract_digest != command.identity.contract_digest
            || !reservation.outstanding
            || reservation.stop_requested
        {
            return Err(ExecutionError::InvalidBinding);
        }
        let workspace = workspace_spec
            .validate()
            .map_err(|e| ExecutionError::Workspace(e.to_string()))?;
        if workspace.project_id != command.identity.project_id
            || workspace.root_id != command.identity.root_id
        {
            return Err(ExecutionError::InvalidBinding);
        }
        let proof = self.admit(ExecutionRequest {
            command,
            workspace: &workspace,
            reservation,
        })?;
        if proof.run_id != command.spec.run_id
            || proof.identity_digest
                != admiralty_core::content_digest(&command.identity)
                    .map_err(|_| ExecutionError::InvalidBinding)?
        {
            return Err(ExecutionError::InvalidBinding);
        }
        let run_id = command.identity.run_id.clone();
        {
            let mut launched = self.launched.lock().map_err(|_| ExecutionError::Internal)?;
            if launched.contains_key(&run_id) {
                return Err(ExecutionError::DuplicateAttempt);
            }
            launched.insert(
                run_id.clone(),
                LaunchRecord {
                    identity: command.identity.clone(),
                    state: LaunchState::Starting,
                },
            );
        }
        // Host invocation may have spawned before returning an error. Never
        // discard the identity or hold the adapter lock across those effects.
        let result = launch_host();
        let state = if result.is_ok() {
            LaunchState::Running
        } else {
            LaunchState::Unknown("session launch outcome is ambiguous")
        };
        match self.launched.lock() {
            Ok(mut launched) => {
                if let Some(record) = launched.get_mut(&run_id) {
                    record.state = state;
                }
            }
            Err(poisoned) => {
                if let Some(record) = poisoned.into_inner().get_mut(&run_id) {
                    record.state = LaunchState::Unknown("execution adapter state is poisoned");
                }
                return Err(ExecutionError::Internal);
            }
        }
        result.map_err(ExecutionError::Session)
    }

    fn admit(&self, request: ExecutionRequest<'_>) -> Result<AdmissionProof, ExecutionError> {
        self.validator
            .validate_current(&request)
            .map_err(ExecutionError::Denied)?;
        let identity_digest = admiralty_core::content_digest(&request.command.identity)
            .map_err(|_| ExecutionError::InvalidBinding)?;
        Ok(AdmissionProof {
            identity_digest,
            run_id: request.command.spec.run_id.clone(),
        })
    }

    /// Observe for at most a 30-second polling budget; acceptance remains a
    /// separate trusted-check decision. See `finish_until` for blocking limits.
    pub fn finish(&self, run_id: &str) -> Result<RuntimeEvidence, ExecutionError> {
        self.finish_until(run_id, Instant::now() + DEFAULT_OBSERVATION_BUDGET)
    }

    /// Close prepared stdin and observe until a monotonic deadline. Busy status
    /// is retried only within that budget; `wait` is never called for a live
    /// child or before complete capture EOF. Unknown is not acceptance or an
    /// automatic launch retry. A later observation may finish a timed-out run.
    ///
    /// This bounds intentional adapter polling and live-child waiting, not hard
    /// real time: input close, output reads, locks and filesystem/kernel calls
    /// can block. Exited-child finalization also retains SessionHost's bounded
    /// capture-drain grace. An unavailable identity returns an adapter error;
    /// evidence never invents the identity of an inaccessible run.
    pub fn finish_until(
        &self,
        run_id: &str,
        deadline: Instant,
    ) -> Result<RuntimeEvidence, ExecutionError> {
        self.finish_until_observing(run_id, deadline, || self.host.status(run_id))
    }

    fn finish_until_observing(
        &self,
        run_id: &str,
        deadline: Instant,
        mut observe: impl FnMut() -> Result<SessionStatus, SessionError>,
    ) -> Result<RuntimeEvidence, ExecutionError> {
        let record = loop {
            match self.launched.try_lock() {
                Ok(launched) => break launched.get(run_id).cloned(),
                Err(TryLockError::Poisoned(poisoned)) => {
                    let mut record = poisoned.into_inner().get(run_id).cloned();
                    if let Some(record) = &mut record {
                        record.state = LaunchState::Unknown("execution adapter state is poisoned");
                    }
                    break record;
                }
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    observation_pause(deadline);
                }
                Err(TryLockError::WouldBlock) => return Err(ExecutionError::Internal),
            }
        }
        .ok_or(ExecutionError::UnknownIdentity)?;
        match record.state {
            LaunchState::Starting => {
                return Ok(unknown_evidence(
                    record.identity,
                    "session launch is still in progress",
                ));
            }
            LaunchState::Unknown(reason) => return Ok(unknown_evidence(record.identity, reason)),
            LaunchState::Running => {}
        }
        if Instant::now() >= deadline {
            return Ok(unknown_evidence(
                record.identity,
                "execution observation deadline exceeded",
            ));
        }
        if self.host.close_input(run_id).is_err() {
            return Ok(unknown_evidence(
                record.identity,
                "session input close outcome is unavailable",
            ));
        }
        loop {
            if Instant::now() >= deadline {
                return Ok(unknown_evidence(
                    record.identity,
                    "execution observation deadline exceeded",
                ));
            }
            let status = match observe() {
                Ok(status) => status,
                Err(SessionError::Busy) => {
                    if Instant::now() >= deadline {
                        return Ok(unknown_evidence(
                            record.identity,
                            "session observation stayed busy until deadline",
                        ));
                    }
                    observation_pause(deadline);
                    continue;
                }
                Err(_) => {
                    return Ok(unknown_evidence(
                        record.identity,
                        "session status is unavailable",
                    ));
                }
            };
            if status.input_outcome_unknown || !status.input_closed {
                return Ok(unknown_evidence(
                    record.identity,
                    "session input close outcome is unavailable",
                ));
            }
            if status.capture_failed || status.capture_state == CaptureState::Failed {
                return Ok(unknown_evidence(record.identity, "capture failed"));
            }
            if status.output_offset > self.max_output_bytes as u64 {
                return Ok(unknown_evidence(record.identity, "output bound exceeded"));
            }
            if let Some((code, success)) = status.exit
                && status.capture_state == CaptureState::Complete
            {
                if Instant::now() >= deadline {
                    return Ok(unknown_evidence(
                        record.identity,
                        "execution observation deadline exceeded",
                    ));
                }
                let exit = match self.host.wait(run_id) {
                    Ok(exit) => exit,
                    Err(_) => {
                        return Ok(unknown_evidence(
                            record.identity,
                            "exited-session finalization failed",
                        ));
                    }
                };
                if exit.code() != code || exit.success() != success {
                    return Ok(unknown_evidence(
                        record.identity,
                        "exit observations are inconsistent",
                    ));
                }
                let outcome = self
                    .outcome(run_id, &status, code, success, deadline)
                    .unwrap_or(ExecutionOutcome::Unknown {
                        reason: "output observation failed",
                    });
                return Ok(RuntimeEvidence {
                    identity: record.identity,
                    outcome,
                });
            }
            observation_pause(deadline);
        }
    }

    fn outcome(
        &self,
        run_id: &str,
        status: &SessionStatus,
        code: Option<i32>,
        success: bool,
        deadline: Instant,
    ) -> Result<ExecutionOutcome, ExecutionError> {
        if status.capture_failed
            || status.capture_state != CaptureState::Complete
            || status.final_output_offset != Some(status.output_offset)
            || status.output_offset > self.max_output_bytes as u64
        {
            return Ok(ExecutionOutcome::Unknown {
                reason: "capture incomplete or output bound exceeded",
            });
        }
        let mut collected =
            Vec::with_capacity(status.output_offset.min(self.max_output_bytes as u64) as usize);
        let mut cursor = 0;
        while cursor < status.output_offset {
            if Instant::now() >= deadline {
                return Ok(ExecutionOutcome::Unknown {
                    reason: "execution observation deadline exceeded",
                });
            }
            let tail = self.host.tail(
                run_id,
                cursor,
                (self.max_output_bytes - cursor as usize).min(64 * 1024),
            )?;
            if tail.next_offset <= cursor || tail.next_offset > status.output_offset {
                return Ok(ExecutionOutcome::Unknown {
                    reason: "output journal made no progress",
                });
            }
            for chunk in tail.chunks {
                collected.extend_from_slice(&chunk.bytes);
            }
            cursor = tail.next_offset;
        }
        let digest = admiralty_core::content_digest(&collected)
            .map_err(|_| ExecutionError::InvalidBinding)?;
        if Instant::now() >= deadline {
            return Ok(ExecutionOutcome::Unknown {
                reason: "execution observation deadline exceeded",
            });
        }
        Ok(if success {
            ExecutionOutcome::CandidateSuccess {
                exit_code: code,
                output_digest: digest,
            }
        } else {
            ExecutionOutcome::Failed {
                exit_code: code,
                output_digest: digest,
            }
        })
    }
}

fn observation_pause(deadline: Instant) {
    thread::sleep(
        OBSERVATION_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
    );
}

fn unknown_evidence(identity: ExecutionIdentity, reason: &'static str) -> RuntimeEvidence {
    RuntimeEvidence {
        identity,
        outcome: ExecutionOutcome::Unknown { reason },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write, path::Path};

    // Child entrypoint invoked by the test harness as an exact, ignored test.
    #[test]
    #[ignore]
    fn deterministic_fake_child() {
        let path = std::env::args()
            .skip(1)
            .find(|s| s.starts_with("FAKE_MARKER="));
        if let Some(path) = path {
            let mut marker = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path.trim_start_matches("FAKE_MARKER="))
                .expect("owned spawn marker");
            marker.write_all(b"spawned\n").expect("spawn marker append");
        }
        if let Some(path) = std::env::args()
            .find_map(|argument| argument.strip_prefix("FAKE_RELEASE=").map(PathBuf::from))
        {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !path.exists() {
                assert!(
                    Instant::now() < deadline,
                    "test child release was not received"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
        println!("deterministic fake child output");
        if let Some(code) = std::env::args()
            .find_map(|argument| argument.strip_prefix("FAKE_EXIT=")?.parse::<i32>().ok())
        {
            std::io::stdout().flush().expect("child output flush");
            std::process::exit(code);
        }
    }

    struct Admit;
    impl ExecutionAdmissionValidator for Admit {
        fn validate_current(&self, _: &ExecutionRequest<'_>) -> Result<(), String> {
            Ok(())
        }
    }

    struct Deny;
    impl ExecutionAdmissionValidator for Deny {
        fn validate_current(&self, _: &ExecutionRequest<'_>) -> Result<(), String> {
            Err("no current resource permit".into())
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        command: PreparedCommand,
        workspace: WorkspaceSpec,
        reservation: Reservation,
        marker: PathBuf,
        release: PathBuf,
    }

    fn fixture(run_id: &str, blocked: bool, exit_code: Option<i32>) -> Fixture {
        use crate::{
            reservations::{AccountId, CurrencyUnit, MinorUnits, ReservationScope},
            workspace::registry::NodeRef,
        };
        use admiralty_core::PrincipalId;

        let temp = tempfile::tempdir().expect("private execution fixture");
        let path = fs::canonicalize(temp.path()).expect("canonical execution workspace");
        let marker = path.join("spawn-marker");
        let release = path.join("release");
        let identity = ExecutionIdentity {
            project_id: ProjectId::new("p").expect("project"),
            root_id: RootId::new("r").expect("root"),
            task_id: TaskId::new("t").expect("task"),
            attempt_id: AttemptId::new("a").expect("attempt"),
            contract_digest: "contract".into(),
            context_digest: "context".into(),
            candidate_digest: "candidate".into(),
            verification_policy_digest: "policy".into(),
            evidence_source: SourceIdentity {
                producer_id: "execution-adapter".into(),
                incarnation: "test".into(),
                sequence: "1".into(),
            },
            workspace: path.clone(),
            provider: "fake".into(),
            model: "fake-model".into(),
            run_id: run_id.into(),
            resource_permit_ref: "permit".into(),
        };
        let workspace = WorkspaceSpec {
            node_id: NodeRef::new("node").expect("node"),
            root_id: identity.root_id.clone(),
            project_id: identity.project_id.clone(),
            workspace: path.clone(),
            owned_paths: vec![PathBuf::from("src")],
            forbidden_paths: vec![],
        };
        let reservation = Reservation {
            id: "reservation".into(),
            scope: ReservationScope {
                project_id: identity.project_id.clone(),
                root_id: identity.root_id.clone(),
                task_id: identity.task_id.clone(),
                attempt_id: identity.attempt_id.clone(),
                owner: PrincipalId::new("worker").expect("owner"),
                incarnation: "boot-1".into(),
                contract_revision: 1,
                contract_digest: "contract".into(),
                authority_ref: "budget-grant".into(),
                resource_claims_digest: "claims".into(),
                account_id: AccountId::new("acct").expect("account"),
                unit: CurrencyUnit::new("USD", 2).expect("currency"),
            },
            correlation_id: "attempt-a".into(),
            revision: 1,
            original: MinorUnits::new(1),
            settled: MinorUnits::new(0),
            stop_requested: false,
            outstanding: true,
        };
        let mut args = vec![
            "--exact".into(),
            "execution::tests::deterministic_fake_child".into(),
            "--ignored".into(),
            "--nocapture".into(),
            "--".into(),
            format!("FAKE_MARKER={}", marker.display()),
        ];
        if blocked {
            args.push(format!("FAKE_RELEASE={}", release.display()));
        }
        if let Some(code) = exit_code {
            args.push(format!("FAKE_EXIT={code}"));
        }
        let spec = LaunchSpec {
            run_id: run_id.into(),
            program: std::env::current_exe().expect("owned test executable"),
            args,
            cwd: path.clone(),
            spool_dir: path.join("spool"),
        };
        let command = PreparedCommand::new(identity, spec, vec![]).expect("prepared fixture");
        Fixture {
            _temp: temp,
            command,
            workspace,
            reservation,
            marker,
            release,
        }
    }

    fn release_and_reap(adapter: &ExecutionAdapter<Admit>, fixture: &Fixture) {
        let run_id = fixture.command.run_id();
        fs::write(&fixture.release, b"release").expect("release owned child");
        adapter.host.close_input(run_id).expect("close owned input");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match adapter.host.status(run_id) {
                Ok(status) if status.exit.is_some() => break,
                Ok(_) | Err(SessionError::Busy) if Instant::now() < deadline => {
                    observation_pause(deadline)
                }
                other => panic!("owned child did not exit: {other:?}"),
            }
        }
        adapter
            .host
            .wait(run_id)
            .expect("finalize already exited owned child");
        assert!(
            adapter
                .host
                .status(run_id)
                .expect("final owned status")
                .exit
                .is_some()
        );
    }

    #[test]
    fn live_child_deadline_is_unknown_and_later_observation_completes() {
        let fixture = fixture("deadline-run", true, None);
        let adapter = ExecutionAdapter::new(Admit, 64 * 1024);
        adapter
            .launch(&fixture.command, &fixture.workspace, &fixture.reservation)
            .expect("launch owned child");
        adapter
            .host
            .close_input(fixture.command.run_id())
            .expect("prepare EOF before timing observation");
        let evidence = adapter
            .finish_until(
                fixture.command.run_id(),
                Instant::now() + Duration::from_millis(25),
            )
            .expect("deadline evidence");
        assert!(matches!(
            evidence.outcome(),
            ExecutionOutcome::Unknown { .. }
        ));
        assert!(
            adapter
                .host
                .status(fixture.command.run_id())
                .expect("owned child status")
                .exit
                .is_none()
        );
        fs::write(&fixture.release, b"release").expect("release owned child");
        let completed = adapter
            .finish_until(
                fixture.command.run_id(),
                Instant::now() + Duration::from_secs(5),
            )
            .expect("later complete evidence");
        assert_eq!(completed.identity(), fixture.command.identity());
        assert!(matches!(
            completed.outcome(),
            ExecutionOutcome::CandidateSuccess {
                exit_code: Some(0),
                ..
            }
        ));
        assert!(
            adapter
                .host
                .status(fixture.command.run_id())
                .expect("owned child reaped")
                .exit
                .is_some()
        );
    }

    #[test]
    fn busy_status_retries_only_to_deadline_then_can_be_observed_again() {
        let fixture = fixture("busy-run", true, None);
        let adapter = ExecutionAdapter::new(Admit, 64 * 1024);
        adapter
            .launch(&fixture.command, &fixture.workspace, &fixture.reservation)
            .expect("launch owned child");
        adapter
            .host
            .close_input(fixture.command.run_id())
            .expect("prepare EOF outside Busy polling budget");
        let mut calls = 0;
        let evidence = adapter
            .finish_until_observing(
                fixture.command.run_id(),
                Instant::now() + Duration::from_millis(100),
                || {
                    calls += 1;
                    Err(SessionError::Busy)
                },
            )
            .expect("busy deadline evidence");
        assert!(calls > 0);
        assert!(
            matches!(evidence.outcome(), ExecutionOutcome::Unknown { reason } if reason.contains("deadline"))
        );
        assert!(
            adapter
                .host
                .status(fixture.command.run_id())
                .expect("still owned live child")
                .exit
                .is_none()
        );
        fs::write(&fixture.release, b"release").expect("release owned child");
        let mut transient_busy = 0;
        let completed = adapter
            .finish_until_observing(
                fixture.command.run_id(),
                Instant::now() + Duration::from_secs(5),
                || {
                    transient_busy += 1;
                    if transient_busy <= 2 {
                        Err(SessionError::Busy)
                    } else {
                        adapter.host.status(fixture.command.run_id())
                    }
                },
            )
            .expect("busy recovery evidence");
        assert!(transient_busy > 2);
        assert!(matches!(
            completed.outcome(),
            ExecutionOutcome::CandidateSuccess { .. }
        ));
        assert!(
            adapter
                .host
                .status(fixture.command.run_id())
                .expect("owned child reaped")
                .exit
                .is_some()
        );
    }

    #[test]
    fn capture_and_observation_errors_leave_live_child_unknown() {
        let fixture = fixture("observation-error-run", true, None);
        let adapter = ExecutionAdapter::new(Admit, 64 * 1024);
        adapter
            .launch(&fixture.command, &fixture.workspace, &fixture.reservation)
            .expect("launch owned child");
        adapter
            .host
            .close_input(fixture.command.run_id())
            .expect("prepare durable EOF outside observation deadlines");
        let mut capture_observations = 0;
        let capture = adapter
            .finish_until_observing(
                fixture.command.run_id(),
                Instant::now() + Duration::from_secs(1),
                || {
                    capture_observations += 1;
                    let mut status = adapter.host.status(fixture.command.run_id())?;
                    status.capture_failed = true;
                    status.capture_state = CaptureState::Failed;
                    Ok(status)
                },
            )
            .expect("capture failure evidence");
        assert!(
            capture_observations > 0,
            "capture observer must run within its deadline; outcome was {:?}",
            capture.outcome()
        );
        assert!(
            matches!(
                capture.outcome(),
                ExecutionOutcome::Unknown {
                    reason: "capture failed"
                }
            ),
            "observed capture failure must remain Unknown with the capture-failed reason"
        );
        let unavailable = adapter
            .finish_until_observing(
                fixture.command.run_id(),
                Instant::now() + Duration::from_secs(1),
                || {
                    Err(SessionError::Io(io::Error::other(
                        "injected status read failure",
                    )))
                },
            )
            .expect("observation error evidence");
        assert!(
            matches!(
                unavailable.outcome(),
                ExecutionOutcome::Unknown {
                    reason: "session status is unavailable"
                }
            ),
            "unavailable status must remain Unknown without inventing an outcome"
        );
        release_and_reap(&adapter, &fixture);
    }

    #[test]
    fn post_invocation_poison_retains_unknown_and_duplicate_never_respawns() {
        let fixture = fixture("ambiguous-run", true, None);
        let adapter = ExecutionAdapter::new(Admit, 64 * 1024);
        let launched = adapter.launch_with_host(
            &fixture.command,
            &fixture.workspace,
            &fixture.reservation,
            || {
                let starting = adapter
                    .finish_until(
                        fixture.command.run_id(),
                        Instant::now() + Duration::from_millis(25),
                    )
                    .expect("Starting record is observable without waiting for launch");
                assert!(matches!(
                    starting.outcome(),
                    ExecutionOutcome::Unknown {
                        reason: "session launch is still in progress"
                    }
                ));
                adapter
                    .host
                    .launch_with_environment(fixture.command.spec.clone(), &[])?;
                Err(SessionError::Poisoned)
            },
        );
        assert!(matches!(
            launched,
            Err(ExecutionError::Session(SessionError::Poisoned))
        ));
        let evidence = adapter
            .finish_until(
                fixture.command.run_id(),
                Instant::now() + Duration::from_secs(1),
            )
            .expect("retained ambiguous evidence");
        assert_eq!(evidence.identity(), fixture.command.identity());
        assert!(matches!(
            evidence.outcome(),
            ExecutionOutcome::Unknown {
                reason: "session launch outcome is ambiguous"
            }
        ));
        assert!(matches!(
            adapter.launch(&fixture.command, &fixture.workspace, &fixture.reservation),
            Err(ExecutionError::DuplicateAttempt)
        ));
        release_and_reap(&adapter, &fixture);
        assert_eq!(
            fs::read(&fixture.marker).expect("one owned spawn"),
            b"spawned\n"
        );
        assert!(matches!(
            adapter
                .finish(fixture.command.run_id())
                .expect("ambiguity remains retained")
                .outcome(),
            ExecutionOutcome::Unknown { .. }
        ));
    }

    #[test]
    fn nonzero_exit_is_failed_and_capture_overflow_is_unknown_not_acceptance() {
        let failed = fixture("nonzero-run", false, Some(17));
        let adapter = ExecutionAdapter::new(Admit, 64 * 1024);
        adapter
            .launch(&failed.command, &failed.workspace, &failed.reservation)
            .expect("launch nonzero child");
        let evidence = adapter
            .finish_until(
                failed.command.run_id(),
                Instant::now() + Duration::from_secs(5),
            )
            .expect("failed evidence");
        assert!(matches!(
            evidence.outcome(),
            ExecutionOutcome::Failed {
                exit_code: Some(17),
                ..
            }
        ));
        assert!(
            adapter
                .host
                .status(failed.command.run_id())
                .expect("nonzero child reaped")
                .exit
                .is_some()
        );
        let overflow = fixture("overflow-run", false, None);
        let tiny = ExecutionAdapter::new(Admit, 1);
        tiny.launch(
            &overflow.command,
            &overflow.workspace,
            &overflow.reservation,
        )
        .expect("launch bounded capture child");
        let evidence = tiny
            .finish_until(
                overflow.command.run_id(),
                Instant::now() + Duration::from_secs(5),
            )
            .expect("overflow evidence");
        assert!(matches!(
            evidence.outcome(),
            ExecutionOutcome::Unknown { .. }
        ));
        release_and_reap(&tiny, &overflow);
    }

    #[test]
    fn denied_current_admission_creates_no_spool_or_child_marker() {
        use crate::{
            reservations::{AccountId, CurrencyUnit, MinorUnits, Reservation, ReservationScope},
            workspace::registry::{NodeRef, WorkspaceSpec},
        };
        use admiralty_core::{PrincipalId, ProjectId, RootId, TaskId};
        use std::{fs, path::Path};

        let temp = tempfile::tempdir().expect("temporary directory");
        let workspace = fs::canonicalize(temp.path()).expect("canonical fixture workspace");
        let marker = workspace.join("spawn-marker");
        let spool = workspace.join("denied-spool");
        let scope = ReservationScope {
            project_id: ProjectId::new("p").expect("project id"),
            root_id: RootId::new("r").expect("root id"),
            task_id: TaskId::new("t").expect("task id"),
            attempt_id: AttemptId::new("a").expect("attempt id"),
            owner: PrincipalId::new("worker").expect("owner id"),
            incarnation: "boot-1".into(),
            contract_revision: 1,
            contract_digest: "contract".into(),
            authority_ref: "budget-grant".into(),
            resource_claims_digest: "claims".into(),
            account_id: AccountId::new("acct").expect("account id"),
            unit: CurrencyUnit::new("USD", 2).expect("currency unit"),
        };
        let reservation = Reservation {
            id: "reservation".into(),
            scope,
            correlation_id: "attempt-a".into(),
            revision: 1,
            original: MinorUnits::new(1),
            settled: MinorUnits::new(0),
            stop_requested: false,
            outstanding: true,
        };
        let workspace_spec = WorkspaceSpec {
            node_id: NodeRef::new("node").expect("node id"),
            root_id: RootId::new("r").expect("root id"),
            project_id: ProjectId::new("p").expect("project id"),
            workspace: workspace.clone(),
            owned_paths: vec![Path::new("src").to_path_buf()],
            forbidden_paths: vec![],
        };
        let identity = ExecutionIdentity {
            project_id: ProjectId::new("p").expect("project id"),
            root_id: RootId::new("r").expect("root id"),
            task_id: TaskId::new("t").expect("task id"),
            attempt_id: AttemptId::new("a").expect("attempt id"),
            contract_digest: "contract".into(),
            context_digest: "context".into(),
            candidate_digest: "candidate".into(),
            verification_policy_digest: "policy".into(),
            evidence_source: SourceIdentity {
                producer_id: "execution-adapter".into(),
                incarnation: "test-incarnation".into(),
                sequence: "1".into(),
            },
            workspace: workspace.clone(),
            provider: "fake".into(),
            model: "fake-model".into(),
            run_id: "denied-attempt".into(),
            resource_permit_ref: "missing-permit".into(),
        };
        let binary = std::env::current_exe().expect("current test executable");
        let spec = LaunchSpec {
            run_id: identity.run_id.clone(),
            program: binary,
            args: vec![
                "--exact".into(),
                "execution::tests::deterministic_fake_child".into(),
                "--ignored".into(),
                "--nocapture".into(),
                "--".into(),
                format!("FAKE_MARKER={}", marker.display()),
            ],
            cwd: workspace,
            spool_dir: spool.clone(),
        };
        let command = PreparedCommand::new(identity, spec, vec![]).expect("prepared command");
        let adapter = ExecutionAdapter::new(Deny, 1024);
        assert!(matches!(
            adapter.launch(&command, &workspace_spec, &reservation),
            Err(ExecutionError::Denied(_))
        ));
        assert!(!marker.exists());
        assert!(!spool.exists());
    }

    #[test]
    fn output_classification_is_bounded_and_success_is_only_a_candidate() {
        let status = SessionStatus {
            pid: 1,
            start_identity: None,
            exit: Some((Some(0), true)),
            input_epoch: 0,
            input_outcome_unknown: false,
            input_closed: true,
            output_offset: 0,
            capture_failed: false,
            capture_state: CaptureState::Complete,
            final_output_offset: Some(0),
        };
        let adapter = ExecutionAdapter::new(Admit, 1024);
        let outcome = adapter.outcome(
            "unused",
            &status,
            Some(0),
            true,
            Instant::now() + Duration::from_secs(1),
        );
        assert!(matches!(
            outcome,
            Ok(ExecutionOutcome::CandidateSuccess { .. })
        ));
    }

    #[test]
    fn bounded_capture_overflow_is_unknown() {
        let status = SessionStatus {
            pid: 1,
            start_identity: None,
            exit: Some((Some(0), true)),
            input_epoch: 0,
            input_outcome_unknown: false,
            input_closed: true,
            output_offset: 2048,
            capture_failed: false,
            capture_state: CaptureState::Complete,
            final_output_offset: Some(2048),
        };
        let adapter = ExecutionAdapter::new(Admit, 1024);
        assert!(matches!(
            adapter.outcome(
                "unused",
                &status,
                Some(0),
                true,
                Instant::now() + Duration::from_secs(1)
            ),
            Ok(ExecutionOutcome::Unknown { .. })
        ));
    }

    #[test]
    fn owned_fake_child_is_recorded_once_and_reopen_is_ambiguous() {
        use crate::session::SessionHost;
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temporary directory");
        let marker = temp.path().join("spawn-marker");
        let spool = temp.path().join("spool");
        let _ = fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700));
        let binary = std::env::current_exe().expect("current test executable");
        let args = vec![
            "--exact".to_owned(),
            "execution::tests::deterministic_fake_child".to_owned(),
            "--ignored".to_owned(),
            "--nocapture".to_owned(),
            "--".to_owned(),
            format!("FAKE_MARKER={}", marker.display()),
        ];
        let spec = LaunchSpec {
            run_id: "stable-attempt-token".into(),
            program: binary,
            args,
            cwd: temp.path().to_path_buf(),
            spool_dir: spool,
        };
        let host = SessionHost::new();
        assert!(host.launch_with_environment(spec.clone(), &[]).is_ok());
        assert!(host.close_input(&spec.run_id).is_ok());
        assert!(host.wait(&spec.run_id).is_ok());
        assert!(spec.spool_dir.join("run.json").is_file());
        assert_eq!(
            fs::read(&marker).ok().as_deref(),
            Some(b"spawned\n".as_slice())
        );
        drop(host);

        let reopened = SessionHost::new();
        assert!(matches!(
            reopened.launch(spec.clone()),
            Err(SessionError::DuplicateRun(_))
        ));
        assert_eq!(
            fs::read(&marker).ok().as_deref(),
            Some(b"spawned\n".as_slice())
        );
    }

    #[test]
    fn prepared_command_rejects_mismatched_run_identity() {
        let identity = ExecutionIdentity {
            project_id: ProjectId::new("p").expect("project id"),
            root_id: RootId::new("r").expect("root id"),
            task_id: TaskId::new("t").expect("task id"),
            attempt_id: AttemptId::new("a").expect("attempt id"),
            contract_digest: "contract".into(),
            context_digest: "context".into(),
            candidate_digest: "candidate".into(),
            verification_policy_digest: "policy".into(),
            evidence_source: SourceIdentity {
                producer_id: "execution-adapter".into(),
                incarnation: "test-incarnation".into(),
                sequence: "1".into(),
            },
            workspace: PathBuf::from("/workspace"),
            provider: "codex".into(),
            model: "m".into(),
            run_id: "stable-token".into(),
            resource_permit_ref: "permit".into(),
        };
        let spec = LaunchSpec {
            run_id: "different-token".into(),
            program: Path::new("/bin/true").into(),
            args: vec![],
            cwd: PathBuf::from("/workspace"),
            spool_dir: PathBuf::from("/tmp/spool"),
        };
        assert!(matches!(
            PreparedCommand::new(identity, spec, vec![]),
            Err(ExecutionError::InvalidBinding)
        ));
    }
}
