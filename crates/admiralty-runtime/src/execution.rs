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
use std::{collections::BTreeMap, ffi::OsString, io, path::PathBuf, sync::Mutex};
use thiserror::Error;

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
    launched: Mutex<BTreeMap<String, ExecutionIdentity>>,
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
        let mut launched = self.launched.lock().map_err(|_| ExecutionError::Internal)?;
        self.host
            .launch_with_environment(command.spec.clone(), &command.environment)?;
        launched.insert(command.identity.run_id.clone(), command.identity.clone());
        Ok(())
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

    /// Closes prepared stdin, reaps the child, and returns an execution candidate;
    /// task acceptance remains a separate trusted-check decision.
    pub fn finish(&self, run_id: &str) -> Result<RuntimeEvidence, ExecutionError> {
        let identity = self
            .launched
            .lock()
            .map_err(|_| ExecutionError::Internal)?
            .get(run_id)
            .cloned()
            .ok_or(ExecutionError::UnknownIdentity)?;
        self.host.close_input(run_id)?;
        let exit = self.host.wait(run_id)?;
        let mut status = self.host.status(run_id)?;
        if status.capture_state == CaptureState::Capturing {
            status = self.host.status(run_id)?;
        }
        Ok(RuntimeEvidence {
            identity,
            outcome: self.outcome(run_id, &status, exit.code(), exit.success())?,
        })
    }

    fn outcome(
        &self,
        run_id: &str,
        status: &SessionStatus,
        code: Option<i32>,
        success: bool,
    ) -> Result<ExecutionOutcome, ExecutionError> {
        if status.capture_failed
            || status.capture_state != CaptureState::Complete
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
            let tail = self.host.tail(
                run_id,
                cursor,
                (self.max_output_bytes - cursor as usize).min(64 * 1024),
            )?;
            if tail.next_offset <= cursor {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path};

    // Child entrypoint invoked by the test harness as an exact, ignored test.
    #[test]
    #[ignore]
    fn deterministic_fake_child() {
        let path = std::env::args()
            .skip(1)
            .find(|s| s.starts_with("FAKE_MARKER="));
        if let Some(path) = path {
            let _ = fs::write(path.trim_start_matches("FAKE_MARKER="), b"spawned\n");
        }
        println!("deterministic fake child output");
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
        let outcome = adapter.outcome("unused", &status, Some(0), true);
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
            adapter.outcome("unused", &status, Some(0), true),
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
