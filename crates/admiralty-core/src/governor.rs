//! Pure bounded attention policy. Durable ingestion, authority checks, artifact
//! custody and atomic delivery/receipt persistence belong to the caller.
use crate::{MessageId, PrincipalId, ProjectId, SourceIdentity, TaskId, content_digest};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const MAX_TOPICS: usize = 2_000;
pub const MAX_UPDATE_BYTES: usize = 16_384;
pub const MAX_PENDING_REFS: usize = 4_096;
pub const MAX_SOURCE_IDS: usize = 16_384;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    TaskWorker,
    Captain,
    SuperCaptain,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Recipient {
    pub role: Role,
    pub principal_id: PrincipalId,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub project_id: ProjectId,
    pub task_id: TaskId,
    pub revision: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RoleBudget {
    pub window_ticks: u64,
    pub ordinary_wakes: u32,
    pub critical_reserve: u32,
    pub max_delivery_refs: u16,
    pub max_delivery_bytes: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GovernorConfig {
    pub task_worker: RoleBudget,
    pub captain: RoleBudget,
    pub super_captain: RoleBudget,
    pub max_topics: usize,
    pub max_pending_refs: usize,
    pub max_source_ids: usize,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        let budget = |ordinary_wakes, critical_reserve, max_delivery_refs| RoleBudget {
            window_ticks: 60,
            ordinary_wakes,
            critical_reserve,
            max_delivery_refs,
            max_delivery_bytes: 8_192,
        };
        Self {
            task_worker: budget(12, 2, 16),
            captain: budget(4, 2, 16),
            super_captain: budget(2, 2, 8),
            max_topics: MAX_TOPICS,
            max_pending_refs: MAX_PENDING_REFS,
            max_source_ids: MAX_SOURCE_IDS,
        }
    }
}

impl GovernorConfig {
    fn budget(&self, role: Role) -> RoleBudget {
        match role {
            Role::TaskWorker => self.task_worker,
            Role::Captain => self.captain,
            Role::SuperCaptain => self.super_captain,
        }
    }
    fn validate(&self) -> Result<(), GovernorError> {
        if !(1..=MAX_TOPICS).contains(&self.max_topics)
            || !(1..=MAX_PENDING_REFS).contains(&self.max_pending_refs)
            || !(1..=MAX_SOURCE_IDS).contains(&self.max_source_ids)
        {
            return Err(GovernorError::Invalid(
                "invalid bounded governor capacity".into(),
            ));
        }
        for budget in [self.task_worker, self.captain, self.super_captain] {
            if budget.window_ticks == 0
                || budget.ordinary_wakes > 10_000
                || budget.critical_reserve > 10_000
                || !(1..=128).contains(&budget.max_delivery_refs)
                || !(1_024..=16_384).contains(&budget.max_delivery_bytes)
            {
                return Err(GovernorError::Invalid(
                    "invalid role attention budget".into(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QuestionFacts {
    pub question: String,
    pub options: Vec<String>,
    pub protected_constraints: Vec<String>,
    pub contract_digest: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlockState {
    Raised,
    Cleared,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuestionState {
    Open,
    Answered,
    Cancelled,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
    Submitted,
    Updated,
    Withdrawn,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionState {
    Recorded,
    Superseded,
    Revoked,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Green,
    Warning,
    Critical,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpdateKind {
    Progress {
        phase: String,
        completed: u32,
        total: u32,
    },
    Blocked {
        blocker_id: String,
        state: BlockState,
    },
    Question {
        question_id: MessageId,
        state: QuestionState,
        facts: QuestionFacts,
    },
    Candidate {
        candidate_ref: String,
        state: CandidateState,
    },
    Decision {
        decision_id: MessageId,
        state: DecisionState,
    },
    Health {
        component_id: String,
        state: HealthState,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttentionUpdate {
    pub source: SourceIdentity,
    pub scope: Scope,
    pub recipient: Recipient,
    pub authority_ref: String,
    /// Caller-owned durable immutable evidence; never opened by this module.
    pub evidence_ref: String,
    pub update: UpdateKind,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum WakeReason {
    BlockedRaised,
    BlockedCleared,
    QuestionOpened,
    QuestionAnswered,
    QuestionCancelled,
    CandidateSubmitted,
    CandidateUpdated,
    CandidateWithdrawn,
    DecisionRecorded,
    DecisionSuperseded,
    DecisionRevoked,
    HealthWarning,
    HealthCritical,
    HealthUnknown,
}

impl WakeReason {
    fn reserve_eligible(self) -> bool {
        !matches!(self, Self::HealthWarning | Self::HealthUnknown)
    }
    fn is_health(self) -> bool {
        matches!(
            self,
            Self::HealthWarning | Self::HealthCritical | Self::HealthUnknown
        )
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateTag {
    Progress,
    Blocked,
    Question,
    Candidate,
    Decision,
    Health,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeliveryRef {
    /// Decimal ingestion order; preserves precision in JS and transition order.
    pub order: String,
    pub source: SourceIdentity,
    pub scope: Scope,
    pub authority_ref: String,
    pub evidence_ref: String,
    pub update_digest: String,
    pub kind: UpdateTag,
    pub duplicate_question_of: Option<SourceIdentity>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BudgetClass {
    Ordinary,
    CriticalReserve,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DeliveryBatch {
    pub delivery_id: String,
    pub recipient: Recipient,
    pub logical_tick: String,
    pub budget_class: BudgetClass,
    pub reasons: BTreeSet<WakeReason>,
    pub refs: Vec<DeliveryRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum DeliveryDecision {
    Quiet,
    Deferred {
        pending_refs: usize,
        window_index: String,
    },
    Wake {
        delivery: DeliveryBatch,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Observation {
    Accepted,
    ProgressCoalesced,
    DuplicateSource,
    EquivalentQuestion,
    HealthUnchanged,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum GovernorError {
    #[error("logical clock is unknown; preserve update and retry without resetting budgets")]
    UnknownClock,
    #[error("logical clock moved backwards")]
    BackwardClock,
    #[error("source identity reused with different content")]
    SourceCollision,
    #[error("authority changed inside an existing scoped revision")]
    AuthorityChanged,
    #[error("governor bound reached: {0}; retain unacknowledged input upstream")]
    Capacity(&'static str),
    #[error("governor counter overflow")]
    Overflow,
    #[error("essential delivery reference does not fit role budget")]
    DeliveryTooLarge,
    #[error("invalid attention update: {0}")]
    Invalid(String),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Topic {
    recipient: Recipient,
    scope: Scope,
}
#[derive(Clone, Debug)]
struct TopicState {
    authority_ref: String,
    progress: Option<DeliveryRef>,
    health: BTreeMap<String, HealthState>,
}
#[derive(Clone, Debug)]
struct Pending {
    reference: DeliveryRef,
    reason: Option<WakeReason>,
    health_component: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct ExactQuestion {
    scope: Scope,
    recipient: Recipient,
    authority_ref: String,
    facts: QuestionFacts,
}
#[derive(Clone, Debug)]
struct QuestionGroup {
    facts: ExactQuestion,
    /// Keep independent lifecycles even when their protected facts are equal.
    members: BTreeMap<MessageId, (u64, SourceIdentity)>,
}
impl QuestionGroup {
    fn canonical_source(&self) -> Option<&SourceIdentity> {
        self.members
            .values()
            .min_by_key(|(order, _)| *order)
            .map(|(_, source)| source)
    }
}
#[derive(Clone, Copy, Debug, Default)]
struct BudgetSpent {
    window: u64,
    ordinary: u32,
    reserve: u32,
}

#[derive(Clone, Debug)]
pub struct AttentionGovernor {
    config: GovernorConfig,
    last_clock: Option<u64>,
    next_order: u64,
    next_delivery: u64,
    topics: BTreeMap<Topic, TopicState>,
    sources: BTreeMap<String, String>,
    questions: BTreeMap<String, QuestionGroup>,
    pending: BTreeMap<Recipient, VecDeque<Pending>>,
    pending_count: usize,
    spent: BTreeMap<Recipient, BudgetSpent>,
}

struct SizeLimit(usize);
impl std::io::Write for SizeLimit {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("bounded JSON size exceeded"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn valid_text(value: &str, limit: usize) -> bool {
    !value.trim().is_empty() && value.len() <= limit
}
fn valid_ref(value: &str) -> bool {
    valid_text(value, 512)
        && !value.chars().any(char::is_whitespace)
        && !value.chars().any(char::is_control)
}
fn valid_id(value: &str) -> bool {
    PrincipalId::new(value).is_ok()
}
fn digest<T: Serialize>(value: &T) -> Result<String, GovernorError> {
    content_digest(value).map_err(|error| GovernorError::Invalid(error.to_string()))
}

impl AttentionUpdate {
    pub fn validate(&self) -> Result<(), GovernorError> {
        let fail = || {
            GovernorError::Invalid(
                "versioned scope, bounded references and complete protected facts required".into(),
            )
        };
        self.source.validate().map_err(|_| fail())?;
        if !valid_id(&self.source.producer_id)
            || !valid_id(&self.source.incarnation)
            || self.scope.revision == 0
            || !valid_ref(&self.authority_ref)
            || !valid_ref(&self.evidence_ref)
        {
            return Err(fail());
        }
        let valid = match &self.update {
            UpdateKind::Progress {
                phase,
                completed,
                total,
            } => valid_id(phase) && completed <= total,
            UpdateKind::Blocked { blocker_id, .. } => valid_id(blocker_id),
            UpdateKind::Candidate { candidate_ref, .. } => valid_ref(candidate_ref),
            UpdateKind::Question { facts, .. } => {
                valid_text(&facts.question, 4096)
                    && facts.options.len() <= 16
                    && facts.options.iter().all(|value| valid_text(value, 512))
                    && facts.protected_constraints.len() <= 32
                    && facts
                        .protected_constraints
                        .iter()
                        .all(|value| valid_text(value, 1024))
                    && facts
                        .contract_digest
                        .strip_prefix("sha256:")
                        .is_some_and(|hex| {
                            hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
            }
            UpdateKind::Decision { .. } => true,
            UpdateKind::Health { component_id, .. } => valid_id(component_id),
        };
        if !valid {
            return Err(fail());
        }
        serde_json::to_writer(SizeLimit(MAX_UPDATE_BYTES), self)
            .map_err(|_| GovernorError::Invalid("update exceeds16KiB serialized bound".into()))?;
        Ok(())
    }
}

impl AttentionGovernor {
    pub fn new(config: GovernorConfig) -> Result<Self, GovernorError> {
        config.validate()?;
        Ok(Self {
            config,
            last_clock: None,
            next_order: 0,
            next_delivery: 0,
            topics: BTreeMap::new(),
            sources: BTreeMap::new(),
            questions: BTreeMap::new(),
            pending: BTreeMap::new(),
            pending_count: 0,
            spent: BTreeMap::new(),
        })
    }
    fn clock(&self, supplied: Option<u64>) -> Result<u64, GovernorError> {
        let now = supplied.ok_or(GovernorError::UnknownClock)?;
        if self.last_clock.is_some_and(|old| now < old) {
            return Err(GovernorError::BackwardClock);
        }
        Ok(now)
    }
    pub fn pending_refs(&self) -> usize {
        self.pending_count
    }
    pub fn topic_count(&self) -> usize {
        self.topics.len()
    }

    pub fn observe(
        &mut self,
        update: AttentionUpdate,
        logical_tick: Option<u64>,
    ) -> Result<Observation, GovernorError> {
        let now = self.clock(logical_tick)?;
        update.validate()?;
        let source_key = serde_json::to_string(&update.source)
            .map_err(|error| GovernorError::Invalid(error.to_string()))?;
        let update_digest = digest(&update)?;
        if let Some(previous) = self.sources.get(&source_key) {
            return if previous == &update_digest {
                self.last_clock = Some(now);
                Ok(Observation::DuplicateSource)
            } else {
                Err(GovernorError::SourceCollision)
            };
        }
        if self.sources.len() >= self.config.max_source_ids {
            return Err(GovernorError::Capacity("source identities"));
        }
        let topic = Topic {
            recipient: update.recipient.clone(),
            scope: update.scope.clone(),
        };
        if let Some(previous) = self.topics.get(&topic) {
            if previous.authority_ref != update.authority_ref {
                return Err(GovernorError::AuthorityChanged);
            }
        } else if self.topics.len() >= self.config.max_topics {
            return Err(GovernorError::Capacity("topics"));
        }
        let next_order = self
            .next_order
            .checked_add(1)
            .ok_or(GovernorError::Overflow)?;
        let mut question = None;
        let mut duplicate = None;
        let mut result = Observation::Accepted;
        let (tag, mut reason, health_component) = match &update.update {
            UpdateKind::Progress { .. } => {
                if self
                    .topics
                    .get(&topic)
                    .is_some_and(|state| state.progress.is_some())
                {
                    result = Observation::ProgressCoalesced;
                }
                (UpdateTag::Progress, None, None)
            }
            UpdateKind::Blocked { state, .. } => (
                UpdateTag::Blocked,
                Some(match state {
                    BlockState::Raised => WakeReason::BlockedRaised,
                    BlockState::Cleared => WakeReason::BlockedCleared,
                }),
                None,
            ),
            UpdateKind::Question {
                question_id,
                state,
                facts,
            } => {
                let exact = ExactQuestion {
                    scope: update.scope.clone(),
                    recipient: update.recipient.clone(),
                    authority_ref: update.authority_ref.clone(),
                    facts: facts.clone(),
                };
                let hash = digest(&(
                    &exact.scope,
                    &exact.recipient,
                    &exact.authority_ref,
                    &exact.facts,
                ))?;
                if let Some(existing) = self.questions.get(&hash)
                    && existing.facts != exact
                {
                    return Err(GovernorError::SourceCollision);
                }
                if *state == QuestionState::Open {
                    if let Some(existing) = self.questions.get(&hash) {
                        duplicate = existing.canonical_source().cloned();
                        result = Observation::EquivalentQuestion;
                    } else if self.questions.len() >= self.config.max_topics {
                        return Err(GovernorError::Capacity("active question facts"));
                    }
                }
                question = Some((hash, exact, question_id.clone(), *state));
                (
                    UpdateTag::Question,
                    Some(match state {
                        QuestionState::Open => WakeReason::QuestionOpened,
                        QuestionState::Answered => WakeReason::QuestionAnswered,
                        QuestionState::Cancelled => WakeReason::QuestionCancelled,
                    }),
                    None,
                )
            }
            UpdateKind::Candidate { state, .. } => (
                UpdateTag::Candidate,
                Some(match state {
                    CandidateState::Submitted => WakeReason::CandidateSubmitted,
                    CandidateState::Updated => WakeReason::CandidateUpdated,
                    CandidateState::Withdrawn => WakeReason::CandidateWithdrawn,
                }),
                None,
            ),
            UpdateKind::Decision { state, .. } => (
                UpdateTag::Decision,
                Some(match state {
                    DecisionState::Recorded => WakeReason::DecisionRecorded,
                    DecisionState::Superseded => WakeReason::DecisionSuperseded,
                    DecisionState::Revoked => WakeReason::DecisionRevoked,
                }),
                None,
            ),
            UpdateKind::Health {
                component_id,
                state,
            } => {
                let previous = self
                    .topics
                    .get(&topic)
                    .and_then(|topic| topic.health.get(component_id));
                let reason = if previous == Some(state) {
                    result = Observation::HealthUnchanged;
                    None
                } else {
                    match state {
                        HealthState::Green => None,
                        HealthState::Warning => Some(WakeReason::HealthWarning),
                        HealthState::Critical => Some(WakeReason::HealthCritical),
                        HealthState::Unknown => Some(WakeReason::HealthUnknown),
                    }
                };
                (UpdateTag::Health, reason, Some(component_id.clone()))
            }
        };
        if duplicate.is_some() {
            reason = None;
        }
        // Health transitions are retained even when green suppresses a wake;
        // a later delivery must not expose an old red state without its clear.
        let retained = !matches!(tag, UpdateTag::Progress)
            && (tag != UpdateTag::Health || result != Observation::HealthUnchanged);
        if retained && self.pending_count >= self.config.max_pending_refs {
            return Err(GovernorError::Capacity("pending critical references"));
        }
        let reference = DeliveryRef {
            order: next_order.to_string(),
            source: update.source.clone(),
            scope: update.scope.clone(),
            authority_ref: update.authority_ref.clone(),
            evidence_ref: update.evidence_ref.clone(),
            update_digest,
            kind: tag,
            duplicate_question_of: duplicate,
        };
        // All fallible checks precede mutation. Rejected inputs remain retryable.
        self.next_order = next_order;
        self.last_clock = Some(now);
        self.sources
            .insert(source_key, reference.update_digest.clone());
        let topic_state = self.topics.entry(topic).or_insert_with(|| TopicState {
            authority_ref: update.authority_ref,
            progress: None,
            health: BTreeMap::new(),
        });
        if tag == UpdateTag::Progress {
            topic_state.progress = Some(reference.clone());
        }
        if let UpdateKind::Health {
            component_id,
            state,
        } = &update.update
        {
            topic_state.health.insert(component_id.clone(), *state);
            if *state == HealthState::Green
                && let Some(queue) = self.pending.get_mut(&update.recipient)
            {
                for pending in queue.iter_mut().filter(|pending| {
                    pending.reference.scope == update.scope
                        && pending.health_component.as_deref() == Some(component_id)
                }) {
                    if pending.reason.is_some_and(WakeReason::is_health) {
                        pending.reason = None;
                    }
                }
            }
        }
        if let Some((hash, exact, question_id, state)) = question {
            if state == QuestionState::Open {
                self.questions
                    .entry(hash)
                    .or_insert_with(|| QuestionGroup {
                        facts: exact,
                        members: BTreeMap::new(),
                    })
                    .members
                    .entry(question_id)
                    .or_insert((next_order, update.source));
            } else if let Some(group) = self.questions.get_mut(&hash) {
                // Closing one equivalent question must not retire another.
                // An unmatched close is still retained as an authoritative
                // reference, but cannot suppress an independent open question.
                group.members.remove(&question_id);
                if group.members.is_empty() {
                    self.questions.remove(&hash);
                }
            }
        }
        if retained {
            self.pending
                .entry(update.recipient)
                .or_default()
                .push_back(Pending {
                    reference,
                    reason,
                    health_component,
                });
            self.pending_count += 1;
        }
        Ok(result)
    }

    pub fn take_delivery(
        &mut self,
        recipient: &Recipient,
        logical_tick: Option<u64>,
    ) -> Result<DeliveryDecision, GovernorError> {
        let now = self.clock(logical_tick)?;
        let Some(queue) = self.pending.get(recipient) else {
            self.last_clock = Some(now);
            return Ok(DeliveryDecision::Quiet);
        };
        if !queue.iter().any(|pending| pending.reason.is_some()) {
            self.last_clock = Some(now);
            return Ok(DeliveryDecision::Quiet);
        }
        let budget = self.config.budget(recipient.role);
        let window = now / budget.window_ticks;
        let mut spent = self
            .spent
            .get(recipient)
            .copied()
            .filter(|spent| spent.window == window)
            .unwrap_or(BudgetSpent {
                window,
                ..Default::default()
            });
        let class = if spent.ordinary < budget.ordinary_wakes {
            BudgetClass::Ordinary
        } else if spent.reserve < budget.critical_reserve
            && queue
                .iter()
                .any(|pending| pending.reason.is_some_and(WakeReason::reserve_eligible))
        {
            BudgetClass::CriticalReserve
        } else {
            self.last_clock = Some(now);
            return Ok(DeliveryDecision::Deferred {
                pending_refs: queue.len(),
                window_index: window.to_string(),
            });
        };
        let first = queue
            .iter()
            .position(|pending| {
                pending.reason.is_some_and(|reason| {
                    class == BudgetClass::Ordinary || reason.reserve_eligible()
                })
            })
            .expect("actionable queue checked above");
        let delivery_number = self
            .next_delivery
            .checked_add(1)
            .ok_or(GovernorError::Overflow)?;
        let mut delivery = DeliveryBatch {
            delivery_id: format!("attention-{delivery_number}"),
            recipient: recipient.clone(),
            logical_tick: now.to_string(),
            budget_class: class,
            reasons: BTreeSet::new(),
            refs: vec![],
        };
        let mut selected = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut health_lineage: BTreeMap<(Scope, String), Vec<usize>> = BTreeMap::new();
        for (index, pending) in queue.iter().enumerate() {
            if let Some(component) = &pending.health_component {
                health_lineage
                    .entry((pending.reference.scope.clone(), component.clone()))
                    .or_default()
                    .push(index);
            }
        }
        let order = std::iter::once(first).chain((0..queue.len()).filter(|index| *index != first));
        for index in order {
            if selected.len() >= budget.max_delivery_refs as usize {
                break;
            }
            if !visited.insert(index) {
                continue;
            }
            let pending = &queue[index];
            // Health can clear before attention. Deliver its retained lineage
            // atomically so a small batch never exposes red without its clear.
            let group = pending
                .health_component
                .as_ref()
                .and_then(|component| {
                    health_lineage.get(&(pending.reference.scope.clone(), component.clone()))
                })
                .cloned()
                .unwrap_or_else(|| vec![index]);
            visited.extend(group.iter().copied());
            if selected.len() + group.len() > budget.max_delivery_refs as usize {
                if selected.is_empty() {
                    return Err(GovernorError::DeliveryTooLarge);
                }
                continue;
            }
            let mut candidate = delivery.clone();
            for group_index in &group {
                let pending = &queue[*group_index];
                candidate.refs.push(pending.reference.clone());
                if let Some(reason) = pending.reason {
                    candidate.reasons.insert(reason);
                }
            }
            if serde_json::to_writer(SizeLimit(budget.max_delivery_bytes as usize), &candidate)
                .is_err()
            {
                if selected.is_empty() {
                    return Err(GovernorError::DeliveryTooLarge);
                }
                continue;
            }
            delivery = candidate;
            selected.extend(group);
        }
        // Include only each actionable topic's latest progress, never a fleet
        // transcript or an unbounded manager summary.
        let scopes: BTreeSet<_> = delivery
            .refs
            .iter()
            .map(|reference| reference.scope.clone())
            .collect();
        for scope in scopes {
            if delivery.refs.len() >= budget.max_delivery_refs as usize {
                break;
            }
            let topic = Topic {
                recipient: recipient.clone(),
                scope,
            };
            if let Some(reference) = self
                .topics
                .get(&topic)
                .and_then(|state| state.progress.as_ref())
            {
                let mut candidate = delivery.clone();
                candidate.refs.push(reference.clone());
                if serde_json::to_writer(SizeLimit(budget.max_delivery_bytes as usize), &candidate)
                    .is_ok()
                {
                    delivery = candidate;
                }
            }
        }
        delivery.refs.sort_by_key(|reference| {
            reference
                .order
                .parse::<u64>()
                .expect("internally generated order")
        });
        match class {
            BudgetClass::Ordinary => {
                spent.ordinary = spent
                    .ordinary
                    .checked_add(1)
                    .ok_or(GovernorError::Overflow)?
            }
            BudgetClass::CriticalReserve => {
                spent.reserve = spent
                    .reserve
                    .checked_add(1)
                    .ok_or(GovernorError::Overflow)?
            }
        }
        let queue = self
            .pending
            .get_mut(recipient)
            .expect("queue observed before mutation");
        let mut index = 0;
        queue.retain(|_| {
            let keep = !selected.contains(&index);
            index += 1;
            keep
        });
        self.pending_count = self
            .pending_count
            .checked_sub(selected.len())
            .expect("selected references are a pending subset");
        self.next_delivery = delivery_number;
        self.last_clock = Some(now);
        self.spent.insert(recipient.clone(), spent);
        Ok(DeliveryDecision::Wake { delivery })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipient(role: Role) -> Recipient {
        Recipient {
            role,
            principal_id: PrincipalId::new("fixture-recipient").expect("fixture recipient"),
        }
    }
    fn update(sequence: u64, kind: UpdateKind) -> AttentionUpdate {
        AttentionUpdate {
            source: SourceIdentity {
                producer_id: "fixture-worker".into(),
                incarnation: "boot-one".into(),
                sequence: sequence.to_string(),
            },
            scope: Scope {
                project_id: ProjectId::new("fixture-project").expect("fixture project"),
                task_id: TaskId::new("TASK-ATTENTION").expect("fixture task"),
                revision: 1,
            },
            recipient: recipient(Role::Captain),
            authority_ref: "grant:candidate-only".into(),
            evidence_ref: format!("event:fixture-worker/{sequence}"),
            update: kind,
        }
    }
    fn progress(sequence: u64, completed: u32) -> AttentionUpdate {
        update(
            sequence,
            UpdateKind::Progress {
                phase: "implementing".into(),
                completed,
                total: 1000,
            },
        )
    }
    fn facts() -> QuestionFacts {
        QuestionFacts {
            question: "Which interface revision preserves the required timeout?".into(),
            options: vec!["v1".into(), "v2".into()],
            protected_constraints: vec![
                "Do not publish".into(),
                "Maximum delay is5ms, not5s".into(),
            ],
            contract_digest: format!("sha256:{}", "a".repeat(64)),
        }
    }
    fn question(sequence: u64, state: QuestionState) -> AttentionUpdate {
        update(
            sequence,
            UpdateKind::Question {
                question_id: MessageId::new(format!("question-{sequence}"))
                    .expect("fixture question"),
                state,
                facts: facts(),
            },
        )
    }
    fn candidate(sequence: u64) -> AttentionUpdate {
        update(
            sequence,
            UpdateKind::Candidate {
                candidate_ref: format!("candidate:{sequence}"),
                state: CandidateState::Submitted,
            },
        )
    }
    fn wake(decision: DeliveryDecision) -> DeliveryBatch {
        match decision {
            DeliveryDecision::Wake { delivery } => delivery,
            other => panic!("expected actionable wake, got {other:?}"),
        }
    }
    fn governor(config: GovernorConfig) -> AttentionGovernor {
        AttentionGovernor::new(config).expect("bounded fixture config")
    }

    #[test]
    fn progress_storm_coalesces_to_one_latest_ref_and_never_wakes_a_captain() {
        let mut policy = governor(GovernorConfig::default());
        for sequence in 1..=500 {
            policy
                .observe(progress(sequence, sequence as u32), Some(sequence))
                .expect("bounded progress");
            assert_eq!(
                policy
                    .take_delivery(&recipient(Role::Captain), Some(sequence))
                    .expect("quiet progress"),
                DeliveryDecision::Quiet
            );
        }
        assert_eq!(policy.topic_count(), 1);
        assert_eq!(policy.pending_refs(), 0);
        policy
            .observe(candidate(501), Some(501))
            .expect("actionable candidate");
        let delivery = wake(
            policy
                .take_delivery(&recipient(Role::Captain), Some(501))
                .expect("candidate attention"),
        );
        assert_eq!(delivery.refs.len(), 2);
        assert!(
            delivery
                .refs
                .iter()
                .any(|reference| reference.kind == UpdateTag::Progress
                    && reference.source.sequence == "500")
        );
        assert_eq!(
            delivery.reasons,
            BTreeSet::from([WakeReason::CandidateSubmitted])
        );
        assert_eq!(policy.pending_refs(), 0);
    }

    #[test]
    fn exact_question_duplicates_share_attention_but_every_source_transition_survives() {
        let mut policy = governor(GovernorConfig::default());
        assert_eq!(
            policy
                .observe(question(1, QuestionState::Open), Some(1))
                .expect("first question"),
            Observation::Accepted
        );
        assert_eq!(
            policy
                .observe(question(2, QuestionState::Open), Some(2))
                .expect("exact question facts"),
            Observation::EquivalentQuestion
        );
        let batch = wake(
            policy
                .take_delivery(&recipient(Role::Captain), Some(2))
                .expect("one wake"),
        );
        assert_eq!(batch.refs.len(), 2);
        assert_eq!(
            batch.refs[1]
                .duplicate_question_of
                .as_ref()
                .map(|source| source.sequence.as_str()),
            Some("1")
        );
        let rendered = serde_json::to_string(&batch).expect("compact reference JSON");
        assert!(!rendered.contains(&facts().question));
        assert!(!rendered.contains("Maximum delay"));
        assert!(rendered.contains("grant:candidate-only"));
        assert_eq!(
            policy
                .take_delivery(&recipient(Role::Captain), Some(2))
                .expect("no repeated attention"),
            DeliveryDecision::Quiet
        );
        let mut negation = question(3, QuestionState::Open);
        if let UpdateKind::Question { facts, .. } = &mut negation.update {
            facts.protected_constraints[0] = "Publish".into();
        }
        assert_eq!(
            policy.observe(negation, Some(3)).expect("changed negation"),
            Observation::Accepted
        );
        let mut units = question(4, QuestionState::Open);
        if let UpdateKind::Question { facts, .. } = &mut units.update {
            facts.protected_constraints[1] = "Maximum delay is5s, not5ms".into();
        }
        assert_eq!(
            policy.observe(units, Some(4)).expect("changed unit"),
            Observation::Accepted
        );
        let mut revision = question(5, QuestionState::Open);
        revision.scope.revision = 2;
        revision.authority_ref = "grant:new-revision".into();
        assert_eq!(
            policy
                .observe(revision, Some(5))
                .expect("new protected revision"),
            Observation::Accepted
        );
        let mut authority = question(6, QuestionState::Open);
        authority.authority_ref = "grant:wider".into();
        assert_eq!(
            policy.observe(authority, Some(6)),
            Err(GovernorError::AuthorityChanged)
        );
    }

    #[test]
    fn blocked_question_candidate_and_decision_transitions_are_not_coalesced() {
        let mut config = GovernorConfig::default();
        config.captain.ordinary_wakes = 20;
        config.captain.max_delivery_refs = 1;
        let mut policy = governor(config);
        let transitions = [
            UpdateKind::Blocked {
                blocker_id: "dependency".into(),
                state: BlockState::Raised,
            },
            UpdateKind::Blocked {
                blocker_id: "dependency".into(),
                state: BlockState::Cleared,
            },
            question(3, QuestionState::Open).update,
            question(4, QuestionState::Answered).update,
            UpdateKind::Candidate {
                candidate_ref: "candidate:one".into(),
                state: CandidateState::Submitted,
            },
            UpdateKind::Candidate {
                candidate_ref: "candidate:two".into(),
                state: CandidateState::Updated,
            },
            UpdateKind::Candidate {
                candidate_ref: "candidate:two".into(),
                state: CandidateState::Withdrawn,
            },
            UpdateKind::Decision {
                decision_id: MessageId::new("decision-one").expect("fixture decision"),
                state: DecisionState::Recorded,
            },
            UpdateKind::Decision {
                decision_id: MessageId::new("decision-one").expect("fixture decision"),
                state: DecisionState::Superseded,
            },
            UpdateKind::Decision {
                decision_id: MessageId::new("decision-two").expect("fixture decision"),
                state: DecisionState::Revoked,
            },
        ];
        for (index, transition) in transitions.into_iter().enumerate() {
            policy
                .observe(update(index as u64 + 1, transition), Some(10))
                .expect("critical transition");
        }
        assert_eq!(policy.pending_refs(), 10);
        for expected in 1..=10 {
            let batch = wake(
                policy
                    .take_delivery(&recipient(Role::Captain), Some(10))
                    .expect("bounded critical delivery"),
            );
            assert_eq!(batch.refs[0].source.sequence, expected.to_string());
        }
        assert_eq!(policy.pending_refs(), 0);
    }

    #[test]
    fn critical_reserve_is_separate_and_exhaustion_defers_without_dropping_facts() {
        let mut config = GovernorConfig::default();
        config.captain.ordinary_wakes = 1;
        config.captain.critical_reserve = 1;
        config.captain.max_delivery_refs = 1;
        let mut policy = governor(config);
        policy
            .observe(question(1, QuestionState::Open), Some(1))
            .expect("ordinary question");
        assert_eq!(
            wake(
                policy
                    .take_delivery(&recipient(Role::Captain), Some(1))
                    .expect("ordinary wake")
            )
            .budget_class,
            BudgetClass::Ordinary
        );
        policy
            .observe(candidate(2), Some(2))
            .expect("critical candidate");
        assert_eq!(
            wake(
                policy
                    .take_delivery(&recipient(Role::Captain), Some(2))
                    .expect("reserve wake")
            )
            .budget_class,
            BudgetClass::CriticalReserve
        );
        policy
            .observe(
                update(
                    3,
                    UpdateKind::Decision {
                        decision_id: MessageId::new("decision").expect("fixture decision"),
                        state: DecisionState::Recorded,
                    },
                ),
                Some(3),
            )
            .expect("critical decision retained");
        assert!(matches!(
            policy
                .take_delivery(&recipient(Role::Captain), Some(3))
                .expect("bounded budget"),
            DeliveryDecision::Deferred {
                pending_refs: 1,
                ..
            }
        ));
        assert_eq!(policy.pending_refs(), 1);
        let reset = wake(
            policy
                .take_delivery(&recipient(Role::Captain), Some(60))
                .expect("recorded next window"),
        );
        assert_eq!(reset.budget_class, BudgetClass::Ordinary);
        assert_eq!(reset.refs[0].source.sequence, "3");
    }

    #[test]
    fn equivalent_questions_keep_independent_lifecycles_and_active_canonical_sources() {
        let mut policy = governor(GovernorConfig::default());
        policy
            .observe(question(1, QuestionState::Open), Some(1))
            .expect("first open lifecycle");
        policy
            .observe(question(2, QuestionState::Open), Some(2))
            .expect("equivalent independent lifecycle");
        let _ = wake(
            policy
                .take_delivery(&recipient(Role::Captain), Some(2))
                .expect("shared attention"),
        );

        // An unmatched close is retained, but must not close either open ID.
        policy
            .observe(question(3, QuestionState::Answered), Some(3))
            .expect("unmatched close retained for authoritative reconciliation");
        assert_eq!(
            policy
                .observe(question(4, QuestionState::Open), Some(4))
                .expect("first lifecycle remains open"),
            Observation::EquivalentQuestion
        );
        let batch = wake(
            policy
                .take_delivery(&recipient(Role::Captain), Some(4))
                .expect("close transition wakes with equivalent source reference"),
        );
        assert_eq!(
            batch.refs[1]
                .duplicate_question_of
                .as_ref()
                .map(|source| source.sequence.as_str()),
            Some("1")
        );

        // Closing the canonical ID selects an actually open equivalent source.
        let mut close = question(5, QuestionState::Cancelled);
        if let UpdateKind::Question { question_id, .. } = &mut close.update {
            *question_id = MessageId::new("question-1").expect("fixture question");
        }
        policy.observe(close, Some(5)).expect("canonical closed");
        assert_eq!(
            policy
                .observe(question(6, QuestionState::Open), Some(6))
                .expect("remaining questions still equivalent"),
            Observation::EquivalentQuestion
        );
        let batch = wake(
            policy
                .take_delivery(&recipient(Role::Captain), Some(6))
                .expect("canonical close retained"),
        );
        assert_eq!(
            batch.refs[1]
                .duplicate_question_of
                .as_ref()
                .map(|source| source.sequence.as_str()),
            Some("2")
        );
    }

    #[test]
    fn role_and_recipient_budgets_are_independent_and_warning_cannot_spend_reserve() {
        let mut config = GovernorConfig::default();
        config.captain.ordinary_wakes = 1;
        config.captain.critical_reserve = 1;
        config.captain.max_delivery_refs = 1;
        config.task_worker.ordinary_wakes = 1;
        config.task_worker.critical_reserve = 0;
        config.super_captain.ordinary_wakes = 0;
        config.super_captain.critical_reserve = 0;
        let mut policy = governor(config);
        policy
            .observe(candidate(1), Some(1))
            .expect("captain ordinary candidate");
        assert_eq!(
            wake(
                policy
                    .take_delivery(&recipient(Role::Captain), Some(1))
                    .expect("captain ordinary wake")
            )
            .budget_class,
            BudgetClass::Ordinary
        );
        policy
            .observe(
                update(
                    2,
                    UpdateKind::Health {
                        component_id: "quota".into(),
                        state: HealthState::Warning,
                    },
                ),
                Some(2),
            )
            .expect("warning remains queued");
        assert!(matches!(
            policy
                .take_delivery(&recipient(Role::Captain), Some(2))
                .expect("warning cannot consume reserve"),
            DeliveryDecision::Deferred {
                pending_refs: 1,
                ..
            }
        ));
        assert_eq!(policy.pending_refs(), 1);

        let mut worker = candidate(3);
        worker.recipient.role = Role::TaskWorker;
        policy.observe(worker, Some(3)).expect("worker own budget");
        assert_eq!(
            wake(
                policy
                    .take_delivery(&recipient(Role::TaskWorker), Some(3))
                    .expect("worker unaffected by captain exhaustion")
            )
            .budget_class,
            BudgetClass::Ordinary
        );
        let mut other_captain = candidate(4);
        other_captain.recipient.principal_id =
            PrincipalId::new("other-captain").expect("fixture principal");
        let other_recipient = other_captain.recipient.clone();
        policy
            .observe(other_captain, Some(4))
            .expect("separate captain");
        assert_eq!(
            wake(
                policy
                    .take_delivery(&other_recipient, Some(4))
                    .expect("other captain owns independent ordinary budget")
            )
            .budget_class,
            BudgetClass::Ordinary
        );
        let mut super_captain = candidate(5);
        super_captain.recipient.role = Role::SuperCaptain;
        policy
            .observe(super_captain, Some(5))
            .expect("super captain candidate retained");
        assert!(matches!(
            policy
                .take_delivery(&recipient(Role::SuperCaptain), Some(5))
                .expect("zero super captain budget respected"),
            DeliveryDecision::Deferred { .. }
        ));
        policy
            .observe(candidate(6), Some(6))
            .expect("captain critical candidate");
        let batch = wake(
            policy
                .take_delivery(&recipient(Role::Captain), Some(6))
                .expect("unused captain reserve survives other recipients"),
        );
        assert_eq!(batch.budget_class, BudgetClass::CriticalReserve);
        assert_eq!(batch.refs[0].source.sequence, "6");
        assert_eq!(policy.pending_refs(), 2);
    }

    #[test]
    fn cleared_health_lineage_is_never_partially_attached_to_a_small_delivery() {
        for byte_limit in [1_024, 8_192] {
            let mut config = GovernorConfig::default();
            config.captain.max_delivery_refs = if byte_limit == 1_024 { 16 } else { 2 };
            config.captain.max_delivery_bytes = byte_limit;
            let mut policy = governor(config);
            for (sequence, state) in [
                (1, HealthState::Green),
                (2, HealthState::Critical),
                (3, HealthState::Green),
            ] {
                policy
                    .observe(
                        update(
                            sequence,
                            UpdateKind::Health {
                                component_id: "disk".into(),
                                state,
                            },
                        ),
                        Some(sequence),
                    )
                    .expect("health history retained");
            }
            policy
                .observe(candidate(4), Some(4))
                .expect("unrelated actionable candidate");
            let delivery = wake(
                policy
                    .take_delivery(&recipient(Role::Captain), Some(4))
                    .expect("candidate fits without incomplete health history"),
            );
            assert_eq!(delivery.refs.len(), 1);
            assert_eq!(delivery.refs[0].kind, UpdateTag::Candidate);
            assert_eq!(policy.pending_refs(), 3);
            assert_eq!(
                policy
                    .take_delivery(&recipient(Role::Captain), Some(4))
                    .expect("cleared health history stays quiet"),
                DeliveryDecision::Quiet
            );
        }
    }

    #[test]
    fn essential_health_lineage_that_cannot_fit_fails_without_spending_or_dropping() {
        let mut config = GovernorConfig::default();
        config.captain.max_delivery_refs = 2;
        let mut policy = governor(config);
        for (sequence, state) in [
            (1, HealthState::Green),
            (2, HealthState::Warning),
            (3, HealthState::Critical),
        ] {
            policy
                .observe(
                    update(
                        sequence,
                        UpdateKind::Health {
                            component_id: "quota".into(),
                            state,
                        },
                    ),
                    Some(sequence),
                )
                .expect("health history retained");
        }
        assert_eq!(
            policy.take_delivery(&recipient(Role::Captain), Some(3)),
            Err(GovernorError::DeliveryTooLarge)
        );
        assert_eq!(policy.pending_refs(), 3);
        assert!(policy.spent.is_empty());
        policy.config.captain.max_delivery_refs = 3;
        let delivery = wake(
            policy
                .take_delivery(&recipient(Role::Captain), Some(3))
                .expect("explicitly large enough delivery keeps complete lineage"),
        );
        assert_eq!(delivery.refs.len(), 3);
        assert_eq!(delivery.budget_class, BudgetClass::Ordinary);
        assert_eq!(policy.pending_refs(), 0);
    }

    #[test]
    fn health_green_and_unchanged_states_never_wake_and_cleared_red_stays_in_lineage() {
        let mut policy = governor(GovernorConfig::default());
        let health = |sequence, state| {
            update(
                sequence,
                UpdateKind::Health {
                    component_id: "disk-pressure".into(),
                    state,
                },
            )
        };
        policy
            .observe(health(1, HealthState::Green), Some(1))
            .expect("green observation");
        assert_eq!(
            policy
                .take_delivery(&recipient(Role::Captain), Some(1))
                .expect("green quiet"),
            DeliveryDecision::Quiet
        );
        assert_eq!(
            policy
                .observe(health(2, HealthState::Green), Some(2))
                .expect("unchanged green"),
            Observation::HealthUnchanged
        );
        policy
            .observe(health(3, HealthState::Critical), Some(3))
            .expect("red transition");
        policy
            .observe(health(4, HealthState::Green), Some(4))
            .expect("red cleared before attention");
        assert_eq!(
            policy
                .take_delivery(&recipient(Role::Captain), Some(4))
                .expect("resolved health quiet"),
            DeliveryDecision::Quiet
        );
        policy
            .observe(question(5, QuestionState::Open), Some(5))
            .expect("separate actionable question");
        let batch = wake(
            policy
                .take_delivery(&recipient(Role::Captain), Some(5))
                .expect("question delivery includes exact health lineage"),
        );
        assert_eq!(
            batch
                .refs
                .iter()
                .filter(|reference| reference.kind == UpdateTag::Health)
                .count(),
            3
        );
        assert!(!batch.reasons.contains(&WakeReason::HealthCritical));
        assert!(batch.reasons.contains(&WakeReason::QuestionOpened));
    }

    #[test]
    fn unknown_backward_clocks_source_collisions_and_overflow_fail_without_mutation() {
        let mut policy = governor(GovernorConfig::default());
        let first = progress(1, 1);
        assert_eq!(
            policy.observe(first.clone(), None),
            Err(GovernorError::UnknownClock)
        );
        assert_eq!(policy.topic_count(), 0);
        policy
            .observe(first.clone(), Some(10))
            .expect("known logical clock");
        let mut collision = first.clone();
        collision.evidence_ref = "event:different-content".into();
        assert_eq!(
            policy.observe(collision, Some(11)),
            Err(GovernorError::SourceCollision)
        );
        assert_eq!(
            policy.observe(first, Some(100)).expect("exact duplicate"),
            Observation::DuplicateSource
        );
        assert_eq!(
            policy.observe(progress(2, 2), Some(99)),
            Err(GovernorError::BackwardClock)
        );
        assert_eq!(
            policy.take_delivery(&recipient(Role::Captain), None),
            Err(GovernorError::UnknownClock)
        );
        let mut overflow = governor(GovernorConfig::default());
        overflow.next_order = u64::MAX;
        assert_eq!(
            overflow.observe(candidate(1), Some(1)),
            Err(GovernorError::Overflow)
        );
        assert_eq!(overflow.topic_count(), 0);
        overflow.next_order = 0;
        overflow
            .observe(candidate(1), Some(1))
            .expect("same unacknowledged source retry");
        overflow.next_delivery = u64::MAX;
        assert_eq!(
            overflow.take_delivery(&recipient(Role::Captain), Some(1)),
            Err(GovernorError::Overflow)
        );
        assert_eq!(overflow.pending_refs(), 1);
    }

    #[test]
    fn topic_source_queue_and_update_size_bounds_apply_backpressure() {
        let config = GovernorConfig {
            max_topics: 1,
            max_source_ids: 3,
            max_pending_refs: 2,
            ..Default::default()
        };
        let mut policy = governor(config);
        policy
            .observe(candidate(1), Some(1))
            .expect("first retained candidate");
        policy
            .observe(candidate(2), Some(2))
            .expect("second retained candidate");
        assert_eq!(
            policy.observe(candidate(3), Some(3)),
            Err(GovernorError::Capacity("pending critical references"))
        );
        let mut foreign_topic = progress(3, 3);
        foreign_topic.scope.task_id = TaskId::new("TASK-OTHER").expect("fixture task");
        assert_eq!(
            policy.observe(foreign_topic, Some(3)),
            Err(GovernorError::Capacity("topics"))
        );
        wake(
            policy
                .take_delivery(&recipient(Role::Captain), Some(3))
                .expect("drain exact pending references"),
        );
        policy
            .observe(candidate(3), Some(3))
            .expect("retry source after capacity returned");
        assert_eq!(
            policy.observe(progress(4, 4), Some(4)),
            Err(GovernorError::Capacity("source identities"))
        );
        let mut oversized = question(10, QuestionState::Open);
        if let UpdateKind::Question { facts, .. } = &mut oversized.update {
            facts.protected_constraints = vec!["x".repeat(1024); 32];
        }
        assert!(oversized.validate().is_err());
        assert!(
            AttentionGovernor::new(GovernorConfig {
                max_topics: MAX_TOPICS + 1,
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn delivery_budget_never_truncates_authority_or_evidence_reference() {
        let mut config = GovernorConfig::default();
        config.captain.max_delivery_bytes = 1024;
        let mut policy = governor(config);
        let mut large = candidate(1);
        large.authority_ref = "a".repeat(512);
        large.evidence_ref = "e".repeat(512);
        policy
            .observe(large.clone(), Some(1))
            .expect("valid16KiB update");
        assert_eq!(
            policy.take_delivery(&recipient(Role::Captain), Some(1)),
            Err(GovernorError::DeliveryTooLarge)
        );
        assert_eq!(policy.pending_refs(), 1);
        policy.config.captain.max_delivery_bytes = 8192;
        let batch = wake(
            policy
                .take_delivery(&recipient(Role::Captain), Some(1))
                .expect("explicitly expanded reference budget"),
        );
        assert_eq!(batch.refs[0].authority_ref, large.authority_ref);
        assert_eq!(batch.refs[0].evidence_ref, large.evidence_ref);
    }

    #[test]
    fn replay_of_recorded_updates_ticks_and_delivery_requests_is_identical() {
        let replay = || {
            let mut policy = governor(GovernorConfig::default());
            let mut decisions = vec![];
            for (tick, event) in [
                (1, progress(1, 1)),
                (2, question(2, QuestionState::Open)),
                (3, candidate(3)),
            ] {
                policy.observe(event, Some(tick)).expect("recorded update");
                decisions.push(
                    policy
                        .take_delivery(&recipient(Role::Captain), Some(tick))
                        .expect("recorded delivery request"),
                );
            }
            serde_json::to_string(&decisions).expect("deterministic delivery JSON")
        };
        assert_eq!(replay(), replay());
    }
}
