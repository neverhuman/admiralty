//! Pure, bounded operator explanations for recorded task state.
use admiralty_core::{
    AttemptId, PrincipalId, ProjectId, RootId, TaskId, content_digest, limits::bounded_json_size,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::{error::Error as StdError, fmt};

pub const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_TEXT_BYTES: usize = 4096;

/// Canonical decimal u64. Strings preserve precision across JSON/JavaScript.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Tick(String);

impl Tick {
    pub fn new(value: impl Into<String>) -> Result<Self, DiagnosticError> {
        let value = value.into();
        parse_tick(&value)?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    fn value(&self) -> Result<u64, DiagnosticError> {
        parse_tick(self.as_str())
    }
}

impl TryFrom<String> for Tick {
    type Error = DiagnosticError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<Tick> for String {
    fn from(value: Tick) -> Self {
        value.0
    }
}

fn parse_tick(value: &str) -> Result<u64, DiagnosticError> {
    if value.len() > 20 {
        return Err(DiagnosticError::InvalidTick);
    }
    value
        .parse::<u64>()
        .ok()
        .filter(|n| n.to_string() == value)
        .ok_or(DiagnosticError::InvalidTick)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedState {
    Planned,
    Ready,
    Claimed,
    Running,
    ToolWait,
    Blocked,
    Review,
    VerifiedRecorded,
    IntegratedRecorded,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Knowledge {
    Known,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderLiveness {
    Live,
    NotLive,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidencePresence {
    Present,
    Missing,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceProvenance {
    Independent,
    SelfReported,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedStatus {
    pub contract_revision: u32,
    pub contract_digest: String,
    pub authority_ref: String,
    pub attempt_id: Option<AttemptId>,
    pub owner: Option<PrincipalId>,
    pub producer_id: String,
    pub producer_incarnation: String,
    pub source_sequence: Tick,
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub source_revision: String,
    pub source_tick: Option<Tick>,
    pub state: RecordedState,
    pub ownership: Knowledge,
    pub provider_liveness: ProviderLiveness,
    pub evidence_presence: EvidencePresence,
    /// Recorded assertion of origin; this component authenticates nothing.
    pub evidence_provenance: EvidenceProvenance,
    pub native_execution_qualified: bool,
    pub candidate_head: Option<String>,
    pub receipt_digest: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticPolicy {
    pub explicit_now: Option<Tick>,
    pub previous_source: Option<SourceProvenance>,
    pub stale_after: Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextAction {
    ReviewPlan,
    InspectRecordedBlocker,
    WaitForTool,
    ReviewCandidate,
    RequestMissingEvidence,
    ReconcileOwnership,
    RefreshSnapshot,
    RequestAuthority,
    ManualReview,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProvenance {
    pub producer_id: String,
    pub producer_incarnation: String,
    pub sequence: Tick,
    pub scope_digest: String,
    pub observation_digest: String,
    pub revision: String,
    pub tick: Option<Tick>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperatorDiagnostic {
    pub contract_revision: u32,
    pub contract_digest: String,
    pub authority_ref: String,
    pub attempt_id: Option<AttemptId>,
    pub owner: Option<PrincipalId>,
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub reason_codes: Vec<String>,
    pub explanation: String,
    pub source: SourceProvenance,
    pub candidate_head: Option<String>,
    pub receipt_digest: Option<String>,
    pub next_actions: Vec<NextAction>,
    execution_granted: bool,
    acceptance_granted: bool,
    recorded_only: bool,
    read_only: bool,
    pub reported_native_execution_qualified: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticError {
    TooLarge,
    InvalidRevision,
    InvalidTick,
    TickArithmetic,
    InvalidDigest,
    TextTooLong,
    InvalidScope,
    InvalidJson,
}
impl fmt::Display for DiagnosticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::TooLarge => "diagnostic input exceeds 1 MiB",
            Self::InvalidRevision => "source revision is not a safe bounded identifier",
            Self::InvalidTick => "tick must be a canonical decimal u64 string",
            Self::TickArithmetic => "tick arithmetic overflow or invalid temporal relationship",
            Self::InvalidDigest => {
                "candidate head must be a full Git object ID and receipt digest must be SHA-256"
            }
            Self::InvalidScope => "invalid recorded scope, source identity or policy",
            Self::InvalidJson => "invalid recorded diagnostic JSON",
            Self::TextTooLong => "input text exceeds 4096 bytes",
        };
        f.write_str(message)
    }
}
impl StdError for DiagnosticError {}

fn safe_revision(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        && value != "."
        && value != ".."
}
fn safe_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// Explain only the supplied immutable status. `explicit_now` is caller supplied;
/// this function has no clock, filesystem, process, or network access.
pub fn explain_status(
    status: &RecordedStatus,
    policy: &DiagnosticPolicy,
) -> Result<OperatorDiagnostic, DiagnosticError> {
    bounded_json_size(&(status, policy), MAX_INPUT_BYTES).map_err(|_| DiagnosticError::TooLarge)?;
    if status.source_revision.len() > MAX_TEXT_BYTES
        || status
            .candidate_head
            .as_ref()
            .is_some_and(|value| value.len() > MAX_TEXT_BYTES)
        || status
            .receipt_digest
            .as_ref()
            .is_some_and(|value| value.len() > MAX_TEXT_BYTES)
    {
        return Err(DiagnosticError::TextTooLong);
    }
    if status.contract_revision == 0
        || !safe_digest(&status.contract_digest)
        || status.authority_ref.is_empty()
        || status.authority_ref.len() > 512
        || status
            .authority_ref
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
        || !safe_revision(&status.producer_id)
        || !safe_revision(&status.producer_incarnation)
    {
        return Err(DiagnosticError::InvalidScope);
    }
    if status.source_sequence.value()? == 0 {
        return Err(DiagnosticError::InvalidScope);
    }
    if !safe_revision(&status.source_revision) {
        return Err(DiagnosticError::InvalidRevision);
    }
    if status
        .candidate_head
        .as_ref()
        .is_some_and(|v| !safe_git_head(v))
        || status
            .receipt_digest
            .as_ref()
            .is_some_and(|v| !safe_digest(v))
    {
        return Err(DiagnosticError::InvalidDigest);
    }

    let now = policy.explicit_now.as_ref().map(Tick::value).transpose()?;
    let threshold = policy.stale_after.value()?;
    if threshold == 0 {
        return Err(DiagnosticError::InvalidScope);
    }
    let scope_digest = content_digest(&(
        &status.project_id,
        &status.root_id,
        &status.task_id,
        status.contract_revision,
        &status.contract_digest,
        &status.authority_ref,
        &status.attempt_id,
        &status.owner,
    ))
    .map_err(|_| DiagnosticError::InvalidScope)?;
    let observation_digest = content_digest(status).map_err(|_| DiagnosticError::InvalidScope)?;
    let mut reasons = BTreeSet::new();
    let mut actions = BTreeSet::new();
    let mut current_source = false;
    if now.is_none() {
        reasons.insert("evaluation_clock_unknown");
        actions.insert(NextAction::RefreshSnapshot);
    }
    match (&status.source_tick, now) {
        (None, _) => {
            reasons.insert("source_tick_unknown");
            actions.insert(NextAction::RefreshSnapshot);
        }
        (Some(tick), Some(now)) => {
            let observed = tick.value()?;
            if observed > now {
                reasons.insert("source_tick_future");
                actions.insert(NextAction::RefreshSnapshot);
            } else if now
                .checked_sub(observed)
                .ok_or(DiagnosticError::TickArithmetic)?
                > threshold
            {
                reasons.insert("source_stale");
                actions.insert(NextAction::RefreshSnapshot);
            } else {
                current_source = true;
                reasons.insert("source_current_at_supplied_clock");
            }
        }
        (Some(tick), None) => {
            tick.value()?;
        }
    }
    if let Some(previous) = &policy.previous_source {
        if !safe_revision(&previous.producer_id)
            || !safe_revision(&previous.producer_incarnation)
            || !safe_revision(&previous.revision)
            || !safe_digest(&previous.scope_digest)
            || !safe_digest(&previous.observation_digest)
        {
            return Err(DiagnosticError::InvalidScope);
        }
        if previous.sequence.value()? == 0 {
            return Err(DiagnosticError::InvalidScope);
        }
        if let Some(tick) = &previous.tick {
            tick.value()?;
        }
        if previous.scope_digest != scope_digest {
            current_source = false;
            reasons.insert("previous_scope_mismatch");
            actions.insert(NextAction::ManualReview);
        } else if previous.producer_id != status.producer_id
            || previous.producer_incarnation != status.producer_incarnation
        {
            current_source = false;
            reasons.insert("source_incarnation_changed");
            actions.insert(NextAction::ReconcileOwnership);
        } else {
            let regressed = status.source_sequence.value()? < previous.sequence.value()?
                || match (&status.source_tick, &previous.tick) {
                    (Some(new), Some(old)) => new.value()? < old.value()?,
                    _ => false,
                };
            if regressed {
                current_source = false;
                reasons.insert("source_clock_or_sequence_regressed");
                actions.insert(NextAction::ManualReview);
            }
            if status.source_sequence == previous.sequence
                && observation_digest != previous.observation_digest
            {
                current_source = false;
                reasons.insert("source_identity_collision");
                actions.insert(NextAction::ManualReview);
            }
        }
        if !current_source {
            actions.insert(NextAction::RefreshSnapshot);
        }
    }

    match status.ownership {
        Knowledge::Known => {}
        Knowledge::Unknown => {
            reasons.insert("ownership_unknown");
            actions.insert(NextAction::ReconcileOwnership);
        }
    }
    match status.provider_liveness {
        ProviderLiveness::Live => {}
        ProviderLiveness::NotLive => {
            reasons.insert("provider_not_live");
            actions.insert(NextAction::RequestMissingEvidence);
        }
        ProviderLiveness::Unknown => {
            reasons.insert("provider_liveness_unknown");
            actions.insert(NextAction::RequestMissingEvidence);
        }
    }
    match status.evidence_presence {
        EvidencePresence::Present => {
            reasons.insert("evidence_present");
        }
        EvidencePresence::Missing => {
            reasons.insert("evidence_missing");
            actions.insert(NextAction::RequestMissingEvidence);
        }
        EvidencePresence::Unknown => {
            reasons.insert("evidence_presence_unknown");
            actions.insert(NextAction::RequestMissingEvidence);
        }
    }
    if !status.native_execution_qualified {
        reasons.insert("native_execution_unqualified");
        actions.insert(NextAction::ManualReview);
    }

    match status.state {
        RecordedState::Planned => {
            reasons.insert("task_planned");
            actions.insert(NextAction::ReviewPlan);
        }
        RecordedState::Ready => {
            reasons.insert("task_ready_recorded");
            actions.insert(NextAction::RequestAuthority);
        }
        RecordedState::Claimed => {
            reasons.insert("task_claimed_recorded");
            actions.insert(NextAction::WaitForTool);
        }
        RecordedState::Running => {
            reasons.insert("task_running_recorded");
            actions.insert(NextAction::WaitForTool);
        }
        RecordedState::ToolWait => {
            reasons.insert("provider_tool_wait");
            actions.insert(NextAction::WaitForTool);
        }
        RecordedState::Blocked => {
            reasons.insert("task_blocked_recorded");
            actions.insert(NextAction::InspectRecordedBlocker);
        }
        RecordedState::Review => {
            reasons.insert("candidate_review_pending");
            actions.insert(NextAction::ReviewCandidate);
        }
        RecordedState::VerifiedRecorded | RecordedState::IntegratedRecorded => {
            let verified = status.state == RecordedState::VerifiedRecorded;
            match (status.evidence_presence, status.evidence_provenance) {
                (EvidencePresence::Present, EvidenceProvenance::Independent) => {
                    reasons.insert(if verified {
                        "verification_independence_claim_recorded"
                    } else {
                        "integration_independence_claim_recorded"
                    });
                }
                (EvidencePresence::Present, EvidenceProvenance::SelfReported) => {
                    reasons.insert(if verified {
                        "verification_unconfirmed_self_report"
                    } else {
                        "integration_unconfirmed_self_report"
                    });
                    actions.insert(NextAction::RequestMissingEvidence);
                }
                _ => {
                    reasons.insert(if verified {
                        "verification_unconfirmed_missing_provenance"
                    } else {
                        "integration_unconfirmed_missing_provenance"
                    });
                    actions.insert(NextAction::RequestMissingEvidence);
                }
            }
            // Recorded verification/integration never means accepted by this diagnostic.
            actions.insert(NextAction::ManualReview);
        }
        RecordedState::Unknown => {
            reasons.insert("task_state_unknown");
            actions.insert(NextAction::RequestMissingEvidence);
        }
    }
    if status.evidence_presence == EvidencePresence::Present
        && status.evidence_provenance == EvidenceProvenance::Unknown
    {
        reasons.insert("evidence_provenance_unknown");
        actions.insert(NextAction::RequestMissingEvidence);
    }
    if status.evidence_provenance == EvidenceProvenance::Independent {
        reasons.insert("recorded_evidence_provenance_unverified");
        actions.insert(NextAction::ManualReview);
    }
    if status.native_execution_qualified {
        reasons.insert("native_qualification_claim_recorded");
        actions.insert(NextAction::ManualReview);
    }
    let bound_owner = status.ownership == Knowledge::Known
        && status.owner.is_some()
        && status.attempt_id.is_some();
    if status.ownership == Knowledge::Known && !bound_owner {
        reasons.insert("ownership_identity_missing");
        actions.insert(NextAction::ReconcileOwnership);
    }
    if !current_source || !bound_owner || status.provider_liveness != ProviderLiveness::Live {
        actions.remove(&NextAction::WaitForTool);
    }
    if !current_source
        || status.candidate_head.is_none()
        || status.evidence_presence != EvidencePresence::Present
    {
        actions.remove(&NextAction::ReviewCandidate);
        if status.state == RecordedState::Review {
            actions.insert(NextAction::RequestMissingEvidence);
        }
    }
    if !current_source {
        actions.remove(&NextAction::RequestAuthority);
    }
    if !current_source {
        reasons.remove("source_current_at_supplied_clock");
    }
    let explanation = match status.state {
        RecordedState::ToolWait => "The recorded provider is waiting for a tool; this does not establish an idle or stalled task.".to_owned(),
        RecordedState::VerifiedRecorded => "Verification is recorded, but this status alone does not confirm independent evidence or acceptance.".to_owned(),
        RecordedState::IntegratedRecorded => "Integration is recorded, but this status alone does not confirm independent evidence or acceptance.".to_owned(),
        RecordedState::Unknown => "The task state is unknown; the source has not been refreshed or inferred.".to_owned(),
        _ => "This explanation reflects only the supplied recorded status and evidence fields.".to_owned(),
    };
    Ok(OperatorDiagnostic {
        contract_revision: status.contract_revision,
        contract_digest: status.contract_digest.clone(),
        authority_ref: status.authority_ref.clone(),
        owner: status.owner.clone(),
        attempt_id: status.attempt_id.clone(),
        project_id: status.project_id.clone(),
        root_id: status.root_id.clone(),
        task_id: status.task_id.clone(),
        reason_codes: reasons.into_iter().map(str::to_owned).collect(),
        explanation,
        source: SourceProvenance {
            producer_id: status.producer_id.clone(),
            producer_incarnation: status.producer_incarnation.clone(),
            sequence: status.source_sequence.clone(),
            scope_digest,
            observation_digest,
            revision: status.source_revision.clone(),
            tick: status.source_tick.clone(),
        },
        candidate_head: status.candidate_head.clone(),
        receipt_digest: status.receipt_digest.clone(),
        next_actions: actions.into_iter().collect(),
        execution_granted: false,
        acceptance_granted: false,
        recorded_only: true,
        read_only: true,
        reported_native_execution_qualified: status.native_execution_qualified,
    })
}

impl OperatorDiagnostic {
    pub fn execution_granted(&self) -> bool {
        self.execution_granted
    }
    pub fn acceptance_granted(&self) -> bool {
        self.acceptance_granted
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticInput {
    pub schema_version: u32,
    pub status: RecordedStatus,
    pub policy: DiagnosticPolicy,
}
pub fn parse_and_explain(bytes: &[u8]) -> Result<OperatorDiagnostic, DiagnosticError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(DiagnosticError::TooLarge);
    }
    let input: DiagnosticInput =
        serde_json::from_slice(bytes).map_err(|_| DiagnosticError::InvalidJson)?;
    if input.schema_version != 1 {
        return Err(DiagnosticError::InvalidScope);
    }
    explain_status(&input.status, &input.policy)
}
fn safe_git_head(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value.bytes().any(|byte| byte != b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status(state: RecordedState) -> RecordedStatus {
        RecordedStatus {
            contract_revision: 1,
            contract_digest: format!("sha256:{}", "a".repeat(64)),
            authority_ref: "grant:recorded-task".into(),
            owner: Some(PrincipalId::new("owner").expect("fixture owner")),
            attempt_id: Some(AttemptId::new("attempt").expect("fixture attempt")),
            producer_id: "recorded-producer".into(),
            producer_incarnation: "boot-one".into(),
            source_sequence: Tick::new("1").expect("fixture sequence"),
            project_id: ProjectId::new("project").expect("project ID"),
            root_id: RootId::new("root").expect("root ID"),
            task_id: TaskId::new("task").expect("task ID"),
            source_revision: "rev-1".into(),
            source_tick: Some(Tick::new("10").expect("tick")),
            state,
            ownership: Knowledge::Known,
            provider_liveness: ProviderLiveness::Live,
            evidence_presence: EvidencePresence::Present,
            evidence_provenance: EvidenceProvenance::Independent,
            native_execution_qualified: false,
            candidate_head: Some("a".repeat(40)),
            receipt_digest: Some(format!("sha256:{}", "b".repeat(64))),
        }
    }
    fn policy(now: &str, stale: &str) -> DiagnosticPolicy {
        DiagnosticPolicy {
            explicit_now: Some(Tick::new(now).expect("clock tick")),
            previous_source: None,
            stale_after: Tick::new(stale).expect("stale duration"),
        }
    }
    #[test]
    fn tool_wait_is_not_stall_and_unknown_ownership_requires_reconciliation() {
        let mut s = status(RecordedState::ToolWait);
        s.ownership = Knowledge::Unknown;
        let d = explain_status(&s, &policy("10", "5")).expect("diagnostic");
        assert!(d.explanation.contains("not establish an idle or stalled"));
        assert!(!d.next_actions.contains(&NextAction::WaitForTool));
        assert!(d.next_actions.contains(&NextAction::ReconcileOwnership));
        assert!(!d.execution_granted);
    }
    #[test]
    fn clock_unknown_future_stale_and_overflow_are_explicit() {
        let mut s = status(RecordedState::Running);
        s.source_tick = None;
        assert!(
            explain_status(&s, &policy("10", "5"))
                .expect("unknown tick")
                .reason_codes
                .contains(&"source_tick_unknown".into())
        );
        s.source_tick = Some(Tick::new("11").expect("future tick"));
        assert!(
            explain_status(&s, &policy("10", "5"))
                .expect("future tick diagnostic")
                .reason_codes
                .contains(&"source_tick_future".into())
        );
        s.source_tick = Some(Tick::new("1").expect("old tick"));
        assert!(
            explain_status(&s, &policy("10", "5"))
                .expect("stale tick")
                .reason_codes
                .contains(&"source_stale".into())
        );
        assert_eq!(
            Tick::new("18446744073709551615").expect("max tick").value(),
            Ok(u64::MAX)
        );
        assert_eq!(
            Tick::new("18446744073709551616"),
            Err(DiagnosticError::InvalidTick)
        );
    }
    #[test]
    fn verified_and_integrated_self_reports_are_not_acceptance() {
        for (state, reason) in [
            (
                RecordedState::VerifiedRecorded,
                "verification_unconfirmed_self_report",
            ),
            (
                RecordedState::IntegratedRecorded,
                "integration_unconfirmed_self_report",
            ),
        ] {
            let mut s = status(state);
            s.evidence_provenance = EvidenceProvenance::SelfReported;
            let d = explain_status(&s, &policy("10", "5")).expect("diagnostic");
            assert!(d.reason_codes.contains(&reason.into()));
            assert!(d.explanation.contains("does not confirm"));
            assert!(!d.execution_granted);
        }
    }
    #[test]
    fn each_recorded_state_has_a_stable_reason_and_typed_action() {
        let cases = [
            (
                RecordedState::Planned,
                "task_planned",
                NextAction::ReviewPlan,
            ),
            (
                RecordedState::Ready,
                "task_ready_recorded",
                NextAction::RequestAuthority,
            ),
            (
                RecordedState::Claimed,
                "task_claimed_recorded",
                NextAction::WaitForTool,
            ),
            (
                RecordedState::Running,
                "task_running_recorded",
                NextAction::WaitForTool,
            ),
            (
                RecordedState::ToolWait,
                "provider_tool_wait",
                NextAction::WaitForTool,
            ),
            (
                RecordedState::Blocked,
                "task_blocked_recorded",
                NextAction::InspectRecordedBlocker,
            ),
            (
                RecordedState::Review,
                "candidate_review_pending",
                NextAction::ReviewCandidate,
            ),
            (
                RecordedState::VerifiedRecorded,
                "verification_independence_claim_recorded",
                NextAction::ManualReview,
            ),
            (
                RecordedState::IntegratedRecorded,
                "integration_independence_claim_recorded",
                NextAction::ManualReview,
            ),
            (
                RecordedState::Unknown,
                "task_state_unknown",
                NextAction::RequestMissingEvidence,
            ),
        ];
        for (state, reason, action) in cases {
            let d = explain_status(&status(state), &policy("10", "5")).expect("diagnostic");
            assert!(d.reason_codes.iter().any(|code| code == reason));
            assert!(d.next_actions.contains(&action));
            assert!(!d.execution_granted);
        }
    }
    #[test]
    fn hostile_freeform_strings_are_rejected_and_never_echoed() {
        let mut s = status(RecordedState::Running);
        s.source_revision = "secret=token".into();
        assert_eq!(
            explain_status(&s, &policy("10", "5")),
            Err(DiagnosticError::InvalidRevision)
        );
        s.source_revision = "rev-1".into();
        s.candidate_head = Some("ssh://secret".into());
        assert_eq!(
            explain_status(&s, &policy("10", "5")),
            Err(DiagnosticError::InvalidDigest)
        );
    }
    #[test]
    fn replay_is_deterministic_and_input_caps_are_enforced() {
        let s = status(RecordedState::Review);
        let p = policy("10", "5");
        assert_eq!(explain_status(&s, &p), explain_status(&s, &p));
        let mut oversized = s;
        oversized.source_revision = "r".repeat(MAX_TEXT_BYTES + 1);
        assert_eq!(
            explain_status(&oversized, &p),
            Err(DiagnosticError::TextTooLong)
        );
        let mut invalid = status(RecordedState::Ready);
        invalid.source_tick = Some(Tick("01".into()));
        assert_eq!(
            explain_status(&invalid, &p),
            Err(DiagnosticError::InvalidTick)
        );
        let codes = BTreeSet::from(["task_ready_recorded".to_string()]);
        assert!(
            explain_status(&status(RecordedState::Ready), &p)
                .expect("diagnostic")
                .reason_codes
                .iter()
                .any(|v| codes.contains(v))
        );
    }
    #[test]
    fn unknown_stale_or_unowned_source_does_not_recommend_waiting_or_candidate_review() {
        for issue in ["unknown-now", "unknown-tick", "stale", "not-live", "owner"] {
            let mut s = status(RecordedState::ToolWait);
            let mut p = policy("20", "5");
            match issue {
                "unknown-now" => p.explicit_now = None,
                "unknown-tick" => s.source_tick = None,
                "not-live" => {
                    s.provider_liveness = ProviderLiveness::NotLive;
                    p = policy("10", "5");
                }
                "owner" => {
                    s.owner = None;
                    p = policy("10", "5");
                }
                _ => {}
            }
            let result = explain_status(&s, &p).expect("missing evidence remains explainable");
            assert!(!result.next_actions.contains(&NextAction::WaitForTool));
            assert!(!result.execution_granted() && !result.acceptance_granted());
        }
        let mut s = status(RecordedState::Review);
        s.candidate_head = None;
        let result =
            explain_status(&s, &policy("10", "5")).expect("missing candidate stays unknown");
        assert!(!result.next_actions.contains(&NextAction::ReviewCandidate));
        assert!(
            result
                .next_actions
                .contains(&NextAction::RequestMissingEvidence)
        );
    }
    #[test]
    fn exact_source_history_detects_clock_regression_identity_collision_and_scope_changes() {
        let mut s = status(RecordedState::Running);
        let mut p = policy("10", "5");
        let previous = explain_status(&s, &p)
            .expect("prior recorded source")
            .source;
        p.previous_source = Some(previous.clone());
        s.source_tick = Some(Tick::new("9").expect("fixture tick"));
        let result = explain_status(&s, &p).expect("regression explained");
        assert!(
            result
                .reason_codes
                .contains(&"source_clock_or_sequence_regressed".into())
        );
        assert!(
            result
                .reason_codes
                .contains(&"source_identity_collision".into())
        );
        assert!(
            !result
                .reason_codes
                .contains(&"source_current_at_supplied_clock".into())
        );
        assert!(!result.next_actions.contains(&NextAction::WaitForTool));
        s = status(RecordedState::Running);
        s.attempt_id = Some(AttemptId::new("other").expect("fixture attempt"));
        assert!(
            explain_status(&s, &p)
                .expect("scope change explained")
                .reason_codes
                .contains(&"previous_scope_mismatch".into())
        );
        s = status(RecordedState::Running);
        s.producer_incarnation = "new-boot".into();
        assert!(
            explain_status(&s, &p)
                .expect("new source incarnation is explicit")
                .reason_codes
                .contains(&"source_incarnation_changed".into())
        );
        s = status(RecordedState::Running);
        s.source_sequence = Tick::new("0").expect("canonical but not a source sequence");
        assert_eq!(explain_status(&s, &p), Err(DiagnosticError::InvalidScope));
    }
    #[test]
    fn strict_json_never_promotes_self_reported_qualification_or_injected_trust_flags() {
        let mut s = status(RecordedState::IntegratedRecorded);
        s.native_execution_qualified = true;
        let input = DiagnosticInput {
            schema_version: 1,
            status: s,
            policy: policy("10", "5"),
        };
        let bytes = serde_json::to_vec(&input).expect("fixture JSON");
        let result = parse_and_explain(&bytes).expect("recorded claim remains usable");
        assert!(
            result
                .reason_codes
                .contains(&"native_qualification_claim_recorded".into())
        );
        assert!(
            result
                .reason_codes
                .contains(&"recorded_evidence_provenance_unverified".into())
        );
        let output = serde_json::to_value(&result).expect("sealed diagnostic output");
        assert_eq!(output["execution_granted"], false);
        assert_eq!(output["acceptance_granted"], false);
        assert_eq!(output["recorded_only"], true);
        let mut value = serde_json::to_value(input).expect("fixture JSON");
        value["status"]["execution_granted"] = serde_json::json!(true);
        assert_eq!(
            parse_and_explain(&serde_json::to_vec(&value).expect("fixture bytes")),
            Err(DiagnosticError::InvalidJson)
        );
        assert_eq!(
            parse_and_explain(&vec![b' '; MAX_INPUT_BYTES + 1]),
            Err(DiagnosticError::TooLarge)
        );
        assert_eq!(
            parse_and_explain(br#"{"schema_version":1,"schema_version":1}"#),
            Err(DiagnosticError::InvalidJson)
        );
    }
    #[test]
    fn typed_admission_is_streaming_and_git_head_identity_is_distinct_from_receipt_digest() {
        let mut s = status(RecordedState::Review);
        s.source_revision = "x".repeat(MAX_INPUT_BYTES + 1);
        assert_eq!(
            explain_status(&s, &policy("10", "5")),
            Err(DiagnosticError::TooLarge)
        );
        s = status(RecordedState::Review);
        s.candidate_head = Some(format!("sha256:{}", "a".repeat(64)));
        assert_eq!(
            explain_status(&s, &policy("10", "5")),
            Err(DiagnosticError::InvalidDigest)
        );
        s.candidate_head = Some("0".repeat(40));
        assert_eq!(
            explain_status(&s, &policy("10", "5")),
            Err(DiagnosticError::InvalidDigest)
        );
        s = status(RecordedState::Review);
        s.contract_revision = 0;
        assert_eq!(
            explain_status(&s, &policy("10", "5")),
            Err(DiagnosticError::InvalidScope)
        );
    }
}
