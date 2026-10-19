//! Trusted startup adapter for native candidate acceptance. No HTTP route or
//! default allow policy is installed here. Actual check execution, actual owned
//! service closure and independent identity/policy/artifact custody are ALL
//! required. Grants are bounded, process-local, and bind one exact source and
//! command; they never authorize attached external effects or other transitions.
use admiralty_core::{
    CandidateDecisionEvidence, DomainEvent, LifecycleAction, LifecycleCommand, PrincipalId,
    ProjectId, Projection, RootId, SchedulingState, SourceIdentity, TaskId, TaskView,
    content_digest, reduce,
    verification::{VerificationPolicy, validate_receipt},
};
use admiralty_runtime::{resource_gate::ClosedAdmission, trusted_checks::ExecutedChecks};
use admiralty_store::{LifecycleAuthority, LifecycleEffect};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub const MAX_NATIVE_GRANTS: usize = 256;

/// Immutable scoped policy installed by the trusted service composition root.
/// No Deserialize implementation, network setter, or installation route exists.
#[derive(Clone, Debug)]
pub struct NativeLifecyclePolicy {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub authority_ref: String,
    pub source_producer: String,
    pub source_incarnation: String,
    pub verification: VerificationPolicy,
}

/// Exact facts presented to the trusted identity/custody authority. These facts
/// alone are not authority. In particular a named verifier is not authenticated.
#[derive(Clone, Debug)]
pub struct NativeDecisionBinding {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub contract_digest: String,
    pub attempt_id: admiralty_core::AttemptId,
    pub worker: PrincipalId,
    pub verifier: PrincipalId,
    pub authority_ref: String,
    pub policy_digest: String,
    pub checks_execution_digest: String,
    pub receipt_digest: String,
    pub closure_digest: String,
    pub source_artifact_digest: String,
    pub protected_oracle_digest: String,
    pub candidate_ref: String,
    pub source_commit_sha: String,
    pub source_tree_sha: String,
}

/// Implemented by the configured, independently authenticated verifier service.
/// The first method verifies actual verifier identity, pinned policy/oracle and
/// custody of source + output artifacts, and reconciled external effects, then
/// returns the digest of that authenticated custody record. Digest shape or a
/// worker-reported success is insufficient. Both callbacks must be bounded and
/// nonblocking; `remains_current` is called inside Store's serialized writer turn.
/// Never perform Store calls, network requests or provider turns from callbacks.
pub trait NativeEvidenceAuthority: Send + Sync {
    fn authenticate_evidence(
        &self,
        binding: &NativeDecisionBinding,
        checks: &ExecutedChecks,
        closed: &ClosedAdmission,
    ) -> Option<String>;
    fn remains_current(&self, binding: &NativeDecisionBinding, custody_digest: &str) -> bool;
}

#[derive(Debug)]
pub enum NativeLifecycleError {
    InvalidPolicy,
    CandidateMismatch,
    MissingClosure,
    EvidenceMismatch,
    Unauthorized,
    GrantCollision,
    Unavailable,
}
impl std::fmt::Display for NativeLifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native lifecycle rejected: {self:?}")
    }
}
impl std::error::Error for NativeLifecycleError {}
type SourceKey = (String, String, String);
#[derive(Clone)]
struct Grant {
    command: LifecycleCommand,
    before_task: String,
    after_task: String,
    binding: NativeDecisionBinding,
    custody_digest: String,
    proof_key: String,
}
#[derive(Default)]
struct Grants {
    by_source: BTreeMap<SourceKey, Grant>,
    proof_sources: BTreeMap<String, SourceKey>,
}

pub struct NativeLifecycleAuthority {
    policy: NativeLifecyclePolicy,
    evidence: Arc<dyn NativeEvidenceAuthority>,
    limit: usize,
    grants: Mutex<Grants>,
}
impl NativeLifecycleAuthority {
    pub fn new(
        policy: NativeLifecyclePolicy,
        evidence: Arc<dyn NativeEvidenceAuthority>,
        limit: usize,
    ) -> Result<Self, NativeLifecycleError> {
        if !(1..=MAX_NATIVE_GRANTS).contains(&limit)
            || !bounded(&policy.authority_ref)
            || !bounded(&policy.source_producer)
            || !bounded(&policy.source_incarnation)
            || policy.verification.schema_version != 1
            || policy.verification.required_negative_checks.is_empty()
        {
            return Err(NativeLifecycleError::InvalidPolicy);
        }
        Ok(Self {
            policy,
            evidence,
            limit,
            grants: Mutex::new(Grants::default()),
        })
    }
    /// Prepare ONE exact acceptance command from real opaque proofs. None means
    /// closure is still pending; it never produces a provisional acceptance.
    /// Failed checks have no ExecutedChecks proof and remain unaccepted. Rejection,
    /// retry/revision/cancel and effect grants require their own trusted protocol.
    pub fn issue_acceptance(
        &self,
        source: &SourceIdentity,
        task: &TaskView,
        checks: &ExecutedChecks,
        closed: Option<&ClosedAdmission>,
    ) -> Result<LifecycleCommand, NativeLifecycleError> {
        if !self.source_allowed(source) {
            return Err(NativeLifecycleError::InvalidPolicy);
        }
        if task.contract.simulated
            || task.state != SchedulingState::AwaitingVerification
            || task.contract.project_id != self.policy.project_id
            || task.contract.root_id != self.policy.root_id
            || task.contract.authority_ref != self.policy.authority_ref
            || task.owner.as_ref() == Some(&self.policy.verification.verifier)
        {
            return Err(NativeLifecycleError::CandidateMismatch);
        }
        let owner = task
            .owner
            .clone()
            .ok_or(NativeLifecycleError::CandidateMismatch)?;
        let attempt = task
            .attempt_id
            .clone()
            .ok_or(NativeLifecycleError::CandidateMismatch)?;
        let candidate = task
            .candidate_ref
            .clone()
            .filter(|v| !v.is_empty())
            .ok_or(NativeLifecycleError::CandidateMismatch)?;
        let inputs = &checks.receipt().inputs;
        let contract_digest = digest(&task.contract)?;
        let policy_digest = digest(&self.policy.verification)?;
        if inputs.project_id != task.contract.project_id
            || inputs.root_id != task.contract.root_id
            || inputs.task_id != task.contract.task_id
            || inputs.attempt_id != attempt
            || inputs.candidate_ref != candidate
            || inputs.contract_revision != task.contract.revision
            || inputs.contract_digest != contract_digest
            || inputs.verification_policy_digest != policy_digest
            || inputs.protected_oracle_digest != self.policy.verification.protected_oracle_digest
            || checks.receipt().verifier != self.policy.verification.verifier
        {
            return Err(NativeLifecycleError::EvidenceMismatch);
        }
        let validated = validate_receipt(
            &task.contract,
            inputs,
            &self.policy.verification,
            checks.receipt(),
        )
        .map_err(|_| NativeLifecycleError::EvidenceMismatch)?;
        if !validated.all_checks_passed() {
            return Err(NativeLifecycleError::EvidenceMismatch);
        }
        // This preparation method runs outside Store's writer callback and
        // before taking the grant lock. The writer callback remains I/O-free.
        checks
            .revalidate_environment(&task.contract, &self.policy.verification)
            .map_err(|_| NativeLifecycleError::EvidenceMismatch)?;
        let closed = closed.ok_or(NativeLifecycleError::MissingClosure)?;
        let identity = &closed.identity().identity;
        if identity.project_id != task.contract.project_id
            || identity.task_id != task.contract.task_id
            || identity.attempt_id != attempt
        {
            return Err(NativeLifecycleError::EvidenceMismatch);
        }
        let binding = NativeDecisionBinding {
            project_id: task.contract.project_id.clone(),
            root_id: task.contract.root_id.clone(),
            task_id: task.contract.task_id.clone(),
            contract_digest: contract_digest.clone(),
            attempt_id: attempt.clone(),
            worker: owner,
            verifier: self.policy.verification.verifier.clone(),
            authority_ref: task.contract.authority_ref.clone(),
            policy_digest: policy_digest.clone(),
            checks_execution_digest: checks.execution_digest().into(),
            receipt_digest: validated.receipt_digest().into(),
            closure_digest: closed.closure_digest().into(),
            source_artifact_digest: inputs.source_artifact_digest.clone(),
            protected_oracle_digest: inputs.protected_oracle_digest.clone(),
            candidate_ref: candidate.clone(),
            source_commit_sha: inputs.source_commit_sha.clone(),
            source_tree_sha: inputs.source_tree_sha.clone(),
        };
        let custody_digest = self
            .evidence
            .authenticate_evidence(&binding, checks, closed)
            .filter(|v| valid_digest(v))
            .ok_or(NativeLifecycleError::Unauthorized)?;
        if !self.evidence.remains_current(&binding, &custody_digest) {
            return Err(NativeLifecycleError::Unauthorized);
        }
        let before_task = digest(task)?;
        let command = LifecycleCommand {
            protocol_version: 2,
            task_id: task.contract.task_id.clone(),
            expected_task_digest: before_task.clone(),
            actor: self.policy.verification.verifier.clone(),
            action: LifecycleAction::DecideCandidate {
                attempt_id: attempt,
                candidate_ref: candidate,
                evidence: CandidateDecisionEvidence {
                    contract_digest,
                    verification_policy_digest: policy_digest,
                    receipt_digest: binding.receipt_digest.clone(),
                    custody_digest: custody_digest.clone(),
                    closure_digest: binding.closure_digest.clone(),
                    source_commit_sha: binding.source_commit_sha.clone(),
                    source_tree_sha: binding.source_tree_sha.clone(),
                    verifier: binding.verifier.clone(),
                },
                accepted: true,
            },
        };
        // Pure replay predicts the subject's own post-state. Dependency release
        // changes other tasks only, and Store independently applies the full graph.
        let mut projection = Projection::default();
        projection
            .tasks
            .insert(task.contract.task_id.clone(), task.clone());
        let post = reduce(
            &projection,
            &DomainEvent::TaskLifecycle {
                command: command.clone(),
            },
        )
        .map_err(|_| NativeLifecycleError::EvidenceMismatch)?;
        let after_task = digest(
            post.tasks
                .get(&task.contract.task_id)
                .ok_or(NativeLifecycleError::EvidenceMismatch)?,
        )?;
        let key = source_key(source);
        let proof_key = digest(&(
            &binding.checks_execution_digest,
            &binding.closure_digest,
            &binding.contract_digest,
        ))?;
        let grant = Grant {
            command: command.clone(),
            before_task,
            after_task,
            binding,
            custody_digest,
            proof_key: proof_key.clone(),
        };
        let mut grants = self
            .grants
            .try_lock()
            .map_err(|_| NativeLifecycleError::Unavailable)?;
        if let Some(existing) = grants.by_source.get(&key) {
            return if existing.command == command && existing.proof_key == proof_key {
                Ok(existing.command.clone())
            } else {
                Err(NativeLifecycleError::GrantCollision)
            };
        }
        if grants.proof_sources.contains_key(&proof_key) {
            return Err(NativeLifecycleError::GrantCollision);
        }
        if grants.by_source.len() >= self.limit {
            return Err(NativeLifecycleError::Unavailable);
        }
        grants.proof_sources.insert(proof_key, key.clone());
        grants.by_source.insert(key, grant);
        Ok(command)
    }
    fn source_allowed(&self, source: &SourceIdentity) -> bool {
        source.producer_id == self.policy.source_producer
            && source.incarnation == self.policy.source_incarnation
            && source
                .validate()
                .ok()
                .is_some_and(|seq| seq <= i64::MAX as u64)
    }
}
impl LifecycleAuthority for NativeLifecycleAuthority {
    fn authorize(
        &self,
        source: &SourceIdentity,
        command: &LifecycleCommand,
        task: &TaskView,
        effect: Option<&LifecycleEffect<'_>>,
    ) -> bool {
        if effect.is_some() || !self.source_allowed(source) {
            return false;
        }
        let grant = {
            let Ok(grants) = self.grants.try_lock() else {
                return false;
            };
            let Some(grant) = grants.by_source.get(&source_key(source)) else {
                return false;
            };
            grant.clone()
        };
        if command != &grant.command
            || task.contract.project_id != grant.binding.project_id
            || task.contract.root_id != grant.binding.root_id
        {
            return false;
        }
        let Ok(current) = digest(task) else {
            return false;
        };
        // Do not consume here: Store calls authorization BEFORE duplicate lookup.
        // Exact replay remains authorized after commit, with current revocation
        // checked again. Any different command/source/proof fails closed.
        (current == grant.before_task || current == grant.after_task)
            && self
                .evidence
                .remains_current(&grant.binding, &grant.custody_digest)
    }
}
fn source_key(source: &SourceIdentity) -> SourceKey {
    (
        source.producer_id.clone(),
        source.incarnation.clone(),
        source.sequence.clone(),
    )
}
fn digest<T: serde::Serialize>(value: &T) -> Result<String, NativeLifecycleError> {
    content_digest(value).map_err(|_| NativeLifecycleError::EvidenceMismatch)
}
fn bounded(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use admiralty_core::{
        AttemptId, CheckSpec, TaskContract,
        verification::{RequiredNegativeCheck, VerificationInputs},
    };
    use admiralty_runtime::{session::SessionHost, trusted_checks::execute_checks};
    use admiralty_store::Store;
    use std::{fs, path::Path, process::Command};
    struct PrivateTemp(std::path::PathBuf);
    impl PrivateTemp {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for PrivateTemp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn private_tempdir() -> PrivateTemp {
        use std::os::unix::fs::DirBuilderExt;
        let path = std::env::temp_dir()
            .canonicalize()
            .expect("temp root")
            .join(format!("native-lifecycle-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("private fixture");
        PrivateTemp(path)
    }
    struct Deny;
    impl NativeEvidenceAuthority for Deny {
        fn authenticate_evidence(
            &self,
            _: &NativeDecisionBinding,
            _: &ExecutedChecks,
            _: &ClosedAdmission,
        ) -> Option<String> {
            None
        }
        fn remains_current(&self, _: &NativeDecisionBinding, _: &str) -> bool {
            false
        }
    }
    fn source() -> SourceIdentity {
        SourceIdentity {
            producer_id: "trusted-verifier-service".into(),
            incarnation: "service-boot".into(),
            sequence: "1".into(),
        }
    }
    fn contract() -> TaskContract {
        serde_json::from_value(serde_json::json!({"schema_version":1,"project_id":"project","root_id":"root","task_id":"task","revision":1,
            "title":"Check actual candidate","objective":"Only authentic execution and closure can accept","non_goals":[],"owned_paths":["file.txt"],"forbidden_paths":[],"read_first":[],"dependencies":[],
            "criteria":[{"id":"clean","text":"candidate is clean","check_ids":["clean"]}],
            "checks":[{"id":"clean","program":"git","args":["diff","--exit-code","HEAD","--","file.txt"],"cwd":".","timeout_secs":10}],"authority_ref":"grant:project","max_attempts":2,"simulated":false})).expect("contract")
    }
    fn policy() -> NativeLifecyclePolicy {
        NativeLifecyclePolicy {
            project_id: ProjectId::new("project").expect("project"),
            root_id: RootId::new("root").expect("root"),
            authority_ref: "grant:project".into(),
            source_producer: source().producer_id,
            source_incarnation: source().incarnation,
            verification: VerificationPolicy {
                schema_version: 1,
                verifier: PrincipalId::new("independent-verifier").expect("verifier"),
                protected_oracle_digest: content_digest(&"oracle").expect("digest"),
                required_negative_checks: vec![RequiredNegativeCheck {
                    check: CheckSpec {
                        id: "reject-missing".into(),
                        program: "git".into(),
                        args: vec![
                            "rev-parse".into(),
                            "--verify".into(),
                            "refs/heads/definitely-missing".into(),
                        ],
                        cwd: ".".into(),
                        timeout_secs: 10,
                    },
                    expected_exit_code: 128,
                    fixture_digest: content_digest(&"missing-ref").expect("digest"),
                }],
            },
        }
    }
    fn candidate(store: &Store) -> TaskView {
        let events = [
            DomainEvent::TaskSubmitted {
                contract: contract(),
            },
            DomainEvent::TaskClaimed {
                task_id: TaskId::new("task").expect("task"),
                revision: 1,
                attempt_id: AttemptId::new("attempt").expect("attempt"),
                owner: PrincipalId::new("worker").expect("owner"),
            },
            DomainEvent::TaskStarted {
                task_id: TaskId::new("task").expect("task"),
                attempt_id: AttemptId::new("attempt").expect("attempt"),
            },
            DomainEvent::CandidateSubmitted {
                task_id: TaskId::new("task").expect("task"),
                attempt_id: AttemptId::new("attempt").expect("attempt"),
                candidate_ref: "candidate-one".into(),
            },
        ];
        for (i, event) in events.into_iter().enumerate() {
            store
                .append(
                    SourceIdentity {
                        producer_id: "task-service".into(),
                        incarnation: "boot".into(),
                        sequence: (i + 1).to_string(),
                    },
                    event,
                    "2026-10-18T00:00:00Z".into(),
                )
                .expect("event");
        }
        store.snapshot().expect("snapshot").projection.tasks[&TaskId::new("task").expect("id")]
            .clone()
    }
    fn git(path: &Path, args: &[&str]) -> String {
        let output = Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", "/nonexistent")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .arg("-c")
            .arg("core.hooksPath=/dev/null")
            .args(args)
            .current_dir(path)
            .output()
            .expect("owned Git fixture");
        assert!(output.status.success(), "fixture Git failed");
        String::from_utf8(output.stdout)
            .expect("UTF8")
            .trim()
            .into()
    }
    #[test]
    fn unissued_forged_and_effect_bearing_commands_cannot_mutate_actual_store() {
        let dir = private_tempdir();
        let authority = Arc::new(
            NativeLifecycleAuthority::new(policy(), Arc::new(Deny), 2).expect("authority"),
        );
        let store =
            Store::open_with_lifecycle_authority(dir.path().join("store.sqlite"), authority)
                .expect("Store");
        let task = candidate(&store);
        let before = store.snapshot().expect("snapshot");
        let command = LifecycleCommand {
            protocol_version: 2,
            task_id: task.contract.task_id.clone(),
            expected_task_digest: content_digest(&task).expect("CAS"),
            actor: policy().verification.verifier,
            action: LifecycleAction::CancelPending {
                reason: "forged request".into(),
            },
        };
        assert!(
            store
                .execute_lifecycle(
                    source(),
                    command.clone(),
                    "2026-10-18T00:00:00Z".into(),
                    None
                )
                .is_err()
        );
        assert!(
            store
                .execute_lifecycle(
                    source(),
                    command,
                    "2026-10-18T00:00:00Z".into(),
                    Some(admiralty_store::outbox::OutboxIntent {
                        idempotency_key: "forged-effect".into(),
                        payload: serde_json::json!({"action":"publish"}),
                        not_before: 0
                    })
                )
                .is_err()
        );
        assert_eq!(store.snapshot().expect("snapshot"), before);
    }
    #[test]
    fn real_executed_checks_without_runtime_closure_do_not_issue_acceptance() {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let dir = private_tempdir();
        let base = fs::canonicalize(dir.path()).expect("canonical");
        let checkout = base.join("candidate");
        let spool = base.join("checks");
        for p in [&checkout, &spool] {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(p)
                .expect("private dir");
        }
        git(&checkout, &["init", "-q"]);
        fs::write(checkout.join("file.txt"), "candidate\n").expect("file");
        git(&checkout, &["add", "file.txt"]);
        git(
            &checkout,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                "candidate",
            ],
        );
        fs::set_permissions(checkout.join(".git"), fs::Permissions::from_mode(0o700))
            .expect("metadata");
        let store = Store::open(base.join("store.sqlite")).expect("Store");
        let task = candidate(&store);
        let policy = policy();
        let inputs = VerificationInputs {
            project_id: task.contract.project_id.clone(),
            root_id: task.contract.root_id.clone(),
            task_id: task.contract.task_id.clone(),
            attempt_id: task.attempt_id.clone().expect("attempt"),
            candidate_ref: task.candidate_ref.clone().expect("candidate"),
            contract_revision: 1,
            contract_digest: content_digest(&task.contract).expect("digest"),
            verification_policy_digest: content_digest(&policy.verification).expect("digest"),
            source_commit_sha: git(&checkout, &["rev-parse", "HEAD"]),
            source_tree_sha: git(&checkout, &["rev-parse", "HEAD^{tree}"]),
            source_artifact_digest: content_digest(&"source-artifact").expect("digest"),
            protected_oracle_digest: policy.verification.protected_oracle_digest.clone(),
        };
        let checks = execute_checks(
            &SessionHost::new(),
            &inputs,
            &task.contract,
            &policy.verification,
            &checkout,
            &spool,
            "native-adapter-checks",
        )
        .expect("real checks");
        let authority =
            NativeLifecycleAuthority::new(policy, Arc::new(Deny), 2).expect("authority");
        assert!(matches!(
            authority.issue_acceptance(&source(), &task, &checks, None),
            Err(NativeLifecycleError::MissingClosure)
        ));
        let mut stale = task;
        stale.candidate_ref = Some("another-candidate".into());
        assert!(matches!(
            authority.issue_acceptance(&source(), &stale, &checks, None),
            Err(NativeLifecycleError::EvidenceMismatch)
        ));
    }
}
