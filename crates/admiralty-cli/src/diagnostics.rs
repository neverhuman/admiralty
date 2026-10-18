//! Pure, bounded operator explanations for recorded task state.
use admiralty_core::{ProjectId, RootId, TaskId};
use std::collections::BTreeSet;
use std::{error::Error as StdError, fmt};

pub const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_TEXT_BYTES: usize = 4096;

/// Canonical decimal u64. Strings preserve precision across JSON/JavaScript.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
        parse_tick(&self.0)
    }
}

fn parse_tick(value: &str) -> Result<u64, DiagnosticError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| n.to_string() == value)
        .ok_or(DiagnosticError::InvalidTick)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Knowledge {
    Known,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderLiveness {
    Live,
    NotLive,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidencePresence {
    Present,
    Missing,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceProvenance {
    Independent,
    SelfReported,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedStatus {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub source_revision: String,
    pub source_tick: Option<Tick>,
    pub state: RecordedState,
    pub ownership: Knowledge,
    pub provider_liveness: ProviderLiveness,
    pub evidence_presence: EvidencePresence,
    /// States whether the evidence's origin is independently authenticated.
    pub evidence_provenance: EvidenceProvenance,
    pub native_execution_qualified: bool,
    pub candidate_head: Option<String>,
    pub receipt_digest: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticPolicy {
    pub explicit_now: Tick,
    pub stale_after: Tick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NextAction {
    WaitForTool,
    ReviewCandidate,
    RequestMissingEvidence,
    ReconcileOwnership,
    RefreshSnapshot,
    RequestAuthority,
    ManualReview,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceProvenance {
    pub revision: String,
    pub tick: Option<Tick>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorDiagnostic {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub reason_codes: Vec<String>,
    pub explanation: String,
    pub source: SourceProvenance,
    pub candidate_head: Option<String>,
    pub receipt_digest: Option<String>,
    pub next_actions: Vec<NextAction>,
    pub execution_granted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosticError {
    TooLarge,
    InvalidRevision,
    InvalidTick,
    TickArithmetic,
    InvalidDigest,
    TextTooLong,
}
impl fmt::Display for DiagnosticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::TooLarge => "diagnostic input exceeds 1 MiB",
            Self::InvalidRevision => "source revision is not a safe bounded identifier",
            Self::InvalidTick => "tick must be a canonical decimal u64 string",
            Self::TickArithmetic => "tick arithmetic overflow or invalid temporal relationship",
            Self::InvalidDigest => "candidate head or receipt digest is not a SHA-256 identity",
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
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Explain only the supplied immutable status. `explicit_now` is caller supplied;
/// this function has no clock, filesystem, process, or network access.
pub fn explain_status(
    status: &RecordedStatus,
    policy: &DiagnosticPolicy,
) -> Result<OperatorDiagnostic, DiagnosticError> {
    let bounded_strings = [
        status.source_revision.as_str(),
        status.candidate_head.as_deref().unwrap_or(""),
        status.receipt_digest.as_deref().unwrap_or(""),
        status.source_tick.as_ref().map(Tick::as_str).unwrap_or(""),
        policy.explicit_now.as_str(),
        policy.stale_after.as_str(),
    ];
    if bounded_strings
        .iter()
        .any(|value| value.len() > MAX_TEXT_BYTES)
    {
        return Err(DiagnosticError::TextTooLong);
    }
    let encoded_size = bounded_strings
        .iter()
        .try_fold(512usize, |size, value| size.checked_add(value.len()))
        .ok_or(DiagnosticError::TooLarge)?;
    if encoded_size > MAX_INPUT_BYTES {
        return Err(DiagnosticError::TooLarge);
    }
    if status.source_revision.len() > MAX_TEXT_BYTES {
        return Err(DiagnosticError::TextTooLong);
    }
    if !safe_revision(&status.source_revision) {
        return Err(DiagnosticError::InvalidRevision);
    }
    if status
        .candidate_head
        .as_ref()
        .is_some_and(|v| !safe_digest(v))
        || status
            .receipt_digest
            .as_ref()
            .is_some_and(|v| !safe_digest(v))
    {
        return Err(DiagnosticError::InvalidDigest);
    }

    let now = policy.explicit_now.value()?;
    let threshold = policy.stale_after.value()?;
    let mut reasons = BTreeSet::new();
    let mut actions = BTreeSet::new();
    match &status.source_tick {
        None => {
            reasons.insert("source_tick_unknown");
            actions.insert(NextAction::RefreshSnapshot);
        }
        Some(tick) => {
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
                reasons.insert("source_current_at_supplied_clock");
            }
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
            actions.insert(NextAction::WaitForTool);
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
            actions.insert(NextAction::RequestAuthority);
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
                        "independent_verification_recorded"
                    } else {
                        "independent_integration_recorded"
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
    let explanation = match status.state {
        RecordedState::ToolWait => "The recorded provider is waiting for a tool; this does not establish an idle or stalled task.".to_owned(),
        RecordedState::VerifiedRecorded => "Verification is recorded, but this status alone does not confirm independent evidence or acceptance.".to_owned(),
        RecordedState::IntegratedRecorded => "Integration is recorded, but this status alone does not confirm independent evidence or acceptance.".to_owned(),
        RecordedState::Unknown => "The task state is unknown; the source has not been refreshed or inferred.".to_owned(),
        _ => "This explanation reflects only the supplied recorded status and evidence fields.".to_owned(),
    };
    Ok(OperatorDiagnostic {
        project_id: status.project_id.clone(),
        root_id: status.root_id.clone(),
        task_id: status.task_id.clone(),
        reason_codes: reasons.into_iter().map(str::to_owned).collect(),
        explanation,
        source: SourceProvenance {
            revision: status.source_revision.clone(),
            tick: status.source_tick.clone(),
        },
        candidate_head: status.candidate_head.clone(),
        receipt_digest: status.receipt_digest.clone(),
        next_actions: actions.into_iter().collect(),
        execution_granted: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status(state: RecordedState) -> RecordedStatus {
        RecordedStatus {
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
            candidate_head: Some(format!("sha256:{}", "a".repeat(64))),
            receipt_digest: Some(format!("sha256:{}", "b".repeat(64))),
        }
    }
    fn policy(now: &str, stale: &str) -> DiagnosticPolicy {
        DiagnosticPolicy {
            explicit_now: Tick::new(now).expect("clock tick"),
            stale_after: Tick::new(stale).expect("stale duration"),
        }
    }
    #[test]
    fn tool_wait_is_not_stall_and_unknown_ownership_requires_reconciliation() {
        let mut s = status(RecordedState::ToolWait);
        s.ownership = Knowledge::Unknown;
        let d = explain_status(&s, &policy("10", "5")).expect("diagnostic");
        assert!(d.explanation.contains("not establish an idle or stalled"));
        assert!(d.next_actions.contains(&NextAction::WaitForTool));
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
                NextAction::WaitForTool,
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
                NextAction::RequestAuthority,
            ),
            (
                RecordedState::Review,
                "candidate_review_pending",
                NextAction::ReviewCandidate,
            ),
            (
                RecordedState::VerifiedRecorded,
                "independent_verification_recorded",
                NextAction::ManualReview,
            ),
            (
                RecordedState::IntegratedRecorded,
                "independent_integration_recorded",
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
}
