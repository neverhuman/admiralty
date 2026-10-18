//! Pure, fail-closed retirement proposals for registered assets.
//!
//! This module records observations and proposals only. It never inspects or
//! changes a filesystem, process, pin store, or backup.

use crate::{PrincipalId, ProjectId, RootId, TaskId};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};

pub const MAX_RETIREMENT_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_RETIREMENT_ITEMS: usize = 1024;
pub const MAX_REPLAY_RECORDS: usize = 8;

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
    pub scope: Option<RetirementScope>,
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
    #[serde(default)]
    pub replay_history: BTreeMap<String, ReplayRecord>,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementProposal {
    pub stage: RetirementStage,
    pub reasons: Vec<String>,
    deletion_authorized: bool,
}
impl RetirementProposal {
    pub fn deletion_authorized(&self) -> bool {
        self.deletion_authorized
    }
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
    let reasons =
        if crate::limits::bounded_json_size(observation, MAX_RETIREMENT_JSON_BYTES).is_err() {
            vec!["unknown:input_limit".into()]
        } else {
            blocking_reasons(observation)
        };
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
/// an exact command replay preserves the current projection without advancing
/// or rewinding it. All retained command identities remain collision checked.
pub fn reduce_retirement(
    state: &RetirementState,
    expected_generation: &DecimalU64,
    key: &str,
    command: &RetirementCommand,
) -> Result<RetirementState, RetirementError> {
    if key.is_empty() || key.len() > 128 {
        return Err(RetirementError::InvalidInput);
    }
    crate::limits::bounded_json_size(
        &(state, expected_generation, key, command),
        MAX_RETIREMENT_JSON_BYTES,
    )
    .map_err(|_| RetirementError::InvalidInput)?;
    let command_digest = crate::content_digest(&("admiralty-retirement-v1", &state.scope, command))
        .map_err(|_| RetirementError::InvalidInput)?;
    if state
        .replay_history
        .iter()
        .any(|(key, record)| key != &record.key || !valid_digest(&record.command_digest))
    {
        return Err(RetirementError::InvalidInput);
    }
    let mut replay_history = state.replay_history.clone();
    if let Some(previous) = &state.last_command {
        if !valid_digest(&previous.command_digest)
            || replay_history
                .get(&previous.key)
                .is_some_and(|entry| entry != previous)
        {
            return Err(RetirementError::InvalidInput);
        }
        replay_history.insert(previous.key.clone(), previous.clone());
    }
    if let Some(previous) = replay_history.get(key) {
        if previous.command_digest != command_digest {
            return Err(RetirementError::IdempotencyCollision);
        }
        return Ok(state.clone());
    }
    if replay_history.len() >= MAX_REPLAY_RECORDS {
        return Err(RetirementError::InvalidInput);
    }
    if state.generation != *expected_generation {
        return Err(RetirementError::CompareAndSwap);
    }
    let next_generation = state.generation.checked_next()?;
    let observation = match command {
        RetirementCommand::Claim { observation }
        | RetirementCommand::Recheck { observation }
        | RetirementCommand::ProposeDelete { observation } => observation,
    };
    if observation.scope.as_ref() != Some(&state.scope) {
        return Err(RetirementError::Preserve);
    }
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
    let record = ReplayRecord {
        key: key.to_owned(),
        command_digest,
        resulting_state: Box::new(snapshot),
    };
    replay_history.insert(key.to_owned(), record.clone());
    Ok(RetirementState {
        scope: state.scope.clone(),
        stage,
        generation: next_generation,
        claim,
        last_command: Some(record),
        replay_history,
    })
}

fn claim_from(observation: &AssetObservation) -> Result<RetirementClaim, RetirementError> {
    let identity = observation
        .identity
        .clone()
        .ok_or(RetirementError::UnknownEvidence)?;
    if !valid_digest(&identity.registration_digest) {
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
    if observation.scope.is_none() {
        reasons.push("unknown:scope".into());
    }
    if observation
        .identity
        .as_ref()
        .is_some_and(|identity| !valid_digest(&identity.registration_digest))
    {
        reasons.push("unknown:registration_digest".into());
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

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation() -> AssetObservation {
        AssetObservation {
            scope: Some(state().scope),
            registered: Some(true),
            identity: Some(AssetIdentity {
                device: DecimalU64::new(1),
                inode: DecimalU64::new(2),
                registration_digest: format!("sha256:{}", "a".repeat(64)),
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
            replay_history: BTreeMap::new(),
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
    #[test]
    fn earlier_replay_never_rewinds_and_legacy_last_key_is_retained() {
        let initial = state();
        let claim = RetirementCommand::Claim {
            observation: observation(),
        };
        let mut first =
            reduce_retirement(&initial, &initial.generation, "claim", &claim).expect("claim");
        // Migrate a legacy projection that retained only last_command.
        first.replay_history.clear();
        let second = reduce_retirement(
            &first,
            &first.generation,
            "recheck",
            &RetirementCommand::Recheck {
                observation: observation(),
            },
        )
        .expect("recheck");
        assert_eq!(second.replay_history.len(), 2);
        assert_eq!(
            reduce_retirement(&second, &initial.generation, "claim", &claim).expect("old replay"),
            second
        );
        let altered = RetirementCommand::Claim {
            observation: AssetObservation {
                dirty: Some(true),
                ..observation()
            },
        };
        assert_eq!(
            reduce_retirement(&second, &second.generation, "claim", &altered),
            Err(RetirementError::IdempotencyCollision)
        );
    }
    #[test]
    fn foreign_scope_invalid_digest_and_typed_oversize_fail_closed() {
        let initial = state();
        let mut foreign = observation();
        foreign.scope.as_mut().expect("scope").root = RootId::new("other").expect("root");
        assert_eq!(
            reduce_retirement(
                &initial,
                &initial.generation,
                "foreign",
                &RetirementCommand::Claim {
                    observation: foreign
                }
            ),
            Err(RetirementError::Preserve)
        );
        let mut invalid = observation();
        invalid
            .identity
            .as_mut()
            .expect("identity")
            .registration_digest = "sha256:short".into();
        assert_eq!(plan_retirement(&invalid).stage, RetirementStage::Refused);
        invalid
            .identity
            .as_mut()
            .expect("identity")
            .registration_digest = "x".repeat(MAX_RETIREMENT_JSON_BYTES);
        assert_eq!(plan_retirement(&invalid).stage, RetirementStage::Refused);
        assert_eq!(
            reduce_retirement(
                &initial,
                &initial.generation,
                "huge",
                &RetirementCommand::Claim {
                    observation: invalid
                }
            ),
            Err(RetirementError::InvalidInput)
        );
    }
}
