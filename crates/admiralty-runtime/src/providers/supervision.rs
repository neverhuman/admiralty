//! Pure, bounded supervision proposals. This module never signals or writes input.
use admiralty_core::{
    AttemptId, PrincipalId, ProjectId, RootId, TaskId, content_digest, limits::bounded_json_size,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_JSON_BYTES: usize = 1024 * 1024;
const MAX_ITEMS: usize = 1024;
const MAX_TEXT: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Tick(String);

impl Tick {
    pub fn new(value: impl Into<String>) -> Result<Self, SupervisionError> {
        let value = value.into();
        if value.len() > 20 {
            return Err(SupervisionError::Invalid(
                "tick exceeds canonical u64 width",
            ));
        }
        let n = value
            .parse::<u64>()
            .map_err(|_| SupervisionError::Invalid("tick must be a canonical decimal u64"))?;
        if n.to_string() != value {
            return Err(SupervisionError::Invalid("tick must be canonical"));
        }
        Ok(Self(value))
    }
    pub fn value(&self) -> u64 {
        self.0.parse().expect("Tick is validated at construction")
    }
}
impl TryFrom<String> for Tick {
    type Error = SupervisionError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<Tick> for String {
    fn from(value: Tick) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisionScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub owner: PrincipalId,
    pub incarnation: String,
    pub authority_ref: String,
    pub contract_digest: String,
    pub revision: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalClocks {
    pub last_byte: Option<Tick>,
    pub provider_event: Option<Tick>,
    pub tool_heartbeat: Option<Tick>,
    pub artifact_change: Option<Tick>,
    pub distinct_command: Option<Tick>,
    pub completed_check: Option<Tick>,
    pub verified_milestone: Option<Tick>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    Alive,
    Exited,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    Running,
    Finished,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderWait {
    None,
    Waiting { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalObservation {
    pub text: String,
    pub current_region: bool,
    pub independently_corroborated: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisionObservation {
    pub scope: SupervisionScope,
    pub clocks: SignalClocks,
    pub process_state: ProcessState,
    pub tool_state: ToolState,
    pub provider_wait: ProviderWait,
    pub approval_observation: Option<ApprovalObservation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectionPolicy {
    pub version: String,
    pub stall_after: Tick,
    pub hysteresis_samples: u32,
    pub cooldown: Tick,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectorState {
    pub scope: SupervisionScope,
    pub last_now: Tick,
    pub last_fingerprint: String,
    pub policy_digest: String,
    pub last_clocks: SignalClocks,
    pub repeated_samples: u32,
    pub last_proposal: Option<Tick>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    Healthy,
    WaitingTool,
    ProviderWaiting,
    ApprovalObserved,
    SuspectStall,
    NeedsInvestigation,
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SupervisionProposal {
    pub classification: Classification,
    pub evidence_requested: Vec<String>,
    pub state: DetectorState,
    may_signal: bool,
    may_send_input: bool,
    execution_granted: bool,
    pub replayed: bool,
}
impl SupervisionProposal {
    pub fn may_signal(&self) -> bool {
        self.may_signal
    }
    pub fn may_send_input(&self) -> bool {
        self.may_send_input
    }
    pub fn execution_granted(&self) -> bool {
        self.execution_granted
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SupervisionError {
    #[error("invalid supervision input: {0}")]
    Invalid(&'static str),
    #[error("scope changed from persisted detector state")]
    ScopeDrift,
    #[error("recorded logical clock is unknown")]
    UnknownClock,
    #[error("detector policy changed without an explicit migration")]
    PolicyDrift,
    #[error("different observation reused an already recorded tick")]
    ObservationCollision,
    #[error("explicit time moved backwards or a clock is in the future")]
    FutureOrNonMonotonic,
    #[error("checked counter arithmetic overflow")]
    Overflow,
}

fn bounded<T: Serialize>(value: &T) -> Result<(), SupervisionError> {
    bounded_json_size(value, MAX_JSON_BYTES)
        .map(|_| ())
        .map_err(|_| SupervisionError::Invalid("JSON exceeds 1 MiB or is not serializable"))
}

fn clocks(o: &SupervisionObservation) -> [Option<u64>; 7] {
    [
        o.clocks.last_byte.as_ref(),
        o.clocks.provider_event.as_ref(),
        o.clocks.tool_heartbeat.as_ref(),
        o.clocks.artifact_change.as_ref(),
        o.clocks.distinct_command.as_ref(),
        o.clocks.completed_check.as_ref(),
        o.clocks.verified_milestone.as_ref(),
    ]
    .map(|x| x.map(Tick::value))
}

pub fn evaluate_supervision(
    policy: &DetectionPolicy,
    observation: &SupervisionObservation,
    previous: Option<&DetectorState>,
    explicit_now: Option<&Tick>,
) -> Result<SupervisionProposal, SupervisionError> {
    let explicit_now = explicit_now.ok_or(SupervisionError::UnknownClock)?;
    bounded(policy)?;
    bounded(observation)?;
    bounded(explicit_now)?;
    if policy.version.trim().is_empty()
        || policy.version.len() > 128
        || policy.hysteresis_samples == 0
        || policy.hysteresis_samples as usize > MAX_ITEMS
        || policy.stall_after.value() == 0
        || policy.version.chars().any(char::is_control)
    {
        return Err(SupervisionError::Invalid(
            "policy version and bounded positive hysteresis required",
        ));
    }
    let digest = observation.scope.contract_digest.strip_prefix("sha256:");
    if observation.scope.revision == 0
        || !digest.is_some_and(|hex| {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        })
        || PrincipalId::new(&observation.scope.incarnation).is_err()
        || observation.scope.authority_ref.is_empty()
        || observation.scope.authority_ref.len() > 512
        || observation
            .scope
            .authority_ref
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(SupervisionError::Invalid(
            "canonical SHA-256 contract digest and nonzero revision required",
        ));
    }
    if let Some(a) = &observation.approval_observation
        && a.text.len() > MAX_TEXT
    {
        return Err(SupervisionError::Invalid(
            "approval observation exceeds 4096 bytes",
        ));
    }
    if let ProviderWait::Waiting { reason } = &observation.provider_wait
        && reason.len() > MAX_TEXT
    {
        return Err(SupervisionError::Invalid(
            "provider wait reason exceeds 4096 bytes",
        ));
    }
    let now = explicit_now.value();
    let policy_digest =
        content_digest(policy).map_err(|_| SupervisionError::Invalid("policy digest"))?;
    if let Some(state) = previous {
        bounded(state)?;
        if state.scope != observation.scope {
            return Err(SupervisionError::ScopeDrift);
        }
        if now < state.last_now.value() {
            return Err(SupervisionError::FutureOrNonMonotonic);
        }
        if state.policy_digest != policy_digest {
            return Err(SupervisionError::PolicyDrift);
        }
        if state.last_fingerprint.len() != 71
            || !state.last_fingerprint.starts_with("sha256:")
            || state.repeated_samples > MAX_ITEMS as u32
        {
            return Err(SupervisionError::Invalid(
                "invalid persisted detector bounds",
            ));
        }
        if state
            .last_proposal
            .as_ref()
            .is_some_and(|tick| tick.value() > state.last_now.value())
        {
            return Err(SupervisionError::FutureOrNonMonotonic);
        }
    }
    let values = clocks(observation);
    if values.iter().flatten().any(|v| *v > now) {
        return Err(SupervisionError::FutureOrNonMonotonic);
    }
    let fingerprint = content_digest(&(observation, policy))
        .map_err(|_| SupervisionError::Invalid("fingerprint serialization failed"))?;
    let mut state = previous.cloned().unwrap_or(DetectorState {
        scope: observation.scope.clone(),
        last_now: explicit_now.clone(),
        last_fingerprint: String::new(),
        policy_digest,
        last_clocks: observation.clocks.clone(),
        repeated_samples: 0,
        last_proposal: None,
    });
    let prior_values = previous
        .map(|p| {
            [
                p.last_clocks.last_byte.as_ref(),
                p.last_clocks.provider_event.as_ref(),
                p.last_clocks.tool_heartbeat.as_ref(),
                p.last_clocks.artifact_change.as_ref(),
                p.last_clocks.distinct_command.as_ref(),
                p.last_clocks.completed_check.as_ref(),
                p.last_clocks.verified_milestone.as_ref(),
            ]
            .map(|x| x.map(Tick::value))
        })
        .unwrap_or([None; 7]);
    let progressed = previous.is_some()
        && values.iter().zip(prior_values).any(|(new, old)| {
            matches!((new, old), (Some(n), Some(o)) if *n > o)
                || matches!((new, old), (Some(_), None))
        });
    if let Some(previous) = previous {
        if values
            .iter()
            .zip(prior_values)
            .any(|(new, old)| matches!((new,old),(Some(n),Some(o)) if *n < o))
            || prior_values
                .iter()
                .flatten()
                .any(|tick| *tick > previous.last_now.value())
        {
            return Err(SupervisionError::FutureOrNonMonotonic);
        }
        if now == previous.last_now.value() && previous.last_fingerprint != fingerprint {
            return Err(SupervisionError::ObservationCollision);
        }
    }
    // Independent clocks are assessed separately; long-running tools are progress evidence, not model silence.
    let known: Vec<u64> = values.into_iter().flatten().collect();
    let stalled_count = known
        .iter()
        .filter(|v| {
            now.checked_sub(**v)
                .is_some_and(|age| age >= policy.stall_after.value())
        })
        .count();
    let unknown = known.len() < 7
        || observation.process_state == ProcessState::Unknown
        || observation.tool_state == ToolState::Unknown;
    let live_tool = observation.tool_state == ToolState::Running
        || observation.process_state == ProcessState::Alive;
    let approval = observation.approval_observation.as_ref().is_some_and(|a| {
        a.current_region && a.independently_corroborated && !a.text.trim().is_empty()
    });
    let classification = if observation.process_state != ProcessState::Alive {
        Classification::NeedsInvestigation
    } else if approval {
        Classification::ApprovalObserved
    } else if matches!(observation.provider_wait, ProviderWait::Waiting { .. }) {
        Classification::ProviderWaiting
    } else if observation.tool_state == ToolState::Running {
        Classification::WaitingTool
    } else if unknown || observation.process_state == ProcessState::Exited {
        Classification::NeedsInvestigation
    } else if stalled_count >= 2 && live_tool {
        Classification::SuspectStall
    } else if stalled_count == 0 {
        Classification::Healthy
    } else {
        Classification::NeedsInvestigation
    };
    if previous.is_some_and(|previous| previous.last_now.value() == now) {
        return Ok(SupervisionProposal {
            classification,
            evidence_requested: vec![],
            state,
            may_signal: false,
            may_send_input: false,
            execution_granted: false,
            replayed: true,
        });
    }
    let same = state.last_fingerprint == fingerprint;
    let previous_progress = progressed;
    state.repeated_samples = if classification == Classification::SuspectStall && same {
        state
            .repeated_samples
            .checked_add(1)
            .ok_or(SupervisionError::Overflow)?
            .min(policy.hysteresis_samples)
    } else if classification == Classification::SuspectStall {
        1
    } else {
        0
    };
    if previous_progress {
        state.repeated_samples = 0;
    }
    state.last_now = explicit_now.clone();
    state.last_fingerprint = fingerprint;
    let mut retained = observation.clocks.clone();
    for (new, old) in [
        (&mut retained.last_byte, &state.last_clocks.last_byte),
        (
            &mut retained.provider_event,
            &state.last_clocks.provider_event,
        ),
        (
            &mut retained.tool_heartbeat,
            &state.last_clocks.tool_heartbeat,
        ),
        (
            &mut retained.artifact_change,
            &state.last_clocks.artifact_change,
        ),
        (
            &mut retained.distinct_command,
            &state.last_clocks.distinct_command,
        ),
        (
            &mut retained.completed_check,
            &state.last_clocks.completed_check,
        ),
        (
            &mut retained.verified_milestone,
            &state.last_clocks.verified_milestone,
        ),
    ] {
        if new.is_none() {
            *new = old.clone();
        }
    }
    state.last_clocks = retained;
    state.scope = observation.scope.clone();
    let cooled = state.last_proposal.as_ref().is_some_and(|t| {
        now.checked_sub(t.value())
            .is_none_or(|age| age < policy.cooldown.value())
    });
    let propose = classification == Classification::SuspectStall
        && stalled_count >= 2
        && state.repeated_samples >= policy.hysteresis_samples
        && !cooled;
    if propose {
        state.last_proposal = Some(explicit_now.clone());
    }
    let evidence_requested = if propose {
        vec!["confirm independent process and provider evidence".to_owned()]
    } else if unknown || classification == Classification::NeedsInvestigation {
        vec!["collect missing clocks and current process/tool state".to_owned()]
    } else {
        Vec::new()
    };
    Ok(SupervisionProposal {
        classification: if propose {
            Classification::SuspectStall
        } else {
            classification
        },
        evidence_requested,
        state,
        may_signal: false,
        may_send_input: false,
        execution_granted: false,
        replayed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use admiralty_core::{ProjectId, RootId, TaskId};

    fn tick(n: u64) -> Tick {
        Tick::new(n.to_string()).expect("fixture tick is canonical")
    }
    fn scope() -> SupervisionScope {
        SupervisionScope {
            project_id: ProjectId::new("p").expect("valid project fixture"),
            root_id: RootId::new("r").expect("valid root fixture"),
            task_id: TaskId::new("t").expect("valid task fixture"),
            attempt_id: AttemptId::new("attempt").expect("fixture attempt"),
            owner: PrincipalId::new("owner").expect("fixture owner"),
            incarnation: "boot-1".into(),
            authority_ref: "grant:task".into(),
            contract_digest: format!("sha256:{}", "a".repeat(64)),
            revision: 1,
        }
    }
    fn obs(clock: Option<Tick>) -> SupervisionObservation {
        SupervisionObservation {
            scope: scope(),
            clocks: SignalClocks {
                last_byte: clock.clone(),
                provider_event: clock.clone(),
                tool_heartbeat: clock.clone(),
                artifact_change: clock.clone(),
                distinct_command: clock.clone(),
                completed_check: clock.clone(),
                verified_milestone: clock,
            },
            process_state: ProcessState::Alive,
            tool_state: ToolState::Finished,
            provider_wait: ProviderWait::None,
            approval_observation: None,
        }
    }
    fn policy() -> DetectionPolicy {
        DetectionPolicy {
            version: "v1".into(),
            stall_after: tick(10),
            hysteresis_samples: 2,
            cooldown: tick(5),
        }
    }

    #[test]
    fn every_clock_is_independent_and_missing_or_future_data_is_not_healthy() {
        let mut input = obs(Some(tick(100)));
        input.clocks.verified_milestone = None;
        assert_eq!(
            evaluate_supervision(&policy(), &input, None, Some(&tick(100)))
                .expect("missing clock is handled")
                .classification,
            Classification::NeedsInvestigation
        );
        input.clocks.verified_milestone = Some(tick(101));
        assert_eq!(
            evaluate_supervision(&policy(), &input, None, Some(&tick(100))),
            Err(SupervisionError::FutureOrNonMonotonic)
        );
        for index in 0..7 {
            let mut sample = obs(Some(tick(100)));
            let clocks = [
                &mut sample.clocks.last_byte,
                &mut sample.clocks.provider_event,
                &mut sample.clocks.tool_heartbeat,
                &mut sample.clocks.artifact_change,
                &mut sample.clocks.distinct_command,
                &mut sample.clocks.completed_check,
                &mut sample.clocks.verified_milestone,
            ];
            *clocks[index] = Some(tick(1));
            assert_eq!(
                evaluate_supervision(&policy(), &sample, None, Some(&tick(100)))
                    .expect("independent clock sample")
                    .classification,
                Classification::NeedsInvestigation
            );
        }
    }

    #[test]
    fn live_tool_silence_is_waiting_and_approval_words_grant_nothing() {
        let mut input = obs(Some(tick(1)));
        input.tool_state = ToolState::Running;
        assert_eq!(
            evaluate_supervision(&policy(), &input, None, Some(&tick(100)))
                .expect("tool wait is conservative")
                .classification,
            Classification::WaitingTool
        );
        input.approval_observation = Some(ApprovalObservation {
            text: "Example: 'approve' (not approved)".into(),
            current_region: false,
            independently_corroborated: false,
        });
        let result = evaluate_supervision(&policy(), &input, None, Some(&tick(100)))
            .expect("approval text is observation only");
        assert!(!result.may_signal && !result.may_send_input && !result.execution_granted);
    }

    #[test]
    fn hysteresis_progress_cooldown_replay_and_scope_drift_are_deterministic() {
        let input = obs(Some(tick(1)));
        let first =
            evaluate_supervision(&policy(), &input, None, Some(&tick(100))).expect("first sample");
        let second = evaluate_supervision(&policy(), &input, Some(&first.state), Some(&tick(101)))
            .expect("second sample");
        assert_eq!(second.classification, Classification::SuspectStall);
        let replay = evaluate_supervision(&policy(), &input, Some(&first.state), Some(&tick(101)))
            .expect("same replay is deterministic");
        assert_eq!(second, replay);
        let cooldown =
            evaluate_supervision(&policy(), &input, Some(&second.state), Some(&tick(102)))
                .expect("cooldown sample");
        assert_eq!(cooldown.classification, Classification::SuspectStall);
        let mut changed = input.clone();
        changed.clocks.last_byte = Some(tick(103));
        let progress =
            evaluate_supervision(&policy(), &changed, Some(&second.state), Some(&tick(103)))
                .expect("progress reset");
        assert_eq!(progress.state.repeated_samples, 0);
        let mut drift = input;
        drift.scope.revision = 2;
        assert_eq!(
            evaluate_supervision(&policy(), &drift, Some(&second.state), Some(&tick(104))),
            Err(SupervisionError::ScopeDrift)
        );
    }

    #[test]
    fn canonical_tick_bounds_and_counter_overflow_are_explicit() {
        assert!(Tick::new("01").is_err());
        assert!(Tick::new("18446744073709551616").is_err());
        let mut state =
            evaluate_supervision(&policy(), &obs(Some(tick(1))), None, Some(&tick(100)))
                .expect("initial state")
                .state;
        state.repeated_samples = u32::MAX;
        assert_eq!(
            evaluate_supervision(
                &policy(),
                &obs(Some(tick(1))),
                Some(&state),
                Some(&tick(101))
            ),
            Err(SupervisionError::Invalid(
                "invalid persisted detector bounds"
            ))
        );
        let huge = "x".repeat(MAX_TEXT + 1);
        let mut input = obs(Some(tick(1)));
        input.approval_observation = Some(ApprovalObservation {
            text: huge,
            current_region: false,
            independently_corroborated: false,
        });
        assert!(evaluate_supervision(&policy(), &input, None, Some(&tick(100))).is_err());
    }
    #[test]
    fn duplicate_ticks_do_not_satisfy_hysteresis_and_changed_tick_content_collides() {
        let input = obs(Some(tick(1)));
        let first = evaluate_supervision(&policy(), &input, None, Some(&tick(100)))
            .expect("initial observation");
        let replay = evaluate_supervision(&policy(), &input, Some(&first.state), Some(&tick(100)))
            .expect("same recorded observation");
        assert!(replay.replayed);
        assert_eq!(replay.state.repeated_samples, 1);
        assert!(replay.evidence_requested.is_empty());
        let mut changed = input.clone();
        changed.clocks.last_byte = Some(tick(2));
        assert_eq!(
            evaluate_supervision(&policy(), &changed, Some(&first.state), Some(&tick(100))),
            Err(SupervisionError::ObservationCollision)
        );
        assert_eq!(
            evaluate_supervision(&policy(), &input, Some(&first.state), None),
            Err(SupervisionError::UnknownClock)
        );
        assert!(!replay.may_signal() && !replay.may_send_input() && !replay.execution_granted());
    }
    #[test]
    fn policy_scope_and_clock_regressions_cannot_reset_detector_history() {
        let input = obs(Some(tick(100)));
        let first = evaluate_supervision(&policy(), &input, None, Some(&tick(100)))
            .expect("initial history");
        assert_eq!(first.state.last_fingerprint.len(), 71);
        let mut changed_policy = policy();
        changed_policy.stall_after = tick(1);
        assert_eq!(
            evaluate_supervision(
                &changed_policy,
                &input,
                Some(&first.state),
                Some(&tick(101))
            ),
            Err(SupervisionError::PolicyDrift)
        );
        let mut gap = input.clone();
        gap.clocks.last_byte = None;
        let gap = evaluate_supervision(&policy(), &gap, Some(&first.state), Some(&tick(101)))
            .expect("missing sample preserves last known");
        assert_eq!(gap.state.last_clocks.last_byte, Some(tick(100)));
        let mut regression = input.clone();
        regression.clocks.last_byte = Some(tick(99));
        assert_eq!(
            evaluate_supervision(&policy(), &regression, Some(&gap.state), Some(&tick(102))),
            Err(SupervisionError::FutureOrNonMonotonic)
        );
        for mutation in ["owner", "attempt", "incarnation"] {
            let mut changed = input.clone();
            match mutation {
                "owner" => changed.scope.owner = PrincipalId::new("other").expect("fixture owner"),
                "attempt" => {
                    changed.scope.attempt_id = AttemptId::new("other").expect("fixture attempt")
                }
                _ => changed.scope.incarnation = "new-boot".into(),
            }
            assert_eq!(
                evaluate_supervision(&policy(), &changed, Some(&first.state), Some(&tick(101))),
                Err(SupervisionError::ScopeDrift)
            );
        }
        let mut large = first.state;
        large.last_fingerprint = "x".repeat(MAX_JSON_BYTES + 1);
        assert!(evaluate_supervision(&policy(), &input, Some(&large), Some(&tick(101))).is_err());
    }
    #[test]
    fn contradictory_dead_process_and_running_tool_requires_investigation() {
        let mut input = obs(Some(tick(1)));
        input.process_state = ProcessState::Exited;
        input.tool_state = ToolState::Running;
        let result = evaluate_supervision(&policy(), &input, None, Some(&tick(100)))
            .expect("contradiction is explicit");
        assert_eq!(result.classification, Classification::NeedsInvestigation);
        assert!(!result.may_signal());
    }
}
