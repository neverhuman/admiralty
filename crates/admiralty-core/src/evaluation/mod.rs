//! Frozen experiment inputs and reproducible decision datasets.
use crate::{PrincipalId, RootId, content_digest, trials::*};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub id: TrialId,
    pub role_prompts: BTreeMap<String, String>,
    pub policy_digest: String,
    pub detector_digest: String,
    pub evaluator_digest: String,
    pub authority_digest: String,
    pub learned_routing: bool,
}

pub fn bundle_digest(bundle: &Bundle) -> Result<String, TrialError> {
    if bundle.role_prompts.is_empty()
        || bundle.role_prompts.len() > 32
        || bundle.role_prompts.iter().any(|(role, prompt)| {
            role.trim().is_empty() || role.len() > 128 || prompt.trim().is_empty()
        })
    {
        return Err(TrialError::Invalid(
            "bundle requires bounded role prompts".into(),
        ));
    }
    for digest in [
        &bundle.policy_digest,
        &bundle.detector_digest,
        &bundle.evaluator_digest,
        &bundle.authority_digest,
    ] {
        validate_digest(digest)?;
    }
    crate::limits::bounded_json_size(bundle, 65_536)
        .map_err(|_| TrialError::TooLarge("bundle exceeds 64 KiB".into()))?;
    content_digest(bundle).map_err(|e| TrialError::Invalid(e.to_string()))
}

pub fn validate_digest(digest: &str) -> Result<(), TrialError> {
    if digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    }) {
        Ok(())
    } else {
        Err(TrialError::Invalid(
            "full lowercase SHA-256 digest required".into(),
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrialMode {
    Replay,
    Fresh,
    HeldOut,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialRegistration {
    pub spec: TrialSpec,
    pub owner: PrincipalId,
    pub mode: TrialMode,
    /// The first two preregistered arms map to champion and candidate respectively.
    pub champion: String,
    pub candidate: String,
    pub managers: BTreeMap<RootId, PrincipalId>,
    pub features: Vec<FeatureManifest>,
    pub max_feature_age_ticks: DecimalU64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecord {
    pub root_id: RootId,
    pub bundle_digest: String,
    /// Digests reference actual retained execution and provider usage artifacts.
    pub execution_digest: String,
    pub usage_digest: Option<String>,
    pub execution_cost_micros: Option<DecimalU64>,
    pub training_cost_micros: Option<DecimalU64>,
    pub analysis_cost_micros: Option<DecimalU64>,
}

impl ExecutionRecord {
    pub fn cost(&self) -> Option<u64> {
        self.execution_cost_micros
            .as_ref()?
            .get()
            .checked_add(self.training_cost_micros.as_ref()?.get())?
            .checked_add(self.analysis_cost_micros.as_ref()?.get())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Accept,
    Reject,
    InsufficientEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DecisionRecord {
    pub reviewer: PrincipalId,
    pub dossier_digest: String,
    pub verdict: Verdict,
    pub alias_revision: u64,
    pub rolled_back: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TrialDataset {
    pub schema_version: u32,
    pub registration: TrialRegistration,
    pub bundles: Vec<Bundle>,
    pub evidence: TrialEvidence,
    pub executions: Vec<ExecutionRecord>,
    pub all_in_cost_micros: Option<DecimalU64>,
    pub proposal: TrialDecision,
    pub ineligibility: Vec<String>,
    pub decision: Option<DecisionRecord>,
    pub rollback_review: Option<DecisionRecord>,
}

impl TrialDataset {
    /// Decision records do not change the immutable dossier being reviewed.
    pub fn dossier_digest(&self) -> Result<String, TrialError> {
        content_digest(&(
            self.schema_version,
            &self.registration,
            &self.bundles,
            &self.evidence,
            &self.executions,
            &self.all_in_cost_micros,
            &self.proposal,
            &self.ineligibility,
        ))
        .map_err(|e| TrialError::Invalid(e.to_string()))
    }
}
