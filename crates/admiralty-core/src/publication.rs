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
    pub admission_snapshot: PublicationSnapshot,
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
    pub scope: PublicationScope,
    pub expected_head: String,
    pub expected_base: String,
    pub authority_generation: DecimalU64,
    pub observed_head: String,
    pub effect_occurred: bool,
    pub provenance_ref: String,
}

#[derive(Clone, Debug, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicationProposal {
    pub eligible: bool,
    execution_granted: bool,
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

impl PublicationProposal {
    pub fn execution_granted(&self) -> bool {
        self.execution_granted
    }
}

/// Binds all frozen admission observations, policy, scope, key and generation.
/// A digest establishes consistency only; it does not authenticate an observer.
pub fn publication_request_digest(
    scope: &PublicationScope,
    snapshot: &PublicationSnapshot,
    intent_key: &str,
    generation: &DecimalU64,
) -> Result<String, PublicationError> {
    let request = (
        "admiralty-publication-v1",
        scope,
        snapshot,
        intent_key,
        generation,
    );
    crate::limits::bounded_json_size(&request, MAX_JSON_BYTES)
        .map_err(|_| PublicationError::LimitExceeded)?;
    crate::content_digest(&request).map_err(|_| PublicationError::LimitExceeded)
}

pub fn propose_publication(
    scope: &PublicationScope,
    snapshot: &PublicationSnapshot,
    intent_key: &str,
    request_digest: &str,
    proposed_generation: &DecimalU64,
) -> PublicationProposal {
    if crate::limits::bounded_json_size(
        &(
            scope,
            snapshot,
            intent_key,
            request_digest,
            proposed_generation,
        ),
        MAX_JSON_BYTES,
    )
    .is_err()
        || !valid_ref(&scope.target_ref)
        || !valid_text(intent_key)
        || !valid_digest(request_digest)
        || snapshot.required_reviewers.is_empty()
        || snapshot.required_checks.is_empty()
        || [
            snapshot.required_reviewers.len(),
            snapshot.required_checks.len(),
            snapshot.approvals.len(),
            snapshot.checks.len(),
        ]
        .into_iter()
        .any(|n| n > MAX_ITEMS)
    {
        return blocked(vec![PublicationBlock::InvalidEvidence]);
    }
    let text_valid = snapshot
        .required_reviewers
        .iter()
        .chain(snapshot.required_checks.iter())
        .all(|s| valid_text(s))
        && snapshot.approvals.iter().all(|a| {
            valid_text(&a.reviewer) && valid_oid(&a.head) && valid_text(&a.provenance_ref)
        })
        && snapshot.checks.iter().all(|c| {
            valid_text(&c.check_id) && valid_oid(&c.head) && valid_text(&c.provenance_ref)
        });
    if !text_valid
        || publication_request_digest(scope, snapshot, intent_key, proposed_generation).as_deref()
            != Ok(request_digest)
    {
        return blocked(vec![PublicationBlock::InvalidEvidence]);
    }
    let mut reasons = Vec::new();
    let (head, base, expected_head, expected_base) = match (
        &snapshot.current_head,
        &snapshot.current_base,
        &snapshot.expected_head,
        &snapshot.expected_base,
    ) {
        (Some(h), Some(b), Some(eh), Some(eb))
            if [h, b, eh, eb].into_iter().all(|s| valid_oid(s)) =>
        {
            (h, b, eh, eb)
        }
        _ => return blocked(vec![PublicationBlock::MissingHeadOrBase]),
    };
    if head != expected_head {
        reasons.push(PublicationBlock::ChangedHead);
    }
    if base != expected_base {
        reasons.push(PublicationBlock::ChangedBase);
    }
    let configured: BTreeSet<_> = snapshot.required_reviewers.iter().collect();
    if configured.len() != snapshot.required_reviewers.len() {
        reasons.push(PublicationBlock::InvalidEvidence);
    }
    if snapshot
        .approvals
        .iter()
        .any(|a| !configured.contains(&a.reviewer))
    {
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
        } else if matching.is_empty() {
            reasons.push(PublicationBlock::MissingApproval);
        } else if matching.iter().any(|a| a.head != *head) {
            reasons.push(PublicationBlock::WrongReviewer);
        }
    }
    let checks: BTreeSet<_> = snapshot.required_checks.iter().collect();
    if checks.len() != snapshot.required_checks.len() {
        reasons.push(PublicationBlock::InvalidEvidence);
    }
    for id in checks {
        let found: Vec<_> = snapshot
            .checks
            .iter()
            .filter(|c| &c.check_id == id && c.head == *head)
            .collect();
        if found.is_empty() || found.iter().any(|c| c.state == CheckState::Unknown) {
            reasons.push(PublicationBlock::MissingOrUnknownCheck);
        } else if found.iter().any(|c| c.state != CheckState::Passed) {
            reasons.push(PublicationBlock::CheckNotPassed);
        }
    }
    if snapshot.owner_only && !snapshot.caller_is_owner {
        reasons.push(PublicationBlock::OwnerOnly);
    }
    if snapshot.unresolved_intent {
        reasons.push(PublicationBlock::UnresolvedEffectSlot);
    }
    if scope.authority_generation.parse().is_err()
        || scope.authority_generation != *proposed_generation
    {
        reasons.push(PublicationBlock::StaleAuthorityGeneration);
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
            admission_snapshot: snapshot.clone(),
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
fn valid_text(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_STRING_BYTES && !s.chars().any(char::is_control)
}
fn valid_oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64)
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_digest(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn valid_ref(s: &str) -> bool {
    valid_text(s)
        && s.starts_with("refs/heads/")
        && !s.ends_with('/')
        && !s.ends_with('.')
        && !s.contains("..")
        && !s.contains("@{")
        && !s.contains("//")
        && !s
            .chars()
            .any(|c| c.is_whitespace() || "~^:?*[\\".contains(c))
        && s.split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}
fn valid_receipt(intent: &PublicationIntent, proof: &ReconciliationReceipt) -> bool {
    proof.intent_key == intent.intent_key
        && proof.request_digest == intent.request_digest
        && proof.scope == intent.scope
        && proof.expected_head == intent.expected_head
        && proof.expected_base == intent.expected_base
        && proof.authority_generation == intent.authority_generation
        && valid_text(&proof.provenance_ref)
        && valid_oid(&proof.observed_head)
        && if proof.effect_occurred {
            proof.observed_head != intent.expected_base
        } else {
            proof.observed_head == intent.expected_base
        }
}
fn valid_intent(intent: &PublicationIntent) -> bool {
    let admitted = propose_publication(
        &intent.scope,
        &intent.admission_snapshot,
        &intent.intent_key,
        &intent.request_digest,
        &intent.authority_generation,
    );
    let Some(mut expected) = admitted.intent else {
        return false;
    };
    expected.stage = intent.stage.clone();
    expected.effect_receipt = intent.effect_receipt.clone();
    expected == *intent
        && match intent.stage {
            IntentStage::ReconciledSuccess => intent
                .effect_receipt
                .as_ref()
                .is_some_and(|r| r.effect_occurred && valid_receipt(intent, r)),
            IntentStage::ReconciledNoEffect => intent
                .effect_receipt
                .as_ref()
                .is_some_and(|r| !r.effect_occurred && valid_receipt(intent, r)),
            _ => intent.effect_receipt.is_none(),
        }
}

/// Pure local consistency check; a trusted durable gateway must authenticate all evidence.
/// Ambiguous effects hold their slot until a scope-bound reconciliation observation.
pub fn reduce_publication_intent(
    existing: Option<&PublicationIntent>,
    incoming: &PublicationIntent,
    receipt: Option<&ReconciliationReceipt>,
) -> Result<PublicationIntent, PublicationError> {
    crate::limits::bounded_json_size(&(existing, incoming, receipt), MAX_JSON_BYTES)
        .map_err(|_| PublicationError::LimitExceeded)?;
    if let Some(old) = existing {
        if old.intent_key != incoming.intent_key {
            return Err(PublicationError::EffectSlotOccupied);
        }
        if old.request_digest != incoming.request_digest
            || old.scope != incoming.scope
            || old.expected_head != incoming.expected_head
            || old.expected_base != incoming.expected_base
            || old.authority_generation != incoming.authority_generation
            || old.admission_snapshot != incoming.admission_snapshot
        {
            return Err(PublicationError::KeyCollision);
        }
        if !valid_intent(old) || !valid_intent(incoming) {
            return Err(PublicationError::InvalidReconciliation);
        }
        if matches!(
            old.stage,
            IntentStage::ReconciledSuccess | IntentStage::ReconciledNoEffect
        ) {
            return if old == incoming && old.effect_receipt.as_ref() == receipt {
                Ok(old.clone())
            } else {
                Err(PublicationError::ReceiptCollision)
            };
        }
        if old == incoming && receipt.is_none() {
            return Ok(old.clone());
        }
        match (&old.stage, &incoming.stage) {
            (IntentStage::Proposed, IntentStage::Claimed)
            | (IntentStage::Claimed, IntentStage::Ambiguous)
                if receipt.is_none() =>
            {
                Ok(incoming.clone())
            }
            (IntentStage::Ambiguous, IntentStage::Ambiguous) if old == incoming => {
                let proof = receipt
                    .filter(|r| valid_receipt(old, r))
                    .ok_or(PublicationError::InvalidReconciliation)?;
                let mut result = old.clone();
                result.stage = if proof.effect_occurred {
                    IntentStage::ReconciledSuccess
                } else {
                    IntentStage::ReconciledNoEffect
                };
                result.effect_receipt = Some(proof.clone());
                Ok(result)
            }
            _ => Err(PublicationError::InvalidReconciliation),
        }
    } else if incoming.stage == IntentStage::Proposed && receipt.is_none() && valid_intent(incoming)
    {
        Ok(incoming.clone())
    } else {
        Err(PublicationError::InvalidReconciliation)
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
            current_head: Some("a".repeat(40)),
            current_base: Some("b".repeat(40)),
            expected_head: Some("a".repeat(40)),
            expected_base: Some("b".repeat(40)),
            required_reviewers: vec!["reviewer".into()],
            approvals: vec![ApprovalEvidence {
                reviewer: "reviewer".into(),
                head: "a".repeat(40),
                dismissed: false,
                provenance_ref: "forge:approval:1".into(),
            }],
            required_checks: vec!["ci".into()],
            checks: vec![CheckEvidence {
                check_id: "ci".into(),
                head: "a".repeat(40),
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
        propose_publication(
            &s,
            &p,
            "k",
            &publication_request_digest(&s, &p, "k", &g).expect("digest"),
            &g,
        )
        .intent
        .expect("valid evidence proposes an intent")
    }
    #[test]
    fn eligible_is_proposal_only_and_replay_is_deterministic() {
        let (s, p, g) = fixtures();
        let a = propose_publication(
            &s,
            &p,
            "k",
            &publication_request_digest(&s, &p, "k", &g).expect("digest"),
            &g,
        );
        let b = propose_publication(
            &s,
            &p,
            "k",
            &publication_request_digest(&s, &p, "k", &g).expect("digest"),
            &g,
        );
        assert!(a.eligible);
        assert!(!a.execution_granted);
        assert_eq!(a, b);
    }
    #[test]
    fn changed_heads_bases_unknown_checks_and_owner_only_block() {
        let (s, mut p, g) = fixtures();
        p.approvals[0].reviewer = "stranger".into();
        assert!(
            propose_publication(
                &s,
                &p,
                "k",
                &publication_request_digest(&s, &p, "k", &g).expect("digest"),
                &g
            )
            .reasons
            .contains(&PublicationBlock::WrongReviewer)
        );
        let (s, mut p, g) = fixtures();
        p.current_head = Some("c".repeat(40));
        assert!(
            !propose_publication(
                &s,
                &p,
                "k",
                &publication_request_digest(&s, &p, "k", &g).expect("digest"),
                &g
            )
            .eligible
        );
        let (s, mut p, g) = fixtures();
        p.current_base = Some("d".repeat(40));
        assert!(
            !propose_publication(
                &s,
                &p,
                "k",
                &publication_request_digest(&s, &p, "k", &g).expect("digest"),
                &g
            )
            .eligible
        );
        let (s, mut p, g) = fixtures();
        p.checks[0].state = CheckState::Pending;
        assert!(
            !propose_publication(
                &s,
                &p,
                "k",
                &publication_request_digest(&s, &p, "k", &g).expect("digest"),
                &g
            )
            .eligible
        );
        let (s, mut p, g) = fixtures();
        p.checks[0].state = CheckState::Unknown;
        assert!(
            propose_publication(
                &s,
                &p,
                "k",
                &publication_request_digest(&s, &p, "k", &g).expect("digest"),
                &g
            )
            .reasons
            .contains(&PublicationBlock::MissingOrUnknownCheck)
        );
        let (s, mut p, g) = fixtures();
        p.owner_only = true;
        assert!(
            propose_publication(
                &s,
                &p,
                "k",
                &publication_request_digest(&s, &p, "k", &g).expect("digest"),
                &g
            )
            .reasons
            .contains(&PublicationBlock::OwnerOnly)
        );
        let (s, mut p, g) = fixtures();
        p.approvals[0].dismissed = true;
        assert!(
            propose_publication(
                &s,
                &p,
                "k",
                &publication_request_digest(&s, &p, "k", &g).expect("digest"),
                &g
            )
            .reasons
            .contains(&PublicationBlock::DismissedApproval)
        );
    }
    #[test]
    fn stale_generation_and_overflow_are_explicit() {
        let (s, p, _) = fixtures();
        let stale = DecimalU64::new(8);
        assert!(
            propose_publication(
                &s,
                &p,
                "k",
                &publication_request_digest(&s, &p, "k", &stale).expect("digest"),
                &stale
            )
            .reasons
            .contains(&PublicationBlock::StaleAuthorityGeneration)
        );
        let newer = DecimalU64::new(10);
        assert!(
            propose_publication(
                &s,
                &p,
                "k",
                &publication_request_digest(&s, &p, "k", &newer).expect("digest"),
                &newer
            )
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
            request_digest: first.request_digest.clone(),
            scope: first.scope.clone(),
            expected_head: first.expected_head.clone(),
            expected_base: first.expected_base.clone(),
            authority_generation: first.authority_generation.clone(),
            observed_head: "c".repeat(40),
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
            request_digest: first.request_digest.clone(),
            scope: first.scope.clone(),
            expected_head: first.expected_head.clone(),
            expected_base: first.expected_base.clone(),
            authority_generation: first.authority_generation.clone(),
            observed_head: "b".repeat(40),
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
    #[test]
    fn contradictory_checks_empty_policy_and_changed_request_are_blocked() {
        let (scope, mut snapshot, generation) = fixtures();
        snapshot.checks.push(CheckEvidence {
            state: CheckState::Failed,
            ..snapshot.checks[0].clone()
        });
        let digest =
            publication_request_digest(&scope, &snapshot, "k", &generation).expect("digest");
        assert!(
            propose_publication(&scope, &snapshot, "k", &digest, &generation)
                .reasons
                .contains(&PublicationBlock::CheckNotPassed)
        );
        let (scope, mut snapshot, generation) = fixtures();
        let digest =
            publication_request_digest(&scope, &snapshot, "k", &generation).expect("digest");
        snapshot.owner_only = true;
        assert!(!propose_publication(&scope, &snapshot, "k", &digest, &generation).eligible);
        snapshot.required_checks.clear();
        snapshot.required_reviewers.clear();
        let digest =
            publication_request_digest(&scope, &snapshot, "k", &generation).expect("digest");
        assert!(!propose_publication(&scope, &snapshot, "k", &digest, &generation).eligible);
    }
    fn receipt(intent: &PublicationIntent, effect: bool) -> ReconciliationReceipt {
        ReconciliationReceipt {
            intent_key: intent.intent_key.clone(),
            request_digest: intent.request_digest.clone(),
            scope: intent.scope.clone(),
            expected_head: intent.expected_head.clone(),
            expected_base: intent.expected_base.clone(),
            authority_generation: intent.authority_generation.clone(),
            observed_head: if effect {
                "c".repeat(40)
            } else {
                intent.expected_base.clone()
            },
            effect_occurred: effect,
            provenance_ref: "forge:receipt".into(),
        }
    }
    #[test]
    fn reconciliation_binds_scope_generation_and_observed_target() {
        let mut intent = proposal();
        intent.stage = IntentStage::Ambiguous;
        let proof = receipt(&intent, false);
        assert!(reduce_publication_intent(Some(&intent), &intent, Some(&proof)).is_ok());
        let mut foreign = proof.clone();
        foreign.scope.root_id = RootId::new("foreign").expect("root");
        assert_eq!(
            reduce_publication_intent(Some(&intent), &intent, Some(&foreign)),
            Err(PublicationError::InvalidReconciliation)
        );
        foreign = proof.clone();
        foreign.authority_generation = DecimalU64::new(8);
        assert_eq!(
            reduce_publication_intent(Some(&intent), &intent, Some(&foreign)),
            Err(PublicationError::InvalidReconciliation)
        );
        foreign = proof;
        foreign.observed_head = "d".repeat(40);
        assert_eq!(
            reduce_publication_intent(Some(&intent), &intent, Some(&foreign)),
            Err(PublicationError::InvalidReconciliation)
        );
        let mut altered = intent.clone();
        altered.authority_generation = DecimalU64::new(10);
        assert_eq!(
            reduce_publication_intent(Some(&intent), &altered, None),
            Err(PublicationError::KeyCollision)
        );
    }
    #[test]
    fn receipt_injection_and_typed_oversize_are_refused() {
        let first = proposal();
        let mut forged = first.clone();
        forged.effect_receipt = Some(receipt(&first, true));
        assert_eq!(
            reduce_publication_intent(None, &forged, None),
            Err(PublicationError::InvalidReconciliation)
        );
        let mut claimed = first.clone();
        claimed.stage = IntentStage::Claimed;
        assert_eq!(
            reduce_publication_intent(Some(&first), &claimed, Some(&receipt(&first, true))),
            Err(PublicationError::InvalidReconciliation)
        );
        forged = first;
        forged.admission_snapshot.checks[0].provenance_ref = "x".repeat(MAX_JSON_BYTES);
        assert_eq!(
            reduce_publication_intent(None, &forged, None),
            Err(PublicationError::LimitExceeded)
        );
        for invalid in [
            "refs/heads/x.lock",
            "refs/heads/x..y",
            "refs/heads/.hidden",
            "refs/heads/a b",
            "main",
        ] {
            assert!(!valid_ref(invalid));
        }
    }
}
