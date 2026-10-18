//! Pure, fail-closed retirement proposals for registered assets.
//!
//! This module records observations and proposals only. It never inspects or
//! changes a filesystem, process, pin store, or backup.

use crate::{PrincipalId, ProjectId, RootId, TaskId};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const MAX_RETIREMENT_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_RETIREMENT_ITEMS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AssetId(String);

impl AssetId {
    pub fn new(value: impl Into<String>) -> Result<Self, RetirementError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || value == "."
            || value == ".."
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        {
            return Err(RetirementError::InvalidIdentifier);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for AssetId {
    type Error = RetirementError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<AssetId> for String {
    fn from(value: AssetId) -> Self {
        value.0
    }
}

/// Canonical decimal representation for externally serialized u64 values.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DecimalU64(String);

impl DecimalU64 {
    pub fn new(value: u64) -> Self {
        Self(value.to_string())
    }
    pub fn parse(&self) -> Result<u64, RetirementError> {
        self.0
            .parse::<u64>()
            .ok()
            .filter(|value| value.to_string() == self.0)
            .ok_or(RetirementError::InvalidCounter)
    }
    fn checked_next(&self) -> Result<Self, RetirementError> {
        self.parse()?
            .checked_add(1)
            .map(Self::new)
            .ok_or(RetirementError::CounterOverflow)
    }
}
impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        let parsed = value.parse::<u64>().map_err(serde::de::Error::custom)?;
        if parsed.to_string() != value {
            return Err(serde::de::Error::custom(
                "counter must be canonical decimal",
            ));
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementScope {
    pub project: ProjectId,
    pub root: RootId,
    pub task: TaskId,
    pub asset_id: AssetId,
    pub owner: PrincipalId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetIdentity {
    pub device: DecimalU64,
    pub inode: DecimalU64,
    pub registration_digest: String,
    pub custody_generation: DecimalU64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetObservation {
    pub registered: Option<bool>,
    pub identity: Option<AssetIdentity>,
    pub pins_revision: Option<DecimalU64>,
    pub active_pins: Option<DecimalU64>,
    pub dirty: Option<bool>,
    pub ignored_private: Option<bool>,
    pub unpushed: Option<bool>,
    pub unmerged: Option<bool>,
    pub active_process: Option<bool>,
    pub unique_data: Option<bool>,
    pub backup_hold: Option<bool>,
    pub foreign: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetirementStage {
    Preview,
    Claimed,
    QuarantineProposed,
    DeleteProposed,
    Refused,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementClaim {
    pub identity: AssetIdentity,
    pub pins_revision: DecimalU64,
    pub active_pins: DecimalU64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementState {
    pub scope: RetirementScope,
    pub stage: RetirementStage,
    pub generation: DecimalU64,
    pub claim: Option<RetirementClaim>,
    pub last_command: Option<ReplayRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayRecord {
    pub key: String,
    pub command_digest: String,
    pub resulting_state: Box<RetirementStateSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementStateSnapshot {
    pub stage: RetirementStage,
    pub generation: DecimalU64,
    pub claim: Option<RetirementClaim>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RetirementCommand {
    Claim { observation: AssetObservation },
    Recheck { observation: AssetObservation },
    ProposeDelete { observation: AssetObservation },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementProposal {
    pub stage: RetirementStage,
    pub reasons: Vec<String>,
    pub deletion_authorized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RetirementError {
    #[error("invalid identifier")]
    InvalidIdentifier,
    #[error("invalid canonical decimal counter")]
    InvalidCounter,
    #[error("counter overflow")]
    CounterOverflow,
    #[error("invalid or oversized JSON input")]
    InvalidInput,
    #[error("unknown or missing evidence blocks retirement")]
    UnknownEvidence,
    #[error("preservation evidence blocks retirement")]
    Preserve,
    #[error("retirement state compare-and-swap failed")]
    CompareAndSwap,
    #[error("idempotency key collision")]
    IdempotencyCollision,
    #[error("command is invalid for current retirement stage")]
    InvalidStage,
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub fn parse_observation_json(bytes: &[u8]) -> Result<AssetObservation, RetirementError> {
    if bytes.len() > MAX_RETIREMENT_JSON_BYTES {
        return Err(RetirementError::InvalidInput);
    }
    serde_json::from_slice(bytes).map_err(|_| RetirementError::InvalidInput)
}

pub fn plan_retirement(observation: &AssetObservation) -> RetirementProposal {
    let reasons = blocking_reasons(observation);
    RetirementProposal {
        stage: if reasons.is_empty() {
            RetirementStage::Preview
        } else {
            RetirementStage::Refused
        },
        reasons,
        deletion_authorized: false,
    }
}

/// Apply one keyed command. `expected_generation` is an explicit CAS token;
/// an exact command replay returns the recorded result without advancing it.
pub fn reduce_retirement(
    state: &RetirementState,
    expected_generation: &DecimalU64,
    key: &str,
    command: &RetirementCommand,
) -> Result<RetirementState, RetirementError> {
    if key.is_empty() || key.len() > 128 {
        return Err(RetirementError::InvalidInput);
    }
    let command_bytes = serde_json::to_vec(command).map_err(|_| RetirementError::InvalidInput)?;
    if command_bytes.len() > MAX_RETIREMENT_JSON_BYTES {
        return Err(RetirementError::InvalidInput);
    }
    let command_digest =
        crate::content_digest(command).map_err(|_| RetirementError::InvalidInput)?;
    if let Some(previous) = &state.last_command
        && previous.key == key
    {
        if previous.command_digest != command_digest {
            return Err(RetirementError::IdempotencyCollision);
        }
        return Ok(RetirementState {
            scope: state.scope.clone(),
            stage: previous.resulting_state.stage,
            generation: previous.resulting_state.generation.clone(),
            claim: previous.resulting_state.claim.clone(),
            last_command: Some(previous.clone()),
        });
    }
    if state.generation != *expected_generation {
        return Err(RetirementError::CompareAndSwap);
    }
    let next_generation = state.generation.checked_next()?;
    let (stage, claim) = match command {
        RetirementCommand::Claim { observation } => {
            if state.stage != RetirementStage::Preview {
                return Err(RetirementError::InvalidStage);
            }
            ensure_clean(observation)?;
            (RetirementStage::Claimed, Some(claim_from(observation)?))
        }
        RetirementCommand::Recheck { observation } => {
            if state.stage != RetirementStage::Claimed {
                return Err(RetirementError::InvalidStage);
            }
            ensure_matches(state.claim.as_ref(), observation)?;
            ensure_clean(observation)?;
            (RetirementStage::QuarantineProposed, state.claim.clone())
        }
        RetirementCommand::ProposeDelete { observation } => {
            if state.stage != RetirementStage::QuarantineProposed {
                return Err(RetirementError::InvalidStage);
            }
            ensure_matches(state.claim.as_ref(), observation)?;
            ensure_clean(observation)?;
            (RetirementStage::DeleteProposed, state.claim.clone())
        }
    };
    let snapshot = RetirementStateSnapshot {
        stage,
        generation: next_generation.clone(),
        claim: claim.clone(),
    };
    Ok(RetirementState {
        scope: state.scope.clone(),
        stage,
        generation: next_generation,
        claim,
        last_command: Some(ReplayRecord {
            key: key.to_owned(),
            command_digest,
            resulting_state: Box::new(snapshot),
        }),
    })
}

fn claim_from(observation: &AssetObservation) -> Result<RetirementClaim, RetirementError> {
    let identity = observation
        .identity
        .clone()
        .ok_or(RetirementError::UnknownEvidence)?;
    if identity.registration_digest.is_empty() || identity.registration_digest.len() > 4096 {
        return Err(RetirementError::InvalidInput);
    }
    identity.device.parse()?;
    identity.inode.parse()?;
    identity.custody_generation.parse()?;
    Ok(RetirementClaim {
        identity,
        pins_revision: observation
            .pins_revision
            .clone()
            .ok_or(RetirementError::UnknownEvidence)?,
        active_pins: observation
            .active_pins
            .clone()
            .ok_or(RetirementError::UnknownEvidence)?,
    })
}

fn ensure_matches(
    claim: Option<&RetirementClaim>,
    observation: &AssetObservation,
) -> Result<(), RetirementError> {
    let claim = claim.ok_or(RetirementError::UnknownEvidence)?;
    let current = claim_from(observation)?;
    if current != *claim {
        return Err(RetirementError::Preserve);
    }
    Ok(())
}

fn ensure_clean(observation: &AssetObservation) -> Result<(), RetirementError> {
    let reasons = blocking_reasons(observation);
    if reasons.iter().any(|reason| reason.starts_with("unknown:")) {
        return Err(RetirementError::UnknownEvidence);
    }
    if !reasons.is_empty() {
        return Err(RetirementError::Preserve);
    }
    Ok(())
}

fn blocking_reasons(observation: &AssetObservation) -> Vec<String> {
    let mut reasons = Vec::new();
    for (name, fact) in [
        ("registered", observation.registered),
        ("dirty", observation.dirty),
        ("ignored_private", observation.ignored_private),
        ("unpushed", observation.unpushed),
        ("unmerged", observation.unmerged),
        ("active_process", observation.active_process),
        ("unique_data", observation.unique_data),
        ("backup_hold", observation.backup_hold),
        ("foreign", observation.foreign),
    ] {
        match fact {
            None => reasons.push(format!("unknown:{name}")),
            Some(false) if name == "registered" => reasons.push("unregistered_asset".into()),
            Some(true) if name == "registered" => {}
            Some(true) => reasons.push(format!("preserve:{name}")),
            Some(false) => {}
        }
    }
    if observation.identity.is_none() {
        reasons.push("unknown:identity".into());
    }
    if observation.pins_revision.is_none() {
        reasons.push("unknown:pins_revision".into());
    }
    match &observation.active_pins {
        None => reasons.push("unknown:active_pins".into()),
        Some(count) => match count.parse() {
            Ok(0) => {}
            Ok(_) => reasons.push("preserve:active_pins".into()),
            Err(_) => reasons.push("unknown:active_pins".into()),
        },
    }
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation() -> AssetObservation {
        AssetObservation {
            registered: Some(true),
            identity: Some(AssetIdentity {
                device: DecimalU64::new(1),
                inode: DecimalU64::new(2),
                registration_digest: "sha256:abc".into(),
                custody_generation: DecimalU64::new(3),
            }),
            pins_revision: Some(DecimalU64::new(4)),
            active_pins: Some(DecimalU64::new(0)),
            dirty: Some(false),
            ignored_private: Some(false),
            unpushed: Some(false),
            unmerged: Some(false),
            active_process: Some(false),
            unique_data: Some(false),
            backup_hold: Some(false),
            foreign: Some(false),
        }
    }
    fn state() -> RetirementState {
        RetirementState {
            scope: RetirementScope {
                project: crate::ProjectId::new("p").expect("valid project fixture"),
                root: crate::RootId::new("r").expect("valid root fixture"),
                task: crate::TaskId::new("t").expect("valid task fixture"),
                asset_id: AssetId::new("a").expect("valid asset fixture"),
                owner: PrincipalId::new("o").expect("valid principal fixture"),
            },
            stage: RetirementStage::Preview,
            generation: DecimalU64::new(0),
            claim: None,
            last_command: None,
        }
    }

    #[test]
    fn exact_clean_transition_and_delete_never_grants_authority() {
        let clean = observation();
        assert_eq!(plan_retirement(&clean).stage, RetirementStage::Preview);
        let s1 = reduce_retirement(
            &state(),
            &DecimalU64::new(0),
            "claim",
            &RetirementCommand::Claim {
                observation: clean.clone(),
            },
        )
        .expect("claim proposal");
        assert_eq!(s1.stage, RetirementStage::Claimed);
        let s2 = reduce_retirement(
            &s1,
            &s1.generation,
            "recheck",
            &RetirementCommand::Recheck {
                observation: clean.clone(),
            },
        )
        .expect("quarantine proposal");
        assert_eq!(s2.stage, RetirementStage::QuarantineProposed);
        let s3 = reduce_retirement(
            &s2,
            &s2.generation,
            "delete",
            &RetirementCommand::ProposeDelete { observation: clean },
        )
        .expect("delete proposal");
        assert_eq!(s3.stage, RetirementStage::DeleteProposed);
        assert!(!plan_retirement(&observation()).deletion_authorized);
    }

    #[test]
    fn unknown_identity_or_owner_blocks() {
        let mut unknown = observation();
        unknown.identity = None;
        assert!(
            plan_retirement(&unknown)
                .reasons
                .iter()
                .any(|r| r == "unknown:identity")
        );
        let mut unregistered = observation();
        unregistered.registered = None;
        assert_eq!(
            reduce_retirement(
                &state(),
                &DecimalU64::new(0),
                "x",
                &RetirementCommand::Claim {
                    observation: unregistered
                }
            ),
            Err(RetirementError::UnknownEvidence)
        );
    }

    #[test]
    fn preservation_facts_block_claims() {
        let mutations: [fn(&mut AssetObservation); 8] = [
            |o: &mut AssetObservation| o.active_process = Some(true),
            |o: &mut AssetObservation| o.ignored_private = Some(true),
            |o: &mut AssetObservation| o.unpushed = Some(true),
            |o: &mut AssetObservation| o.unmerged = Some(true),
            |o: &mut AssetObservation| o.dirty = Some(true),
            |o: &mut AssetObservation| o.backup_hold = Some(true),
            |o: &mut AssetObservation| o.foreign = Some(true),
            |o: &mut AssetObservation| o.unique_data = Some(true),
        ];
        for mutate in mutations {
            let mut o = observation();
            mutate(&mut o);
            assert_eq!(plan_retirement(&o).stage, RetirementStage::Refused);
        }
    }

    #[test]
    fn pin_race_and_identity_swap_fail_closed() {
        let clean = observation();
        let claimed = reduce_retirement(
            &state(),
            &DecimalU64::new(0),
            "claim",
            &RetirementCommand::Claim { observation: clean },
        )
        .expect("claim");
        let mut raced = observation();
        raced.pins_revision = Some(DecimalU64::new(5));
        assert_eq!(
            reduce_retirement(
                &claimed,
                &claimed.generation,
                "race",
                &RetirementCommand::Recheck { observation: raced }
            ),
            Err(RetirementError::Preserve)
        );
        let mut swapped = observation();
        swapped.identity.as_mut().expect("identity").inode = DecimalU64::new(9);
        assert_eq!(
            reduce_retirement(
                &claimed,
                &claimed.generation,
                "swap",
                &RetirementCommand::Recheck {
                    observation: swapped
                }
            ),
            Err(RetirementError::Preserve)
        );
    }

    #[test]
    fn cas_replay_collision_and_checked_generation() {
        let initial = state();
        let cmd = RetirementCommand::Claim {
            observation: observation(),
        };
        let next =
            reduce_retirement(&initial, &initial.generation, "k", &cmd).expect("first command");
        assert_eq!(
            reduce_retirement(&next, &DecimalU64::new(0), "k", &cmd).expect("exact replay"),
            next
        );
        assert_eq!(
            reduce_retirement(
                &next,
                &next.generation,
                "k",
                &RetirementCommand::Claim {
                    observation: AssetObservation {
                        registered: None,
                        ..observation()
                    }
                }
            ),
            Err(RetirementError::IdempotencyCollision)
        );
        assert_eq!(
            reduce_retirement(&next, &DecimalU64::new(0), "other", &cmd),
            Err(RetirementError::CompareAndSwap)
        );
        let mut overflow = initial;
        overflow.generation = DecimalU64("18446744073709551615".into());
        assert_eq!(
            reduce_retirement(&overflow, &overflow.generation, "overflow", &cmd),
            Err(RetirementError::CounterOverflow)
        );
    }

    #[test]
    fn external_json_is_bounded_and_rejects_unknown_fields() {
        assert_eq!(
            parse_observation_json(&vec![b' '; MAX_RETIREMENT_JSON_BYTES + 1]),
            Err(RetirementError::InvalidInput)
        );
        assert_eq!(
            parse_observation_json(br#"{"extra":true}"#),
            Err(RetirementError::InvalidInput)
        );
    }
}
