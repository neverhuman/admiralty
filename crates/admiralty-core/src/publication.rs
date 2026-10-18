//! Pure publication admission and effect reconciliation contracts.
//!
//! This module proposes work only. It has no forge, credential, clock, or process API.
use crate::{ProjectId, RootId, TaskId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_ITEMS: usize = 1024;
pub const MAX_STRING_BYTES: usize = 4096;

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct DecimalU64(String);

impl DecimalU64 {
    pub fn new(value: u64) -> Self {
        Self(value.to_string())
    }
    pub fn parse(&self) -> Result<u64, PublicationError> {
        let value = self
            .0
            .parse::<u64>()
            .map_err(|_| PublicationError::InvalidDecimal)?;
        if value.to_string() != self.0 {
            return Err(PublicationError::InvalidDecimal);
        }
        Ok(value)
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for DecimalU64 {
    type Error = PublicationError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let parsed = Self(value);
        parsed.parse()?;
        Ok(parsed)
    }
}
impl From<DecimalU64> for String {
    fn from(value: DecimalU64) -> Self {
        value.0
    }
}
impl fmt::Display for DecimalU64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub target_ref: String,
    pub authority_generation: DecimalU64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ApprovalEvidence {
    pub reviewer: String,
    pub head: String,
    pub dismissed: bool,
    /// Independent forge/evidence source identity, not a worker supplied assertion.
    pub provenance_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckEvidence {
    pub check_id: String,
    pub head: String,
    pub state: CheckState,
    pub provenance_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Passed,
    Pending,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationSnapshot {
    pub current_head: Option<String>,
    pub current_base: Option<String>,
    pub expected_head: Option<String>,
    pub expected_base: Option<String>,
    pub required_reviewers: Vec<String>,
    pub approvals: Vec<ApprovalEvidence>,
    pub required_checks: Vec<String>,
    pub checks: Vec<CheckEvidence>,
    pub owner_only: bool,
    pub caller_is_owner: bool,
    pub unresolved_intent: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationIntent {
    pub intent_key: String,
    pub request_digest: String,
    pub scope: PublicationScope,
    pub expected_head: String,
    pub expected_base: String,
    pub authority_generation: DecimalU64,
    pub stage: IntentStage,
    pub effect_receipt: Option<ReconciliationReceipt>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntentStage {
    Proposed,
    Claimed,
    Ambiguous,
    ReconciledSuccess,
    ReconciledNoEffect,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationReceipt {
    pub intent_key: String,
    pub request_digest: String,
    pub observed_head: String,
    pub effect_occurred: bool,
    pub provenance_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationProposal {
    pub eligible: bool,
    pub execution_granted: bool,
    pub reasons: Vec<PublicationBlock>,
    pub intent: Option<PublicationIntent>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PublicationBlock {
    MissingHeadOrBase,
    ChangedHead,
    ChangedBase,
    WrongReviewer,
    DismissedApproval,
    MissingApproval,
    MissingOrUnknownCheck,
    CheckNotPassed,
    OwnerOnly,
    StaleAuthorityGeneration,
    UnresolvedEffectSlot,
    InvalidEvidence,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PublicationError {
    #[error("invalid canonical decimal u64")]
    InvalidDecimal,
    #[error("authority generation overflow")]
    GenerationOverflow,
    #[error("input exceeds size or collection limits")]
    LimitExceeded,
    #[error("intent key collision: same key with different request bytes")]
    KeyCollision,
    #[error("ambiguous intent requires an independent matching reconciliation receipt")]
    InvalidReconciliation,
    #[error("reconciliation receipt conflicts with prior receipt")]
    ReceiptCollision,
    #[error("one unresolved effect slot already exists for this target")]
    EffectSlotOccupied,
}

pub fn next_authority_generation(current: &DecimalU64) -> Result<DecimalU64, PublicationError> {
    Ok(DecimalU64::new(
        current
            .parse()?
            .checked_add(1)
            .ok_or(PublicationError::GenerationOverflow)?,
    ))
}

/// Checks bounded serialized input before callers deserialize it.
pub fn validate_input_size(bytes: &[u8]) -> Result<(), PublicationError> {
    if bytes.len() > MAX_JSON_BYTES {
        return Err(PublicationError::LimitExceeded);
    }
    Ok(())
}

pub fn propose_publication(
    scope: &PublicationScope,
    snapshot: &PublicationSnapshot,
    intent_key: &str,
    request_digest: &str,
    proposed_generation: &DecimalU64,
) -> PublicationProposal {
    let mut reasons = Vec::new();
    let valid_strings = [scope.target_ref.as_str(), intent_key, request_digest]
        .iter()
        .all(|s| !s.is_empty() && s.len() <= MAX_STRING_BYTES);
    let evidence_strings_valid = snapshot
        .current_head
        .iter()
        .chain(snapshot.current_base.iter())
        .chain(snapshot.expected_head.iter())
        .chain(snapshot.expected_base.iter())
        .chain(snapshot.required_reviewers.iter())
        .chain(snapshot.required_checks.iter())
        .chain(
            snapshot
                .approvals
                .iter()
                .flat_map(|a| [&a.reviewer, &a.head, &a.provenance_ref]),
        )
        .chain(
            snapshot
                .checks
                .iter()
                .flat_map(|c| [&c.check_id, &c.head, &c.provenance_ref]),
        )
        .all(|s| !s.is_empty() && s.len() <= MAX_STRING_BYTES);
    if !valid_strings
        || !evidence_strings_valid
        || snapshot.required_reviewers.len() > MAX_ITEMS
        || snapshot.required_checks.len() > MAX_ITEMS
        || snapshot.approvals.len() > MAX_ITEMS
        || snapshot.checks.len() > MAX_ITEMS
    {
        reasons.push(PublicationBlock::InvalidEvidence);
    }
    let (head, base, expected_head, expected_base) = match (
        &snapshot.current_head,
        &snapshot.current_base,
        &snapshot.expected_head,
        &snapshot.expected_base,
    ) {
        (Some(h), Some(b), Some(eh), Some(eb)) => (h, b, eh, eb),
        _ => {
            reasons.push(PublicationBlock::MissingHeadOrBase);
            return blocked(reasons);
        }
    };
    if head != expected_head {
        reasons.push(PublicationBlock::ChangedHead);
    }
    if base != expected_base {
        reasons.push(PublicationBlock::ChangedBase);
    }
    let configured: BTreeSet<_> = snapshot.required_reviewers.iter().collect();
    if snapshot.approvals.iter().any(|approval| {
        !configured.contains(&approval.reviewer) && !approval.provenance_ref.is_empty()
    }) {
        reasons.push(PublicationBlock::WrongReviewer);
    }
    for reviewer in configured {
        let matching: Vec<_> = snapshot
            .approvals
            .iter()
            .filter(|a| &a.reviewer == reviewer)
            .collect();
        if matching.iter().any(|a| a.dismissed) {
            reasons.push(PublicationBlock::DismissedApproval);
            continue;
        }
        if matching
            .iter()
            .any(|a| a.head == *head && !a.provenance_ref.is_empty())
        {
            continue;
        }
        if matching.iter().any(|a| !a.provenance_ref.is_empty()) {
            reasons.push(PublicationBlock::WrongReviewer);
        } else {
            reasons.push(PublicationBlock::MissingApproval);
        }
    }
    let checks: BTreeSet<_> = snapshot.required_checks.iter().collect();
    for id in checks {
        let found: Vec<_> = snapshot
            .checks
            .iter()
            .filter(|c| &c.check_id == id && c.head == *head && !c.provenance_ref.is_empty())
            .collect();
        if found.is_empty() || found.iter().any(|c| c.state == CheckState::Unknown) {
            reasons.push(PublicationBlock::MissingOrUnknownCheck);
        } else if !found.iter().any(|c| c.state == CheckState::Passed) {
            reasons.push(PublicationBlock::CheckNotPassed);
        }
    }
    if snapshot.owner_only && !snapshot.caller_is_owner {
        reasons.push(PublicationBlock::OwnerOnly);
    }
    if snapshot.unresolved_intent {
        reasons.push(PublicationBlock::UnresolvedEffectSlot);
    }
    match (
        scope.authority_generation.parse(),
        proposed_generation.parse(),
    ) {
        (Ok(current), Ok(proposed)) if proposed == current => (),
        _ => reasons.push(PublicationBlock::StaleAuthorityGeneration),
    }
    if !reasons.is_empty() {
        return blocked(reasons);
    }
    PublicationProposal {
        eligible: true,
        execution_granted: false,
        reasons,
        intent: Some(PublicationIntent {
            intent_key: intent_key.to_owned(),
            request_digest: request_digest.to_owned(),
            scope: scope.clone(),
            expected_head: head.clone(),
            expected_base: base.clone(),
            authority_generation: proposed_generation.clone(),
            stage: IntentStage::Proposed,
            effect_receipt: None,
        }),
    }
}

fn blocked(reasons: Vec<PublicationBlock>) -> PublicationProposal {
    PublicationProposal {
        eligible: false,
        execution_granted: false,
        reasons,
        intent: None,
    }
}

/// Applies an idempotent local intent transition; an ambiguous slot is freed only by independent reconciliation.
pub fn reduce_publication_intent(
    existing: Option<&PublicationIntent>,
    incoming: &PublicationIntent,
    receipt: Option<&ReconciliationReceipt>,
) -> Result<PublicationIntent, PublicationError> {
    if incoming.intent_key.is_empty() || incoming.request_digest.is_empty() {
        return Err(PublicationError::LimitExceeded);
    }
    let Some(old) = existing else {
        if incoming.stage != IntentStage::Proposed {
            return Err(PublicationError::InvalidReconciliation);
        }
        return Ok(incoming.clone());
    };
    if old.intent_key != incoming.intent_key {
        return Err(PublicationError::EffectSlotOccupied);
    }
    if old.request_digest != incoming.request_digest
        || old.scope != incoming.scope
        || old.expected_head != incoming.expected_head
        || old.expected_base != incoming.expected_base
    {
        return Err(PublicationError::KeyCollision);
    }
    if old == incoming && matches!(old.stage, IntentStage::Proposed | IntentStage::Claimed)
        || old == incoming && old.stage == IntentStage::Ambiguous && receipt.is_none()
    {
        return Ok(old.clone());
    }
    match old.stage {
        IntentStage::Proposed if incoming.stage == IntentStage::Claimed => Ok(incoming.clone()),
        IntentStage::Claimed if incoming.stage == IntentStage::Ambiguous => Ok(incoming.clone()),
        IntentStage::Ambiguous => {
            let proof = receipt.ok_or(PublicationError::InvalidReconciliation)?;
            if proof.intent_key != old.intent_key
                || proof.request_digest != old.request_digest
                || proof.provenance_ref.is_empty()
            {
                return Err(PublicationError::InvalidReconciliation);
            }
            let mut result = old.clone();
            result.stage = if proof.effect_occurred {
                IntentStage::ReconciledSuccess
            } else {
                IntentStage::ReconciledNoEffect
            };
            result.effect_receipt = Some(proof.clone());
            Ok(result)
        }
        IntentStage::ReconciledSuccess | IntentStage::ReconciledNoEffect => {
            if old.effect_receipt.as_ref() == receipt {
                Ok(old.clone())
            } else {
                Err(PublicationError::ReceiptCollision)
            }
        }
        _ => Err(PublicationError::InvalidReconciliation),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixtures() -> (PublicationScope, PublicationSnapshot, DecimalU64) {
        let scope = PublicationScope {
            project_id: ProjectId::new("p").expect("valid project id"),
            root_id: RootId::new("r").expect("valid root id"),
            task_id: TaskId::new("t").expect("valid task id"),
            target_ref: "refs/heads/main".into(),
            authority_generation: DecimalU64::new(9),
        };
        let snapshot = PublicationSnapshot {
            current_head: Some("h1".into()),
            current_base: Some("b1".into()),
            expected_head: Some("h1".into()),
            expected_base: Some("b1".into()),
            required_reviewers: vec!["reviewer".into()],
            approvals: vec![ApprovalEvidence {
                reviewer: "reviewer".into(),
                head: "h1".into(),
                dismissed: false,
                provenance_ref: "forge:approval:1".into(),
            }],
            required_checks: vec!["ci".into()],
            checks: vec![CheckEvidence {
                check_id: "ci".into(),
                head: "h1".into(),
                state: CheckState::Passed,
                provenance_ref: "forge:check:1".into(),
            }],
            owner_only: false,
            caller_is_owner: false,
            unresolved_intent: false,
        };
        (scope, snapshot, DecimalU64::new(9))
    }
    fn proposal() -> PublicationIntent {
        let (s, p, g) = fixtures();
        propose_publication(&s, &p, "k", "sha256:request", &g)
            .intent
            .expect("valid evidence proposes an intent")
    }
    #[test]
    fn eligible_is_proposal_only_and_replay_is_deterministic() {
        let (s, p, g) = fixtures();
        let a = propose_publication(&s, &p, "k", "d", &g);
        let b = propose_publication(&s, &p, "k", "d", &g);
        assert!(a.eligible);
        assert!(!a.execution_granted);
        assert_eq!(a, b);
    }
    #[test]
    fn changed_heads_bases_unknown_checks_and_owner_only_block() {
        let (s, mut p, g) = fixtures();
        p.approvals[0].reviewer = "stranger".into();
        assert!(
            propose_publication(&s, &p, "k", "d", &g)
                .reasons
                .contains(&PublicationBlock::WrongReviewer)
        );
        let (s, mut p, g) = fixtures();
        p.current_head = Some("h2".into());
        assert!(!propose_publication(&s, &p, "k", "d", &g).eligible);
        let (s, mut p, g) = fixtures();
        p.current_base = Some("b2".into());
        assert!(!propose_publication(&s, &p, "k", "d", &g).eligible);
        let (s, mut p, g) = fixtures();
        p.checks[0].state = CheckState::Pending;
        assert!(!propose_publication(&s, &p, "k", "d", &g).eligible);
        let (s, mut p, g) = fixtures();
        p.checks[0].state = CheckState::Unknown;
        assert!(
            propose_publication(&s, &p, "k", "d", &g)
                .reasons
                .contains(&PublicationBlock::MissingOrUnknownCheck)
        );
        let (s, mut p, g) = fixtures();
        p.owner_only = true;
        assert!(
            propose_publication(&s, &p, "k", "d", &g)
                .reasons
                .contains(&PublicationBlock::OwnerOnly)
        );
        let (s, mut p, g) = fixtures();
        p.approvals[0].dismissed = true;
        assert!(
            propose_publication(&s, &p, "k", "d", &g)
                .reasons
                .contains(&PublicationBlock::DismissedApproval)
        );
    }
    #[test]
    fn stale_generation_and_overflow_are_explicit() {
        let (s, p, _) = fixtures();
        let stale = DecimalU64::new(8);
        assert!(
            propose_publication(&s, &p, "k", "d", &stale)
                .reasons
                .contains(&PublicationBlock::StaleAuthorityGeneration)
        );
        let newer = DecimalU64::new(10);
        assert!(
            propose_publication(&s, &p, "k", "d", &newer)
                .reasons
                .contains(&PublicationBlock::StaleAuthorityGeneration)
        );
        let max = DecimalU64::try_from(u64::MAX.to_string()).expect("max counter parses exactly");
        assert_eq!(
            next_authority_generation(&max),
            Err(PublicationError::GenerationOverflow)
        );
    }
    #[test]
    fn ambiguous_timeout_holds_slot_until_independent_receipt() {
        let first = proposal();
        let claimed =
            reduce_publication_intent(None, &first, None).expect("proposal enters the intent log");
        let mut claimed = claimed;
        claimed.stage = IntentStage::Claimed;
        let claimed =
            reduce_publication_intent(Some(&first), &claimed, None).expect("claim is recorded");
        let mut ambiguous = claimed.clone();
        ambiguous.stage = IntentStage::Ambiguous;
        let ambiguous = reduce_publication_intent(Some(&claimed), &ambiguous, None)
            .expect("timeout becomes ambiguous");
        assert_eq!(
            reduce_publication_intent(Some(&ambiguous), &ambiguous, None)
                .expect("ambiguous replay is idempotent"),
            ambiguous
        );
        let proof = ReconciliationReceipt {
            intent_key: "k".into(),
            request_digest: "sha256:request".into(),
            observed_head: "h2".into(),
            effect_occurred: true,
            provenance_ref: "forge:receipt:2".into(),
        };
        let done = reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&proof))
            .expect("remote success reconciles the slot");
        assert_eq!(done.stage, IntentStage::ReconciledSuccess);
        assert_eq!(
            reduce_publication_intent(Some(&done), &done, Some(&proof))
                .expect("receipt replay is idempotent"),
            done
        );
    }
    #[test]
    fn key_and_receipt_collisions_fail() {
        let first = proposal();
        let mut altered = first.clone();
        altered.request_digest = "other".into();
        assert_eq!(
            reduce_publication_intent(Some(&first), &altered, None),
            Err(PublicationError::KeyCollision)
        );
        let mut ambiguous = first.clone();
        ambiguous.stage = IntentStage::Ambiguous;
        let proof = ReconciliationReceipt {
            intent_key: "k".into(),
            request_digest: "sha256:request".into(),
            observed_head: "h".into(),
            effect_occurred: false,
            provenance_ref: "forge:r".into(),
        };
        let reconciled = reduce_publication_intent(Some(&ambiguous), &ambiguous, Some(&proof))
            .expect("receipt reconciles ambiguity");
        let changed = ReconciliationReceipt {
            effect_occurred: true,
            ..proof
        };
        assert_eq!(
            reduce_publication_intent(Some(&reconciled), &reconciled, Some(&changed)),
            Err(PublicationError::ReceiptCollision)
        );
    }
}
