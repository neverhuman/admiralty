//! Pure proposals for auditing exact staged-index evidence. This module does not
//! authenticate the auditor, invoke the quality tool, or execute reproduction checks.
use admiralty_core::{
    CheckSpec, MessageId, PrincipalId, ProjectId, RootId, TaskId, content_digest,
    safe_relative_path,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_AUDIT_JSON_BYTES: usize = 1_048_576;
pub const MAX_AUDIT_ITEMS: usize = 1_024;
pub const MAX_AUDIT_STRING_BYTES: usize = 4_096;

/// Precision-safe canonical decimal encoding for score counters.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecimalU64(u64);
impl DecimalU64 {
    pub fn new(value: u64) -> Self {
        Self(value)
    }
    pub fn get(&self) -> u64 {
        self.0
    }
}
impl Serialize for DecimalU64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let parsed = value.parse::<u64>().map_err(de::Error::custom)?;
        if parsed.to_string() != value {
            return Err(de::Error::custom("expected canonical decimal u64"));
        }
        Ok(Self(parsed))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub contract_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StagedAuditIdentity {
    /// Full commit identity (40 or 64 lowercase hexadecimal characters).
    pub base_sha: String,
    /// Full staged tree identity (40 or 64 lowercase hexadecimal characters).
    pub staged_tree_sha: String,
    pub profile_digest: String,
    pub policy_digest: String,
    pub oracle_digest: String,
    pub auditor: PrincipalId,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TreeSource {
    StagedIndex,
    WorkingTree,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Info,
    Warning,
    Hard,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditFinding {
    pub id: String,
    pub severity: FindingSeverity,
    pub detail: String,
    pub evidence_ref: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Passed,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckResult {
    pub check_id: String,
    pub outcome: CheckOutcome,
    /// `None` means no exit was observed; it is never treated as zero.
    pub exit_code: Option<i32>,
    pub evidence_ref: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StagedAuditEvidence {
    pub audit_id: MessageId,
    pub scope: AuditScope,
    pub identity: StagedAuditIdentity,
    /// Digest of the immutable staged-content capsule supplied to the auditor.
    pub staged_content_digest: String,
    pub scope_coverage: Vec<String>,
    pub coverage_complete: bool,
    pub full_context_available: bool,
    pub profile_supported: bool,
    pub tree_source: TreeSource,
    /// The auditor confirms the materialized index matched the supplied tree.
    pub index_matches_staged_tree: bool,
    pub score: Option<DecimalU64>,
    pub findings: Vec<AuditFinding>,
    pub check_results: Vec<CheckResult>,
    /// Bounded immutable references only; never commands or executable actions.
    pub evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditReplayReceipt {
    pub audit_id: MessageId,
    pub evidence_digest: String,
    pub outcome: AuditOutcome,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditPolicy {
    pub required_identity: StagedAuditIdentity,
    pub required_paths: Vec<String>,
    pub hard_findings: Vec<String>,
    pub required_checks: Vec<CheckSpec>,
    pub minimum_score: Option<DecimalU64>,
    /// Caller supplies the persisted prior receipt projection for replay checks.
    pub prior_audit: Option<AuditReplayReceipt>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuditOutcome {
    PassProposal {
        score: Option<DecimalU64>,
    },
    NeedsFullContext {
        missing_paths: Vec<String>,
        full_context_missing: bool,
    },
    Unsupported {
        reason: String,
    },
    Reproduce {
        checks: Vec<CheckSpec>,
        evidence_refs: Vec<String>,
    },
    HardFindings {
        finding_ids: Vec<String>,
    },
    BelowThreshold {
        score: DecimalU64,
        minimum: DecimalU64,
    },
    ScoreUnknown {
        minimum: DecimalU64,
    },
    Invalidated {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuditProposal {
    pub audit_id: MessageId,
    pub scope: AuditScope,
    pub identity: StagedAuditIdentity,
    pub evidence_digest: String,
    pub outcome: AuditOutcome,
    pub replayed: bool,
    /// An audit proposal is never task or candidate acceptance.
    pub acceptance_granted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct AuditError(pub &'static str);

fn audit_digest(
    scope: &AuditScope,
    policy: &AuditPolicy,
    evidence: &StagedAuditEvidence,
) -> Result<String, AuditError> {
    content_digest(&(
        scope,
        &policy.required_identity,
        &policy.required_paths,
        &policy.hard_findings,
        &policy.required_checks,
        &policy.minimum_score,
        evidence,
    ))
    .map_err(|_| AuditError("cannot fingerprint bounded audit request"))
}

fn policy_content_digest(policy: &AuditPolicy) -> Result<String, AuditError> {
    content_digest(&(
        &policy.required_paths,
        &policy.hard_findings,
        &policy.required_checks,
        &policy.minimum_score,
    ))
    .map_err(|_| AuditError("cannot fingerprint bounded audit policy"))
}

pub fn evaluate_staged_audit(
    expected_scope: &AuditScope,
    policy: &AuditPolicy,
    evidence: &StagedAuditEvidence,
) -> Result<AuditProposal, AuditError> {
    validate(expected_scope, policy, evidence)?;
    let evidence_digest = audit_digest(expected_scope, policy, evidence)?;
    let invalidation = if evidence.identity != policy.required_identity {
        Some("base, staged tree, profile, policy, oracle or auditor identity changed")
    } else if evidence.tree_source != TreeSource::StagedIndex || !evidence.index_matches_staged_tree
    {
        Some("evidence does not bind the exact materialized staged index")
    } else {
        None
    };
    let (outcome, replayed) = if let Some(prior) = &policy.prior_audit {
        if prior.audit_id == evidence.audit_id {
            if prior.evidence_digest == evidence_digest {
                (prior.outcome.clone(), true)
            } else {
                return Err(AuditError("audit ID reused with different evidence"));
            }
        } else {
            (
                evaluate_outcome(expected_scope, policy, evidence, invalidation),
                false,
            )
        }
    } else {
        (
            evaluate_outcome(expected_scope, policy, evidence, invalidation),
            false,
        )
    };
    Ok(AuditProposal {
        audit_id: evidence.audit_id.clone(),
        scope: expected_scope.clone(),
        identity: evidence.identity.clone(),
        evidence_digest,
        outcome,
        replayed,
        acceptance_granted: false,
    })
}

fn evaluate_outcome(
    expected_scope: &AuditScope,
    policy: &AuditPolicy,
    evidence: &StagedAuditEvidence,
    invalidation: Option<&'static str>,
) -> AuditOutcome {
    if &evidence.scope != expected_scope {
        return AuditOutcome::Invalidated {
            reason: "scope mismatch".into(),
        };
    }
    if let Some(reason) = invalidation {
        return AuditOutcome::Invalidated {
            reason: reason.into(),
        };
    }
    if !evidence.profile_supported {
        return AuditOutcome::Unsupported {
            reason: "profile does not support this audit scope".into(),
        };
    }
    let covered: BTreeSet<_> = evidence.scope_coverage.iter().map(String::as_str).collect();
    let missing_paths: Vec<_> = policy
        .required_paths
        .iter()
        .filter(|path| !covered.contains(path.as_str()))
        .cloned()
        .collect();
    if !evidence.coverage_complete || !evidence.full_context_available || !missing_paths.is_empty()
    {
        return AuditOutcome::NeedsFullContext {
            missing_paths,
            full_context_missing: !evidence.full_context_available,
        };
    }
    let hard_findings: Vec<_> = evidence
        .findings
        .iter()
        .filter(|finding| {
            policy.hard_findings.contains(&finding.id) || finding.severity == FindingSeverity::Hard
        })
        .map(|finding| finding.id.clone())
        .collect();
    if !hard_findings.is_empty() {
        return AuditOutcome::HardFindings {
            finding_ids: hard_findings,
        };
    }
    let results: BTreeMap<_, _> = evidence
        .check_results
        .iter()
        .map(|result| (result.check_id.as_str(), result))
        .collect();
    let unresolved: Vec<_> = policy.required_checks.iter().filter(|check| !matches!(results.get(check.id.as_str()), Some(result) if result.outcome == CheckOutcome::Passed && result.exit_code == Some(0))).cloned().collect();
    if !unresolved.is_empty() {
        return AuditOutcome::Reproduce {
            checks: unresolved,
            evidence_refs: evidence.evidence_refs.clone(),
        };
    }
    if let Some(minimum) = &policy.minimum_score {
        match &evidence.score {
            Some(score) if score < minimum => {
                return AuditOutcome::BelowThreshold {
                    score: score.clone(),
                    minimum: minimum.clone(),
                };
            }
            Some(_) => {}
            None => {
                return AuditOutcome::ScoreUnknown {
                    minimum: minimum.clone(),
                };
            }
        }
    }
    AuditOutcome::PassProposal {
        score: evidence.score.clone(),
    }
}

fn validate(
    scope: &AuditScope,
    policy: &AuditPolicy,
    evidence: &StagedAuditEvidence,
) -> Result<(), AuditError> {
    let mut size = SizeLimit(MAX_AUDIT_JSON_BYTES);
    serde_json::to_writer(&mut size, &(scope, policy, evidence))
        .map_err(|_| AuditError("audit input exceeds 1 MiB or cannot be serialized"))?;
    if scope.contract_digest.trim().is_empty() || !digest(&scope.contract_digest) {
        return Err(AuditError("invalid contract digest"));
    }
    if evidence.scope.contract_digest.trim().is_empty() || !digest(&evidence.scope.contract_digest)
    {
        return Err(AuditError("invalid evidence contract digest"));
    }
    if !digest(&evidence.staged_content_digest) {
        return Err(AuditError("invalid staged content digest"));
    }
    for value in [
        &policy.required_identity.profile_digest,
        &policy.required_identity.policy_digest,
        &policy.required_identity.oracle_digest,
        &evidence.identity.profile_digest,
        &evidence.identity.policy_digest,
        &evidence.identity.oracle_digest,
    ] {
        if !digest(value) {
            return Err(AuditError("invalid profile, policy or oracle digest"));
        }
    }
    if policy_content_digest(policy)? != policy.required_identity.policy_digest {
        return Err(AuditError(
            "policy content does not match its protected digest",
        ));
    }
    if !full_git_sha(&policy.required_identity.base_sha)
        || !full_git_sha(&policy.required_identity.staged_tree_sha)
        || !full_git_sha(&evidence.identity.base_sha)
        || !full_git_sha(&evidence.identity.staged_tree_sha)
    {
        return Err(AuditError("invalid full base or staged tree identity"));
    }
    for list_len in [
        policy.required_paths.len(),
        policy.hard_findings.len(),
        policy.required_checks.len(),
        evidence.scope_coverage.len(),
        evidence.findings.len(),
        evidence.check_results.len(),
        evidence.evidence_refs.len(),
    ] {
        if list_len > MAX_AUDIT_ITEMS {
            return Err(AuditError("audit collection bound exceeded"));
        }
    }
    let unique = |values: Vec<&str>| values.iter().collect::<BTreeSet<_>>().len() == values.len();
    if !unique(policy.required_paths.iter().map(String::as_str).collect())
        || !unique(policy.hard_findings.iter().map(String::as_str).collect())
        || !unique(
            policy
                .required_checks
                .iter()
                .map(|check| check.id.as_str())
                .collect(),
        )
        || !unique(evidence.scope_coverage.iter().map(String::as_str).collect())
        || !unique(
            evidence
                .findings
                .iter()
                .map(|finding| finding.id.as_str())
                .collect(),
        )
        || !unique(
            evidence
                .check_results
                .iter()
                .map(|result| result.check_id.as_str())
                .collect(),
        )
    {
        return Err(AuditError("duplicate path, finding or check ID"));
    }
    let required_check_ids: BTreeSet<_> = policy
        .required_checks
        .iter()
        .map(|check| check.id.as_str())
        .collect();
    if evidence
        .check_results
        .iter()
        .any(|result| !required_check_ids.contains(result.check_id.as_str()))
    {
        return Err(AuditError("result references an undeclared check"));
    }
    if policy
        .required_paths
        .iter()
        .any(|path| !safe_relative_path(path) || !bounded(path))
        || policy
            .hard_findings
            .iter()
            .any(|value| value.trim().is_empty() || !bounded(value))
        || evidence
            .scope_coverage
            .iter()
            .any(|path| !safe_relative_path(path) || !bounded(path))
    {
        return Err(AuditError("invalid or oversized scope path or finding ID"));
    }
    for check in &policy.required_checks {
        validate_check(check)?;
    }
    for finding in &evidence.findings {
        if finding.id.trim().is_empty()
            || !bounded(&finding.id)
            || !bounded(&finding.detail)
            || finding
                .evidence_ref
                .as_ref()
                .is_some_and(|value| !valid_ref(value))
        {
            return Err(AuditError("invalid or oversized finding"));
        }
    }
    for result in &evidence.check_results {
        let coherent = match result.outcome {
            CheckOutcome::Passed => result.exit_code == Some(0),
            CheckOutcome::Failed => result.exit_code.is_some_and(|code| code != 0),
            CheckOutcome::Unknown => result.exit_code.is_none(),
        };
        if result.check_id.trim().is_empty()
            || !bounded(&result.check_id)
            || !coherent
            || result
                .evidence_ref
                .as_ref()
                .is_some_and(|value| !valid_ref(value))
        {
            return Err(AuditError(
                "invalid check result or unknown exit representation",
            ));
        }
    }
    if evidence.evidence_refs.iter().any(|value| !valid_ref(value)) {
        return Err(AuditError("invalid evidence reference"));
    }
    if let Some(prior) = &policy.prior_audit
        && !digest(&prior.evidence_digest)
    {
        return Err(AuditError("invalid replay evidence digest"));
    }
    Ok(())
}
fn validate_check(check: &CheckSpec) -> Result<(), AuditError> {
    if check.id.trim().is_empty()
        || !bounded(&check.id)
        || check.program.trim().is_empty()
        || !bounded(&check.program)
        || check.program.contains('\0')
        || check.args.len() > MAX_AUDIT_ITEMS
        || check
            .args
            .iter()
            .any(|arg| !bounded(arg) || arg.contains('\0'))
        || !safe_relative_path(&check.cwd)
        || !bounded(&check.cwd)
        || check.timeout_secs == 0
    {
        return Err(AuditError("invalid bounded reproduction check"));
    }
    Ok(())
}
fn valid_ref(value: &str) -> bool {
    bounded(value) && value.starts_with("sha256:") && digest(value)
}
fn bounded(value: &str) -> bool {
    value.len() <= MAX_AUDIT_STRING_BYTES
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn full_git_sha(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
struct SizeLimit(usize);
impl std::io::Write for SizeLimit {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("audit input size limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pid(value: &str) -> PrincipalId {
        PrincipalId::new(value).expect("fixture principal")
    }
    fn mid(value: &str) -> MessageId {
        MessageId::new(value).expect("fixture audit ID")
    }
    fn d(ch: char) -> String {
        format!("sha256:{}", ch.to_string().repeat(64))
    }
    fn scope() -> AuditScope {
        AuditScope {
            project_id: ProjectId::new("project").expect("fixture project"),
            root_id: RootId::new("root").expect("fixture root"),
            task_id: TaskId::new("task").expect("fixture task"),
            contract_digest: d('a'),
        }
    }
    fn identity() -> StagedAuditIdentity {
        StagedAuditIdentity {
            base_sha: "1".repeat(40),
            staged_tree_sha: "2".repeat(40),
            profile_digest: d('b'),
            policy_digest: d('c'),
            oracle_digest: d('d'),
            auditor: pid("auditor"),
        }
    }
    fn check() -> CheckSpec {
        CheckSpec {
            id: "unit".into(),
            program: "cargo".into(),
            args: vec!["test".into()],
            cwd: ".".into(),
            timeout_secs: 60,
        }
    }
    fn policy() -> AuditPolicy {
        let mut policy = AuditPolicy {
            required_identity: identity(),
            required_paths: vec!["src/lib.rs".into()],
            hard_findings: vec!["secret".into()],
            required_checks: vec![check()],
            minimum_score: Some(DecimalU64::new(80)),
            prior_audit: None,
        };
        policy.required_identity.policy_digest =
            policy_content_digest(&policy).expect("fixture policy digest");
        policy
    }
    fn evidence() -> StagedAuditEvidence {
        StagedAuditEvidence {
            audit_id: mid("audit-1"),
            scope: scope(),
            identity: policy().required_identity,
            staged_content_digest: d('e'),
            scope_coverage: vec!["src/lib.rs".into()],
            coverage_complete: true,
            full_context_available: true,
            profile_supported: true,
            tree_source: TreeSource::StagedIndex,
            index_matches_staged_tree: true,
            score: Some(DecimalU64::new(90)),
            findings: vec![],
            check_results: vec![CheckResult {
                check_id: "unit".into(),
                outcome: CheckOutcome::Passed,
                exit_code: Some(0),
                evidence_ref: Some(d('e')),
            }],
            evidence_refs: vec![d('e')],
        }
    }
    fn evaluate(p: &AuditPolicy, e: &StagedAuditEvidence) -> Result<AuditProposal, AuditError> {
        evaluate_staged_audit(&scope(), p, e)
    }
    #[test]
    fn exact_staged_evidence_produces_proposal_only() {
        let result = evaluate(&policy(), &evidence()).expect("exact staged audit");
        assert!(matches!(result.outcome, AuditOutcome::PassProposal { .. }));
        assert!(!result.acceptance_granted);
        let mut wrong_scope = evidence();
        wrong_scope.scope.project_id = ProjectId::new("foreign").expect("fixture project");
        assert!(matches!(
            evaluate(&policy(), &wrong_scope)
                .expect("scope mismatch")
                .outcome,
            AuditOutcome::Invalidated { .. }
        ));
    }
    #[test]
    fn identity_changes_and_worktree_substitution_invalidate() {
        for change in 0..6 {
            let mut e = evidence();
            match change {
                0 => e.identity.base_sha = "3".repeat(40),
                1 => e.identity.staged_tree_sha = "3".repeat(40),
                2 => e.identity.profile_digest = d('f'),
                3 => e.identity.policy_digest = d('f'),
                4 => e.identity.oracle_digest = d('f'),
                _ => e.identity.auditor = pid("other"),
            }
            assert!(matches!(
                evaluate(&policy(), &e).expect("identity result").outcome,
                AuditOutcome::Invalidated { .. }
            ));
        }
        let mut e = evidence();
        e.tree_source = TreeSource::WorkingTree;
        assert!(matches!(
            evaluate(&policy(), &e)
                .expect("working tree result")
                .outcome,
            AuditOutcome::Invalidated { .. }
        ));
        e.tree_source = TreeSource::StagedIndex;
        e.index_matches_staged_tree = false;
        assert!(matches!(
            evaluate(&policy(), &e)
                .expect("partial index result")
                .outcome,
            AuditOutcome::Invalidated { .. }
        ));
    }
    #[test]
    fn coverage_context_and_unsupported_profile_never_pass() {
        let mut e = evidence();
        e.coverage_complete = false;
        assert!(matches!(
            evaluate(&policy(), &e).expect("coverage gap").outcome,
            AuditOutcome::NeedsFullContext { .. }
        ));
        e = evidence();
        e.full_context_available = false;
        assert!(matches!(
            evaluate(&policy(), &e).expect("context gap").outcome,
            AuditOutcome::NeedsFullContext {
                full_context_missing: true,
                ..
            }
        ));
        e = evidence();
        e.profile_supported = false;
        assert!(matches!(
            evaluate(&policy(), &e)
                .expect("unsupported profile")
                .outcome,
            AuditOutcome::Unsupported { .. }
        ));
    }
    #[test]
    fn hard_finding_cannot_be_relaxed_by_score_and_unknown_score_stays_unknown() {
        let mut e = evidence();
        e.findings.push(AuditFinding {
            id: "secret".into(),
            severity: FindingSeverity::Warning,
            detail: "protected finding".into(),
            evidence_ref: Some(d('e')),
        });
        e.score = Some(DecimalU64::new(u64::MAX));
        let mut lowered = policy();
        lowered.minimum_score = Some(DecimalU64::new(0));
        lowered.required_identity.policy_digest =
            policy_content_digest(&lowered).expect("lowered policy digest");
        e.identity.policy_digest = lowered.required_identity.policy_digest.clone();
        assert!(matches!(
            evaluate(&lowered, &e).expect("hard finding").outcome,
            AuditOutcome::HardFindings { .. }
        ));
        e = evidence();
        e.score = None;
        assert!(matches!(
            evaluate(&policy(), &e).expect("unknown score").outcome,
            AuditOutcome::ScoreUnknown { .. }
        ));
    }
    #[test]
    fn missing_or_unknown_checks_propose_reproduction_and_bad_exit_is_rejected() {
        let mut e = evidence();
        e.check_results[0].outcome = CheckOutcome::Unknown;
        e.check_results[0].exit_code = None;
        assert!(
            matches!(evaluate(&policy(), &e).expect("unknown check").outcome, AuditOutcome::Reproduce { checks, .. } if checks.len() == 1)
        );
        e.check_results[0].outcome = CheckOutcome::Passed;
        e.check_results[0].exit_code = Some(1);
        assert!(evaluate(&policy(), &e).is_err());
    }
    #[test]
    fn replay_is_idempotent_and_same_audit_id_changed_bytes_collides() {
        let p = policy();
        let e = evidence();
        let digest = audit_digest(&scope(), &p, &e).expect("fixture evidence digest");
        let original = evaluate(&p, &e).expect("original evaluation");
        let mut replay = p.clone();
        replay.prior_audit = Some(AuditReplayReceipt {
            audit_id: e.audit_id.clone(),
            evidence_digest: digest.clone(),
            outcome: original.outcome,
        });
        let replayed = evaluate(&replay, &e).expect("exact replay");
        assert!(replayed.replayed);
        assert!(matches!(
            replayed.outcome,
            AuditOutcome::PassProposal { .. }
        ));
        let mut changed = e.clone();
        changed.score = Some(DecimalU64::new(91));
        assert!(evaluate(&replay, &changed).is_err());
    }
    #[test]
    fn malformed_bounds_duplicates_unknown_fields_and_decimal_precision_fail() {
        let mut e = evidence();
        e.scope_coverage.push("src/lib.rs".into());
        assert!(evaluate(&policy(), &e).is_err());
        let mut p = policy();
        p.minimum_score = Some(DecimalU64::new(0));
        assert!(evaluate(&p, &evidence()).is_err());
        p = policy();
        p.required_paths = (0..=MAX_AUDIT_ITEMS)
            .map(|index| format!("src/file-{index}.rs"))
            .collect();
        p.required_identity.policy_digest =
            policy_content_digest(&p).expect("oversized fixture policy digest");
        assert!(evaluate(&p, &evidence()).is_err());
        e = evidence();
        e.evidence_refs = vec!["x".repeat(MAX_AUDIT_STRING_BYTES + 1)];
        assert!(evaluate(&policy(), &e).is_err());
        assert!(serde_json::from_str::<DecimalU64>("\"01\"").is_err());
        assert!(
            serde_json::from_str::<AuditScope>(
                r#"{"project_id":"p","root_id":"r","task_id":"t","contract_digest":"x","extra":1}"#
            )
            .is_err()
        );
        let mut p = policy();
        p.required_paths = vec!["../escape".into()];
        assert!(evaluate(&p, &evidence()).is_err());
    }
}
