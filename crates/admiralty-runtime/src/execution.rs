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
use std::{collections::BTreeMap, io, path::PathBuf, sync::Mutex};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ExecutionIdentity {
    pub project_id: String,
    pub root_id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub contract_digest: String,
    pub context_digest: String,
    pub candidate_digest: String,
    pub policy_digest: String,
    pub evidence_source: String,
    pub workspace: PathBuf,
    pub provider: String,
    pub model: String,
    pub run_id: String,
    pub resource_permit_ref: String,
}

/// Prepared argv plus its complete declared identity. This is not itself authority.
#[derive(Clone, Debug)]
pub struct PreparedCommand {
    identity: ExecutionIdentity,
    spec: LaunchSpec,
}
impl PreparedCommand {
    pub fn new(identity: ExecutionIdentity, spec: LaunchSpec) -> Result<Self, ExecutionError> {
        if identity.run_id != spec.run_id
            || identity.workspace != spec.cwd
            || identity.provider.trim().is_empty()
            || identity.model.trim().is_empty()
            || identity.resource_permit_ref.trim().is_empty()
            || [
                &identity.project_id,
                &identity.root_id,
                &identity.task_id,
                &identity.attempt_id,
                &identity.contract_digest,
                &identity.context_digest,
                &identity.candidate_digest,
                &identity.policy_digest,
                &identity.evidence_source,
            ]
            .iter()
            .any(|s| s.trim().is_empty())
        {
            return Err(ExecutionError::InvalidBinding);
        }
        crate::session::validate_launch_envelope(&spec)?;
        Ok(Self { identity, spec })
    }

    pub fn identity(&self) -> &ExecutionIdentity {
        &self.identity
    }

    pub fn run_id(&self) -> &str {
        &self.spec.run_id
    }
}

/// Validation is intentionally injected: this crate has no authoritative live
/// policy/lease or CPU permit service to consult. Implementations must reject stale
/// or mismatched workspace, reservation, provider/model, and resource permit state.
pub trait ExecutionAdmissionValidator {
    fn validate_current(
        &self,
        identity: &ExecutionIdentity,
        workspace: &ValidatedWorkspace,
        reservation: &Reservation,
    ) -> Result<(), String>;
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
    pub identity: ExecutionIdentity,
    pub outcome: ExecutionOutcome,
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
            || reservation.scope.project_id.to_string() != command.identity.project_id
            || reservation.scope.root_id.to_string() != command.identity.root_id
            || reservation.scope.task_id.to_string() != command.identity.task_id
            || reservation.scope.attempt_id.to_string() != command.identity.attempt_id
            || reservation.scope.contract_digest != command.identity.contract_digest
            || !reservation.outstanding
            || reservation.stop_requested
        {
            return Err(ExecutionError::InvalidBinding);
        }
        let workspace = workspace_spec
            .validate()
            .map_err(|e| ExecutionError::Workspace(e.to_string()))?;
        if workspace.project_id.to_string() != command.identity.project_id
            || workspace.root_id.to_string() != command.identity.root_id
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
        self.host.launch(command.spec.clone())?;
        launched.insert(command.identity.run_id.clone(), command.identity.clone());
        Ok(())
    }

    fn admit(&self, request: ExecutionRequest<'_>) -> Result<AdmissionProof, ExecutionError> {
        self.validator
            .validate_current(
                &request.command.identity,
                request.workspace,
                request.reservation,
            )
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
        fn validate_current(
            &self,
            _: &ExecutionIdentity,
            _: &ValidatedWorkspace,
            _: &Reservation,
        ) -> Result<(), String> {
            Ok(())
        }
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
        assert!(host.launch(spec.clone()).is_ok());
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
            project_id: "p".into(),
            root_id: "r".into(),
            task_id: "t".into(),
            attempt_id: "a".into(),
            contract_digest: "contract".into(),
            context_digest: "context".into(),
            candidate_digest: "candidate".into(),
            policy_digest: "policy".into(),
            evidence_source: "authority".into(),
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
            PreparedCommand::new(identity, spec),
            Err(ExecutionError::InvalidBinding)
        ));
    }
}
