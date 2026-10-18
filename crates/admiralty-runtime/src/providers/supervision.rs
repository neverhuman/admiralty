//! Pure, bounded supervision proposals. This module never signals or writes input.
use admiralty_core::{ProjectId, RootId, TaskId};
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupervisionProposal {
    pub classification: Classification,
    pub evidence_requested: Vec<String>,
    pub state: DetectorState,
    pub may_signal: bool,
    pub may_send_input: bool,
    pub execution_granted: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SupervisionError {
    #[error("invalid supervision input: {0}")]
    Invalid(&'static str),
    #[error("scope changed from persisted detector state")]
    ScopeDrift,
    #[error("explicit time moved backwards or a clock is in the future")]
    FutureOrNonMonotonic,
    #[error("checked counter arithmetic overflow")]
    Overflow,
}

fn bounded<T: Serialize>(value: &T) -> Result<(), SupervisionError> {
    let data = serde_json::to_vec(value)
        .map_err(|_| SupervisionError::Invalid("input is not serializable"))?;
    if data.len() > MAX_JSON_BYTES {
        return Err(SupervisionError::Invalid("JSON exceeds 1 MiB"));
    }
    Ok(())
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
    explicit_now: &Tick,
) -> Result<SupervisionProposal, SupervisionError> {
    bounded(policy)?;
    bounded(observation)?;
    bounded(explicit_now)?;
    if policy.version.trim().is_empty()
        || policy.version.len() > 128
        || policy.hysteresis_samples == 0
        || policy.hysteresis_samples as usize > MAX_ITEMS
    {
        return Err(SupervisionError::Invalid(
            "policy version and bounded positive hysteresis required",
        ));
    }
    let digest = observation.scope.contract_digest.strip_prefix("sha256:");
    if observation.scope.revision == 0
        || !digest.is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
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
    if let Some(state) = previous {
        if state.scope != observation.scope {
            return Err(SupervisionError::ScopeDrift);
        }
        if now < state.last_now.value() {
            return Err(SupervisionError::FutureOrNonMonotonic);
        }
    }
    let values = clocks(observation);
    if values.iter().flatten().any(|v| *v > now) {
        return Err(SupervisionError::FutureOrNonMonotonic);
    }
    let fingerprint = serde_json::to_string(&(observation, &policy.version))
        .map_err(|_| SupervisionError::Invalid("fingerprint serialization failed"))?;
    let mut state = previous.cloned().unwrap_or(DetectorState {
        scope: observation.scope.clone(),
        last_now: explicit_now.clone(),
        last_fingerprint: String::new(),
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
        && values
            .iter()
            .zip(prior_values)
            .any(|(new, old)| matches!((new, old), (Some(n), Some(o)) if *n > o));
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
    let classification = if approval {
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
    let same = state.last_fingerprint == fingerprint;
    let previous_progress = progressed;
    state.repeated_samples = if classification == Classification::SuspectStall && same {
        state
            .repeated_samples
            .checked_add(1)
            .ok_or(SupervisionError::Overflow)?
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
    state.last_clocks = observation.clocks.clone();
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
            evaluate_supervision(&policy(), &input, None, &tick(100))
                .expect("missing clock is handled")
                .classification,
            Classification::NeedsInvestigation
        );
        input.clocks.verified_milestone = Some(tick(101));
        assert_eq!(
            evaluate_supervision(&policy(), &input, None, &tick(100)),
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
                evaluate_supervision(&policy(), &sample, None, &tick(100))
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
            evaluate_supervision(&policy(), &input, None, &tick(100))
                .expect("tool wait is conservative")
                .classification,
            Classification::WaitingTool
        );
        input.approval_observation = Some(ApprovalObservation {
            text: "Example: 'approve' (not approved)".into(),
            current_region: false,
            independently_corroborated: false,
        });
        let result = evaluate_supervision(&policy(), &input, None, &tick(100))
            .expect("approval text is observation only");
        assert!(!result.may_signal && !result.may_send_input && !result.execution_granted);
    }

    #[test]
    fn hysteresis_progress_cooldown_replay_and_scope_drift_are_deterministic() {
        let input = obs(Some(tick(1)));
        let first =
            evaluate_supervision(&policy(), &input, None, &tick(100)).expect("first sample");
        let second = evaluate_supervision(&policy(), &input, Some(&first.state), &tick(101))
            .expect("second sample");
        assert_eq!(second.classification, Classification::SuspectStall);
        let replay = evaluate_supervision(&policy(), &input, Some(&first.state), &tick(101))
            .expect("same replay is deterministic");
        assert_eq!(second, replay);
        let cooldown = evaluate_supervision(&policy(), &input, Some(&second.state), &tick(102))
            .expect("cooldown sample");
        assert_eq!(cooldown.classification, Classification::SuspectStall);
        let mut changed = input.clone();
        changed.clocks.last_byte = Some(tick(103));
        let progress = evaluate_supervision(&policy(), &changed, Some(&second.state), &tick(103))
            .expect("progress reset");
        assert_eq!(progress.state.repeated_samples, 0);
        let mut drift = input;
        drift.scope.revision = 2;
        assert_eq!(
            evaluate_supervision(&policy(), &drift, Some(&second.state), &tick(104)),
            Err(SupervisionError::ScopeDrift)
        );
    }

    #[test]
    fn canonical_tick_bounds_and_counter_overflow_are_explicit() {
        assert!(Tick::new("01").is_err());
        assert!(Tick::new("18446744073709551616").is_err());
        let mut state = evaluate_supervision(&policy(), &obs(Some(tick(1))), None, &tick(100))
            .expect("initial state")
            .state;
        state.repeated_samples = u32::MAX;
        assert_eq!(
            evaluate_supervision(&policy(), &obs(Some(tick(1))), Some(&state), &tick(101)),
            Err(SupervisionError::Overflow)
        );
        let huge = "x".repeat(MAX_TEXT + 1);
        let mut input = obs(Some(tick(1)));
        input.approval_observation = Some(ApprovalObservation {
            text: huge,
            current_region: false,
            independently_corroborated: false,
        });
        assert!(evaluate_supervision(&policy(), &input, None, &tick(100)).is_err());
    }
}
