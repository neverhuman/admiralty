//! Pure, replayable trial assignment and decision proposals.
//!
//! This module never activates a bundle or changes evaluator authority. It
//! records a fixed cohort and returns a proposal for independent review.
use crate::{PrincipalId, ProjectId, RootId, TaskId, content_digest, limits::bounded_json_size};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const MAX_JSON_BYTES: usize = 1_048_576;
const MAX_COHORT: usize = 1024;
const MAX_FEATURES: usize = 256;
const MAX_STR: usize = 4096;

/// Canonical decimal representation for externally serialized counters/ticks.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DecimalU64(u64);

impl DecimalU64 {
    pub fn new(value: u64) -> Self {
        Self(value)
    }
    pub fn get(&self) -> u64 {
        self.0
    }
}
impl TryFrom<String> for DecimalU64 {
    type Error = TrialError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(TrialError::Invalid(
                "counter must be a canonical decimal string".into(),
            ));
        }
        value
            .parse::<u64>()
            .map(Self)
            .map_err(|_| TrialError::Invalid("counter exceeds u64".into()))
    }
}
impl From<DecimalU64> for String {
    fn from(value: DecimalU64) -> Self {
        value.0.to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialSpec {
    pub trial_id: TrialId,
    pub project_id: ProjectId,
    pub scope_root_id: RootId,
    pub scope_task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub fixed_cohort: Vec<ClusterMember>,
    pub immutable_bundle_digests: Vec<String>,
    pub protected_oracle_digest: String,
    pub pre_registered_seed: String,
    pub decision_tick: DecimalU64,
    pub cluster_assignments: Vec<ClusterAssignment>,
    pub required_maturity: DecimalU64,
    pub planned_looks: DecimalU64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TrialId(String);
impl TrialId {
    pub fn new(value: impl Into<String>) -> Result<Self, TrialError> {
        let s = value.into();
        validate_identifier(&s)?;
        Ok(Self(s))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for TrialId {
    type Error = TrialError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}
impl From<TrialId> for String {
    fn from(id: TrialId) -> Self {
        id.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterAssignment {
    pub cluster_id: ClusterId,
    pub variants: Vec<VariantId>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterMember {
    pub root_id: RootId,
    pub cluster_id: ClusterId,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ClusterId(String);
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct VariantId(String);
macro_rules! local_id {
    ($ty:ty) => {
        impl $ty {
            pub fn new(v: impl Into<String>) -> Result<Self, TrialError> {
                let s = v.into();
                validate_identifier(&s)?;
                Ok(Self(s))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $ty {
            type Error = TrialError;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                Self::new(s)
            }
        }
        impl From<$ty> for String {
            fn from(v: $ty) -> Self {
                v.0
            }
        }
    };
}
local_id!(ClusterId);
local_id!(VariantId);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureManifest {
    pub feature_id: FeatureId,
    pub digest: String,
    pub available_tick: DecimalU64,
    pub decision_phase: DecisionPhase,
    pub missingness: Missingness,
    pub extraction_cost_micros: Option<DecimalU64>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FeatureId(String);
local_id!(FeatureId);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionPhase {
    InitialAssignment,
    Continuation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Missingness {
    Known,
    Unknown,
    Missing { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub root_id: RootId,
    pub cluster_id: ClusterId,
    pub variant: VariantId,
    pub propensity_numerator: DecimalU64,
    pub propensity_denominator: DecimalU64,
    pub unit_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub root_id: RootId,
    pub label: OutcomeLabel,
    pub observed_tick: DecimalU64,
    pub heldout_contaminated: bool,
    pub rework: bool,
    pub analysis_value_micros: Option<DecimalU64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeLabel {
    Mature,
    Failed,
    Cancelled,
    Immature,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialEvidence {
    pub trial: TrialSpec,
    pub bundle_digests_at_analysis: Vec<String>,
    pub protected_oracle_digest_at_analysis: String,
    pub features: Vec<FeatureManifest>,
    pub assignments: Vec<Assignment>,
    pub outcomes: Vec<Outcome>,
    pub look_count: DecimalU64,
    pub analysis_tick: DecimalU64,
    pub independent_analysis: Option<AnalysisReceipt>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalysisReceipt {
    pub receipt_digest: String,
    pub analyst: PrincipalId,
    pub analyzed_evidence_digest: String,
    pub recommendation: AnalysisRecommendation,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisRecommendation {
    RecommendIndependentReview,
    Reject,
    InsufficientEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum TrialDecision {
    PromotableProposal {
        reasons: Vec<String>,
        promotion_authorized: bool,
        rollback: RollbackProposal,
    },
    RejectProposal {
        reasons: Vec<String>,
        promotion_authorized: bool,
    },
    InsufficientEvidence {
        reasons: Vec<String>,
        promotion_authorized: bool,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackProposal {
    pub champion_bundle_digests: Vec<String>,
    pub applies_to_new_work_only: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TrialError {
    #[error("invalid trial input: {0}")]
    Invalid(String),
    #[error("duplicate identity or conflicting bytes: {0}")]
    Collision(String),
    #[error("trial input exceeds bound: {0}")]
    TooLarge(String),
}

fn validate_identifier(s: &str) -> Result<(), TrialError> {
    if s.is_empty()
        || s.len() > 128
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        || s == "."
        || s == ".."
    {
        Err(TrialError::Invalid("invalid identifier".into()))
    } else {
        Ok(())
    }
}
fn check_text(s: &str, name: &str) -> Result<(), TrialError> {
    if s.is_empty() || s.len() > MAX_STR {
        Err(TrialError::Invalid(format!("{name} is empty or too long")))
    } else {
        Ok(())
    }
}
fn check_json<T: Serialize>(v: &T) -> Result<(), TrialError> {
    bounded_json_size(v, MAX_JSON_BYTES)
        .map(|_| ())
        .map_err(|_| TrialError::TooLarge("invalid JSON or exceeds 1 MiB".into()))
}
fn check_digest(value: &str) -> Result<(), TrialError> {
    if value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        Ok(())
    } else {
        Err(TrialError::Invalid(
            "full lowercase SHA-256 digest required".into(),
        ))
    }
}
fn unit_id(spec: &TrialSpec, member: &ClusterMember) -> Result<String, TrialError> {
    content_digest(&(spec, member))
        .map(|digest| format!("trial-unit-{}", &digest[7..]))
        .map_err(|err| TrialError::Invalid(err.to_string()))
}
pub fn trial_evidence_digest(e: &TrialEvidence) -> Result<String, TrialError> {
    check_json(e)?;
    content_digest(&(
        &e.trial,
        &e.assignments,
        &e.outcomes,
        &e.features,
        &e.look_count,
        &e.analysis_tick,
    ))
    .map_err(|err| TrialError::Invalid(err.to_string()))
}

/// Assign roots by cluster using the preregistered seed. Input order cannot
/// affect output; all roots in one cluster receive the same arm.
pub fn assign_trial(
    spec: &TrialSpec,
    members: &[ClusterMember],
    numerator: u64,
    denominator: u64,
) -> Result<Vec<Assignment>, TrialError> {
    validate_spec(spec)?;
    if members.is_empty() || members.len() > MAX_COHORT {
        return Err(TrialError::TooLarge(
            "cohort must contain 1..1024 roots".into(),
        ));
    }
    check_json(&(spec, members))?;
    let mut roots = BTreeSet::new();
    for member in members {
        if !roots.insert(member.root_id.clone()) {
            return Err(TrialError::Collision("duplicate root".into()));
        }
    }
    let actual: BTreeMap<_, _> = members
        .iter()
        .map(|m| (&m.root_id, &m.cluster_id))
        .collect();
    let expected: BTreeMap<_, _> = spec
        .fixed_cohort
        .iter()
        .map(|m| (&m.root_id, &m.cluster_id))
        .collect();
    if actual != expected {
        return Err(TrialError::Collision(
            "assignment members differ from preregistered fixed cohort".into(),
        ));
    }
    let variants = &spec.cluster_assignments[0].variants;
    if numerator != 1 || denominator != variants.len() as u64 {
        return Err(TrialError::Invalid(
            "propensity must match balanced uniform arm allocation".into(),
        ));
    }
    let mut ranked = Vec::new();
    for cluster in &spec.cluster_assignments {
        let digest = content_digest(&(
            &spec.pre_registered_seed,
            &spec.project_id,
            &spec.scope_root_id,
            &spec.scope_task_id,
            &spec.trial_id,
            &cluster.cluster_id,
        ))
        .map_err(|err| TrialError::Invalid(err.to_string()))?;
        ranked.push((digest, cluster.cluster_id.clone()));
    }
    ranked.sort();
    let offset_digest =
        content_digest(&(&spec.pre_registered_seed, &spec.trial_id, "balanced-offset"))
            .map_err(|err| TrialError::Invalid(err.to_string()))?;
    let offset = u64::from_str_radix(&offset_digest[7..23], 16)
        .map_err(|_| TrialError::Invalid("invalid seed digest".into()))?
        % variants.len() as u64;
    let cluster_arms: BTreeMap<_, _> = ranked
        .into_iter()
        .enumerate()
        .map(|(index, (_, cluster))| {
            (
                cluster,
                variants[(index + offset as usize) % variants.len()].clone(),
            )
        })
        .collect();
    let mut out = Vec::new();
    for member in members {
        out.push(Assignment {
            root_id: member.root_id.clone(),
            cluster_id: member.cluster_id.clone(),
            variant: cluster_arms
                .get(&member.cluster_id)
                .ok_or_else(|| TrialError::Invalid("undeclared cluster".into()))?
                .clone(),
            propensity_numerator: DecimalU64::new(numerator),
            propensity_denominator: DecimalU64::new(denominator),
            unit_id: unit_id(spec, member)?,
        });
    }
    out.sort_by(|a, b| a.root_id.cmp(&b.root_id));
    Ok(out)
}

pub fn validate_trial(e: &TrialEvidence) -> Result<(), TrialError> {
    check_json(e)?;
    validate_spec(&e.trial)?;
    if e.assignments.len() > MAX_COHORT || e.outcomes.len() > MAX_COHORT {
        return Err(TrialError::TooLarge("cohort exceeds 1024".into()));
    }
    if e.features.len() > MAX_FEATURES {
        return Err(TrialError::TooLarge("features exceed 256".into()));
    }
    if e.trial.immutable_bundle_digests != e.bundle_digests_at_analysis {
        return Err(TrialError::Invalid("immutable bundle drift".into()));
    }
    if e.trial.protected_oracle_digest != e.protected_oracle_digest_at_analysis {
        return Err(TrialError::Invalid("protected oracle drift".into()));
    }
    let mut roots = BTreeSet::new();
    let mut units = BTreeSet::new();
    let mut cluster_arms = BTreeMap::new();
    let declared_clusters: BTreeMap<_, _> = e
        .trial
        .cluster_assignments
        .iter()
        .map(|c| (&c.cluster_id, &c.variants))
        .collect();
    for a in &e.assignments {
        if a.propensity_denominator.get() == 0
            || a.propensity_numerator.get() == 0
            || a.propensity_numerator.get() > a.propensity_denominator.get()
        {
            return Err(TrialError::Invalid("invalid propensity".into()));
        }
        if !roots.insert(a.root_id.clone()) || !units.insert(a.unit_id.clone()) {
            return Err(TrialError::Collision("duplicate root/unit identity".into()));
        }
        let expected = unit_id(
            &e.trial,
            &ClusterMember {
                root_id: a.root_id.clone(),
                cluster_id: a.cluster_id.clone(),
            },
        )?;
        if a.unit_id != expected {
            return Err(TrialError::Collision(
                "unit id does not match trial scope".into(),
            ));
        }
        if !declared_clusters
            .get(&a.cluster_id)
            .is_some_and(|variants| variants.contains(&a.variant))
        {
            return Err(TrialError::Invalid(
                "assignment does not match declared cluster variants".into(),
            ));
        }
        let variant_count = declared_clusters
            .get(&a.cluster_id)
            .map_or(0, |variants| variants.len()) as u64;
        if a.propensity_numerator.get() != 1 || a.propensity_denominator.get() != variant_count {
            return Err(TrialError::Invalid(
                "propensity does not match preregistered uniform draw".into(),
            ));
        }
        if let Some(prev) = cluster_arms.insert(a.cluster_id.clone(), a.variant.clone())
            && prev != a.variant
        {
            return Err(TrialError::Collision(
                "cluster assigned to multiple arms".into(),
            ));
        }
    }
    let expected_assignments = assign_trial(
        &e.trial,
        &e.trial.fixed_cohort,
        1,
        e.trial.cluster_assignments[0].variants.len() as u64,
    )?;
    let mut actual_assignments = e.assignments.clone();
    actual_assignments.sort_by(|a, b| a.root_id.cmp(&b.root_id));
    if actual_assignments != expected_assignments {
        return Err(TrialError::Collision(
            "assignments differ from recorded seed/fixed cohort".into(),
        ));
    }
    if e.analysis_tick.get() < e.trial.decision_tick.get() {
        return Err(TrialError::Invalid("analysis precedes assignment".into()));
    }
    let mut outcome_roots = BTreeSet::new();
    for o in &e.outcomes {
        if o.observed_tick.get() <= e.trial.decision_tick.get()
            || o.observed_tick.get() > e.analysis_tick.get()
        {
            return Err(TrialError::Invalid(
                "outcome outside recorded assignment/analysis clock interval".into(),
            ));
        }
        if !outcome_roots.insert(o.root_id.clone()) {
            return Err(TrialError::Collision("duplicate outcome".into()));
        }
        if !roots.contains(&o.root_id) {
            return Err(TrialError::Invalid("outcome outside fixed cohort".into()));
        }
    }
    let mut feature_ids = BTreeSet::new();
    for f in &e.features {
        check_digest(&f.digest)?;
        if !feature_ids.insert(f.feature_id.clone()) {
            return Err(TrialError::Collision("duplicate feature id".into()));
        }
        if f.available_tick.get() > e.trial.decision_tick.get() {
            return Err(TrialError::Invalid(
                "future feature leaks into decision".into(),
            ));
        }
        if f.decision_phase == DecisionPhase::Continuation {
            return Err(TrialError::Invalid(
                "continuation feature leaks into initial assignment".into(),
            ));
        }
        if let Missingness::Missing { reason } = &f.missingness {
            check_text(reason, "missingness reason")?;
        }
    }
    Ok(())
}

fn validate_spec(spec: &TrialSpec) -> Result<(), TrialError> {
    check_json(spec)?;
    check_text(&spec.pre_registered_seed, "preregistered seed")?;
    check_digest(&spec.protected_oracle_digest)?;
    check_digest(&spec.contract_digest)?;
    if spec.contract_revision == 0
        || spec.fixed_cohort.is_empty()
        || spec.fixed_cohort.len() > MAX_COHORT
        || spec.cluster_assignments.is_empty()
        || spec.cluster_assignments.len() > MAX_COHORT
        || spec.planned_looks.get() == 0
        || spec.planned_looks.get() > 16
        || spec.required_maturity.get() == 0
        || spec.required_maturity.get() > spec.fixed_cohort.len() as u64
    {
        return Err(TrialError::Invalid(
            "invalid fixed cohort, revision or preregistered budget".into(),
        ));
    }
    if spec.immutable_bundle_digests.is_empty() || spec.immutable_bundle_digests.len() > 4 {
        return Err(TrialError::Invalid(
            "at least one immutable bundle digest is required".into(),
        ));
    }
    for digest in &spec.immutable_bundle_digests {
        check_digest(digest)?;
    }
    if spec
        .immutable_bundle_digests
        .iter()
        .collect::<BTreeSet<_>>()
        .len()
        != spec.immutable_bundle_digests.len()
    {
        return Err(TrialError::Collision("duplicate bundle".into()));
    }
    let common = &spec.cluster_assignments[0].variants;
    if !(2..=4).contains(&common.len())
        || common.iter().collect::<BTreeSet<_>>().len() != common.len()
    {
        return Err(TrialError::Invalid(
            "two to four distinct common trial arms required".into(),
        ));
    }
    let cluster_ids: BTreeSet<_> = spec
        .cluster_assignments
        .iter()
        .map(|cluster| &cluster.cluster_id)
        .collect();
    if cluster_ids.len() != spec.cluster_assignments.len()
        || spec
            .cluster_assignments
            .iter()
            .any(|cluster| &cluster.variants != common)
    {
        return Err(TrialError::Invalid(
            "clusters require the same preregistered arm ordering".into(),
        ));
    }
    let roots: BTreeSet<_> = spec
        .fixed_cohort
        .iter()
        .map(|member| &member.root_id)
        .collect();
    let member_clusters: BTreeSet<_> = spec
        .fixed_cohort
        .iter()
        .map(|member| &member.cluster_id)
        .collect();
    if roots.len() != spec.fixed_cohort.len() || member_clusters != cluster_ids {
        return Err(TrialError::Collision(
            "fixed cohort roots/clusters are not unique and complete".into(),
        ));
    }
    if spec.cluster_assignments.len() > MAX_COHORT {
        return Err(TrialError::TooLarge("clusters exceed 1024".into()));
    }
    let mut clusters = BTreeSet::new();
    for cluster in &spec.cluster_assignments {
        if !clusters.insert(&cluster.cluster_id) {
            return Err(TrialError::Collision("duplicate cluster assignment".into()));
        }
        if cluster.variants.is_empty() {
            return Err(TrialError::Invalid("cluster has no variants".into()));
        }
        if cluster.variants.iter().collect::<BTreeSet<_>>().len() != cluster.variants.len() {
            return Err(TrialError::Collision("duplicate variant in cluster".into()));
        }
    }
    Ok(())
}

/// Produce only a review proposal. No outcome values are interpreted as a
/// statistically supported benefit by this deterministic component.
pub fn propose_trial_decision(
    e: &TrialEvidence,
    champion_bundle_digests: Vec<String>,
) -> Result<TrialDecision, TrialError> {
    validate_trial(e)?;
    let mut reasons = Vec::new();
    if e.look_count.get() > e.trial.planned_looks.get() {
        reasons.push("unplanned repeated look".into());
    }
    if e.look_count.get() == 0 {
        reasons.push("no recorded analysis look".into());
    }
    if e.assignments.iter().any(|a| {
        a.propensity_denominator.get() == 0
            || a.propensity_numerator.get() == 0
            || a.propensity_numerator.get() > a.propensity_denominator.get()
    }) {
        reasons.push("invalid propensity".into());
    }
    if e.outcomes.iter().any(|o| o.heldout_contaminated) {
        reasons.push("held-out contamination".into());
    }
    if e.outcomes
        .iter()
        .any(|o| o.label == OutcomeLabel::Immature || o.analysis_value_micros.is_none())
        || e.features
            .iter()
            .any(|feature| feature.extraction_cost_micros.is_none())
    {
        reasons.push("immature labels or unknown all-in analysis costs".into());
    }
    if e.assignments
        .iter()
        .map(|assignment| &assignment.variant)
        .collect::<BTreeSet<_>>()
        .len()
        < 2
    {
        reasons.push("no independent control/candidate arm coverage".into());
    }
    let maturity = e
        .outcomes
        .iter()
        .filter(|o| {
            matches!(
                o.label,
                OutcomeLabel::Mature | OutcomeLabel::Failed | OutcomeLabel::Cancelled
            )
        })
        .count() as u64;
    if maturity < e.trial.required_maturity.get() {
        reasons.push("fixed cohort has immature or missing outcomes".into());
    }
    // The assignment list is the fixed cohort: failed, cancelled and rework
    // roots remain present regardless of whether their outcome matured.
    if e.assignments
        .iter()
        .any(|a| !e.outcomes.iter().any(|o| o.root_id == a.root_id))
    {
        reasons.push("fixed cohort outcome missing".into());
    }
    let Some(receipt) = &e.independent_analysis else {
        return Ok(TrialDecision::InsufficientEvidence {
            reasons: vec!["independent analysis receipt required".into()],
            promotion_authorized: false,
        });
    };
    check_digest(&receipt.receipt_digest)?;
    let digest = trial_evidence_digest(e)?;
    if receipt.analyzed_evidence_digest != digest {
        return Ok(TrialDecision::InsufficientEvidence {
            reasons: vec!["analysis receipt does not bind evidence".into()],
            promotion_authorized: false,
        });
    }
    if !reasons.is_empty() {
        return Ok(TrialDecision::InsufficientEvidence {
            reasons,
            promotion_authorized: false,
        });
    }
    match receipt.recommendation {
        AnalysisRecommendation::Reject => {
            return Ok(TrialDecision::RejectProposal {
                reasons: vec![
                    "independent analysis recommends rejection; external review required".into(),
                ],
                promotion_authorized: false,
            });
        }
        AnalysisRecommendation::InsufficientEvidence => {
            return Ok(TrialDecision::InsufficientEvidence {
                reasons: vec!["independent analysis reports insufficient evidence".into()],
                promotion_authorized: false,
            });
        }
        AnalysisRecommendation::RecommendIndependentReview => {}
    }
    if champion_bundle_digests.is_empty()
        || champion_bundle_digests.len() > 4
        || champion_bundle_digests.iter().any(|digest| {
            check_digest(digest).is_err() || !e.trial.immutable_bundle_digests.contains(digest)
        })
    {
        return Err(TrialError::Invalid(
            "rollback must name a recorded immutable trial bundle".into(),
        ));
    }
    Ok(TrialDecision::PromotableProposal {
        reasons: vec!["independent analysis supplied; external review required".into()],
        promotion_authorized: false,
        rollback: RollbackProposal {
            champion_bundle_digests,
            applies_to_new_work_only: true,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn trial() -> TrialSpec {
        TrialSpec {
            trial_id: TrialId::new("trial-1").expect("valid trial id"),
            project_id: ProjectId::new("p").expect("valid project id"),
            scope_root_id: RootId::new("scope").expect("valid scope root"),
            scope_task_id: TaskId::new("task").expect("valid task id"),
            contract_revision: 1,
            contract_digest: format!("sha256:{}", "c".repeat(64)),
            fixed_cohort: members(),
            immutable_bundle_digests: vec![format!("sha256:{}", "a".repeat(64))],
            protected_oracle_digest: format!("sha256:{}", "b".repeat(64)),
            pre_registered_seed: "seed-9".into(),
            decision_tick: DecimalU64::new(50),
            cluster_assignments: vec![
                ClusterAssignment {
                    cluster_id: ClusterId::new("cluster-a").expect("valid cluster"),
                    variants: vec![
                        VariantId::new("control").expect("valid variant"),
                        VariantId::new("candidate").expect("valid variant"),
                    ],
                },
                ClusterAssignment {
                    cluster_id: ClusterId::new("cluster-b").expect("valid cluster"),
                    variants: vec![
                        VariantId::new("control").expect("valid variant"),
                        VariantId::new("candidate").expect("valid variant"),
                    ],
                },
            ],
            required_maturity: DecimalU64::new(2),
            planned_looks: DecimalU64::new(1),
        }
    }
    fn roots() -> Vec<RootId> {
        ["r1", "r2", "r3", "r4"]
            .into_iter()
            .map(|s| RootId::new(s).expect("valid root id"))
            .collect()
    }
    fn members() -> Vec<ClusterMember> {
        roots()
            .into_iter()
            .enumerate()
            .map(|(i, root_id)| ClusterMember {
                root_id,
                cluster_id: ClusterId::new(if i < 2 { "cluster-a" } else { "cluster-b" })
                    .expect("valid cluster"),
            })
            .collect()
    }
    fn evidence() -> TrialEvidence {
        let t = trial();
        let assignments = assign_trial(&t, &members(), 1, 2).expect("valid assignments");
        let outcomes = assignments
            .iter()
            .map(|a| Outcome {
                root_id: a.root_id.clone(),
                label: OutcomeLabel::Mature,
                observed_tick: DecimalU64::new(55),
                heldout_contaminated: false,
                rework: false,
                analysis_value_micros: Some(DecimalU64::new(1)),
            })
            .collect();
        TrialEvidence {
            trial: t.clone(),
            bundle_digests_at_analysis: t.immutable_bundle_digests.clone(),
            protected_oracle_digest_at_analysis: t.protected_oracle_digest.clone(),
            features: vec![],
            assignments,
            outcomes,
            look_count: DecimalU64::new(1),
            analysis_tick: DecimalU64::new(60),
            independent_analysis: None,
        }
    }
    fn bind_receipt(e: &mut TrialEvidence) {
        let d = trial_evidence_digest(e).expect("evidence digest");
        e.independent_analysis = Some(AnalysisReceipt {
            receipt_digest: format!("sha256:{}", "d".repeat(64)),
            analyst: PrincipalId::new("independent-reviewer").expect("fixture analyst"),
            analyzed_evidence_digest: d,
            recommendation: AnalysisRecommendation::RecommendIndependentReview,
        });
    }

    #[test]
    fn seed_replay_and_input_order_are_stable() {
        let t = trial();
        let r = members();
        let a = assign_trial(&t, &r, 1, 2).expect("assign roots");
        let mut reversed = r;
        reversed.reverse();
        let b = assign_trial(&t, &reversed, 1, 2).expect("replay reordered roots");
        assert_eq!(a, b);
        assert_eq!(a, assign_trial(&t, &members(), 1, 2).expect("seed replay"));
        assert_eq!(
            a[0].variant, a[1].variant,
            "roots in a cluster share an arm"
        );
    }
    #[test]
    fn clusters_cannot_receive_multiple_arms_and_duplicates_fail() {
        let mut e = evidence();
        let mut other = e.assignments[0].clone();
        other.root_id = RootId::new("other").expect("valid root");
        other.unit_id = format!("trial-1:scope:{}", other.root_id);
        other.variant = VariantId::new("candidate").expect("valid variant");
        e.assignments.push(other);
        assert!(matches!(validate_trial(&e), Err(TrialError::Collision(_))));
        assert!(matches!(
            assign_trial(
                &trial(),
                &[members()[0].clone(), members()[0].clone()],
                1,
                2
            ),
            Err(TrialError::Collision(_))
        ));
    }
    #[test]
    fn rejects_leakage_invalid_propensity_and_drift() {
        let mut e = evidence();
        e.features.push(FeatureManifest {
            feature_id: FeatureId::new("future").expect("feature id"),
            digest: "sha256:f".into(),
            available_tick: DecimalU64::new(51),
            decision_phase: DecisionPhase::InitialAssignment,
            missingness: Missingness::Unknown,
            extraction_cost_micros: None,
        });
        assert!(validate_trial(&e).is_err());
        e.features[0].available_tick = DecimalU64::new(10);
        e.features[0].decision_phase = DecisionPhase::Continuation;
        assert!(validate_trial(&e).is_err());
        assert!(assign_trial(&trial(), &members(), 0, 2).is_err());
        assert!(assign_trial(&trial(), &members(), 3, 2).is_err());
        e.features.clear();
        e.protected_oracle_digest_at_analysis = "sha256:changed".into();
        assert!(validate_trial(&e).is_err());
        e.protected_oracle_digest_at_analysis = "sha256:oracle".into();
        e.bundle_digests_at_analysis[0] = "sha256:changed".into();
        assert!(validate_trial(&e).is_err());
    }
    #[test]
    fn fixed_cohort_maturity_peeks_and_analysis_receipt_gate_proposal() {
        let mut e = evidence();
        assert!(matches!(
            propose_trial_decision(&e, vec![]).expect("no-receipt proposal"),
            TrialDecision::InsufficientEvidence { .. }
        ));
        bind_receipt(&mut e);
        e.outcomes[0].label = OutcomeLabel::Cancelled;
        e.outcomes[1].label = OutcomeLabel::Immature;
        e.look_count = DecimalU64::new(2);
        assert!(matches!(
            propose_trial_decision(&e, vec![]).expect("immature proposal"),
            TrialDecision::InsufficientEvidence { .. }
        ));
        e.look_count = DecimalU64::new(1);
        e.outcomes[1].label = OutcomeLabel::Mature;
        bind_receipt(&mut e);
        let decision = propose_trial_decision(&e, e.trial.immutable_bundle_digests.clone())
            .expect("eligible proposal");
        assert!(matches!(
            decision,
            TrialDecision::PromotableProposal {
                promotion_authorized: false,
                rollback: RollbackProposal {
                    applies_to_new_work_only: true,
                    ..
                },
                ..
            }
        ));
    }
    #[test]
    fn serialization_bounds_and_decimal_are_strict() {
        assert!(serde_json::from_str::<DecimalU64>("1").is_err());
        assert!(serde_json::from_str::<DecimalU64>("\"01\"").is_err());
        assert!(serde_json::from_str::<DecimalU64>("\"18446744073709551616\"").is_err());
        let mut t = trial();
        t.pre_registered_seed = "x".repeat(MAX_STR + 1);
        assert!(assign_trial(&t, &members(), 1, 2).is_err());
        let mut e = evidence();
        e.assignments[0].propensity_numerator = DecimalU64::new(0);
        assert!(validate_trial(&e).is_err());
    }
    #[test]
    fn balanced_draw_and_fixed_cohort_cannot_be_reassigned_or_filtered() {
        let mut e = evidence();
        let arms: BTreeSet<_> = e
            .assignments
            .iter()
            .map(|assignment| &assignment.variant)
            .collect();
        assert_eq!(arms.len(), 2, "two clusters receive balanced arms");
        let current = e.assignments[0].variant.clone();
        let other = e.trial.cluster_assignments[0]
            .variants
            .iter()
            .find(|variant| **variant != current)
            .expect("other arm")
            .clone();
        let cluster = e.assignments[0].cluster_id.clone();
        for assignment in &mut e.assignments {
            if assignment.cluster_id == cluster {
                assignment.variant = other.clone();
            }
        }
        assert!(
            validate_trial(&e).is_err(),
            "whole-cluster arm forgery must fail"
        );
        let mut e = evidence();
        e.assignments.pop();
        e.outcomes.pop();
        assert!(
            validate_trial(&e).is_err(),
            "failed roots cannot be dropped"
        );
        let t = trial();
        let one = assign_trial(&t, &members(), 1, 2).expect("original assignment");
        let mut other_scope = t;
        other_scope.project_id = ProjectId::new("foreign").expect("fixture project");
        assert_ne!(
            one[0].unit_id,
            assign_trial(&other_scope, &members(), 1, 2).expect("different project proposal")[0]
                .unit_id
        );
    }
    #[test]
    fn immature_unknown_cost_or_clock_leak_cannot_be_promotable() {
        let mut e = evidence();
        e.outcomes[0].label = OutcomeLabel::Immature;
        bind_receipt(&mut e);
        assert!(matches!(
            propose_trial_decision(&e, vec![]).expect("insufficient"),
            TrialDecision::InsufficientEvidence { .. }
        ));
        let mut e = evidence();
        e.outcomes[0].analysis_value_micros = None;
        bind_receipt(&mut e);
        assert!(matches!(
            propose_trial_decision(&e, vec![]).expect("unknown costs"),
            TrialDecision::InsufficientEvidence { .. }
        ));
        let mut e = evidence();
        e.outcomes[0].observed_tick = DecimalU64::new(49);
        assert!(validate_trial(&e).is_err());
        e.outcomes[0].observed_tick = DecimalU64::new(61);
        assert!(validate_trial(&e).is_err());
        let mut e = evidence();
        bind_receipt(&mut e);
        e.independent_analysis
            .as_mut()
            .expect("receipt")
            .recommendation = AnalysisRecommendation::Reject;
        assert!(matches!(
            propose_trial_decision(&e, vec![]).expect("rejection proposal"),
            TrialDecision::RejectProposal {
                promotion_authorized: false,
                ..
            }
        ));
    }
}
