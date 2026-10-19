//! Pure, bounded accounting primitives for fixed assignment cohorts.
use crate::{ProjectId, RootId, TaskId, content_digest, limits::bounded_json_size};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_RECORDS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
pub struct DecimalU64(#[schemars(with = "String")] pub u64);
impl Serialize for DecimalU64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        canonical_u64(&s).map(Self).map_err(de::Error::custom)
    }
}
impl DecimalU64 {
    pub fn parse(value: &str) -> Result<Self, EconomicsError> {
        canonical_u64(value)
            .map(Self)
            .map_err(|e| EconomicsError::Invalid(e.into()))
    }
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
pub struct DecimalU128(#[schemars(with = "String")] pub u128);
impl Serialize for DecimalU128 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for DecimalU128 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        canonical_u128(&s).map(Self).map_err(de::Error::custom)
    }
}
impl DecimalU128 {
    pub fn parse(value: &str) -> Result<Self, EconomicsError> {
        canonical_u128(value)
            .map(Self)
            .map_err(|e| EconomicsError::Invalid(e.into()))
    }
    pub fn as_u128(&self) -> u128 {
        self.0
    }
}

fn canonical_u64(s: &str) -> Result<u64, &'static str> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("expected canonical non-negative decimal u64");
    }
    s.parse().map_err(|_| "decimal u64 overflow")
}
fn canonical_u128(s: &str) -> Result<u128, &'static str> {
    if s.is_empty() || (s.len() > 1 && s.starts_with('0')) || !s.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("expected canonical non-negative decimal u128");
    }
    s.parse().map_err(|_| "decimal u128 overflow")
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct BoundedId(String);
impl BoundedId {
    pub fn new(value: impl Into<String>) -> Result<Self, EconomicsError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
        {
            return Err(EconomicsError::Invalid(
                "identifier must be 1..128 safe ASCII characters".into(),
            ));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for BoundedId {
    type Error = EconomicsError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<BoundedId> for String {
    fn from(value: BoundedId) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CohortSpec {
    pub cohort_id: BoundedId,
    pub project_id: ProjectId,
    pub fixed_assignment_roots: Vec<RootId>,
    pub required_delivery_stage: BoundedId,
    pub valuation_digest: String,
    pub assigned_at: DecimalU64,
    pub bindings: Vec<RootBinding>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RootBinding {
    pub root_id: RootId,
    pub task_id: TaskId,
    pub contract_revision: u32,
    pub contract_digest: String,
    pub protected_oracle_digest: String,
}
fn digest_valid(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
fn bounded<T: Serialize + ?Sized>(value: &T) -> Result<(), EconomicsError> {
    bounded_json_size(value, MAX_JSON_BYTES)
        .map(|_| ())
        .map_err(|_| EconomicsError::Invalid("invalid or oversized JSON".into()))
}
impl CohortSpec {
    pub fn validate(&self) -> Result<(), EconomicsError> {
        bounded(self)?;
        if self.fixed_assignment_roots.is_empty() || self.fixed_assignment_roots.len() > MAX_RECORDS
        {
            return Err(EconomicsError::Invalid(
                "cohort roots must contain 1..1024 assignments".into(),
            ));
        }
        if !digest_valid(&self.valuation_digest) {
            return Err(EconomicsError::Invalid(
                "valuation digest must contain 1..128 characters".into(),
            ));
        }
        let roots: BTreeSet<_> = self.fixed_assignment_roots.iter().collect();
        if roots.len() != self.fixed_assignment_roots.len() {
            return Err(EconomicsError::Invalid(
                "duplicate fixed assignment root".into(),
            ));
        }
        let bound_roots: BTreeSet<_> = self
            .bindings
            .iter()
            .map(|binding| &binding.root_id)
            .collect();
        if self.bindings.len() != roots.len()
            || bound_roots != roots
            || self.bindings.iter().any(|binding| {
                binding.contract_revision == 0
                    || !digest_valid(&binding.contract_digest)
                    || !digest_valid(&binding.protected_oracle_digest)
            })
        {
            return Err(EconomicsError::Invalid(
                "every fixed root needs one exact contract/oracle binding".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ObservedAmount {
    Known { amount: DecimalU128 },
    Estimated { amount: DecimalU128 },
    Unknown { reason: String },
}
impl ObservedAmount {
    fn numeric(&self) -> Option<(u128, bool)> {
        match self {
            Self::Known { amount } => Some((amount.0, true)),
            Self::Estimated { amount } => Some((amount.0, false)),
            Self::Unknown { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ParentInclusion {
    Standalone,
    Inclusive { parent_observation_id: BoundedId },
    Interval { start: DecimalU64, end: DecimalU64 },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsageRecord {
    pub project_id: ProjectId,
    pub binding: RootBinding,
    pub observation_id: BoundedId,
    pub revision: DecimalU64,
    pub root_scope: RootId,
    pub role: BoundedId,
    pub purpose: BoundedId,
    pub provider: BoundedId,
    pub unit: BoundedId,
    pub amount: ObservedAmount,
    pub parent_inclusion: ParentInclusion,
}
impl UsageRecord {
    pub fn validate(&self) -> Result<(), EconomicsError> {
        if self.revision.0 == 0 {
            return Err(EconomicsError::Invalid("revision must be positive".into()));
        }
        if let ParentInclusion::Interval { start, end } = &self.parent_inclusion
            && start.0 > end.0
        {
            return Err(EconomicsError::Invalid(
                "inclusion interval is reversed".into(),
            ));
        }
        if let ObservedAmount::Unknown { reason } = &self.amount
            && (reason.is_empty() || reason.len() > 4096)
        {
            return Err(EconomicsError::Invalid(
                "unknown amount requires a bounded reason".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RootStatus {
    Succeeded,
    Failed,
    Cancelled,
    Running,
    Reopened,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RootOutcome {
    pub project_id: ProjectId,
    pub binding: RootBinding,
    pub acceptance_receipt_digest: Option<String>,
    pub root_id: RootId,
    pub status: RootStatus,
    pub accepted_stage: Option<BoundedId>,
    pub maturity: DecimalU64,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    Complete,
    Estimated,
    Partial,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UnitTotal {
    pub unit: BoundedId,
    pub amount: Option<DecimalU128>,
    pub coverage: Coverage,
    pub included_records: DecimalU64,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CohortSummary {
    pub cohort_id: BoundedId,
    pub cohort_digest: String,
    pub assigned_roots: DecimalU64,
    pub accepted_roots: Option<DecimalU64>,
    pub failed_roots: DecimalU64,
    pub cancelled_roots: DecimalU64,
    pub immature_roots: DecimalU64,
    pub units: Vec<UnitTotal>,
    pub accepted_yield: Option<Rational>,
    pub legacy_yield_heuristic: Option<Rational>,
    pub limitations: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rational {
    pub numerator: DecimalU128,
    pub denominator: DecimalU128,
}
impl Rational {
    pub fn new(numerator: u128, denominator: u128) -> Option<Self> {
        (denominator != 0).then_some(Self {
            numerator: DecimalU128(numerator),
            denominator: DecimalU128(denominator),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonStatus {
    Comparable,
    InsufficientEvidence,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CohortComparison {
    pub status: ComparisonStatus,
    pub coordination_share: Option<Rational>,
    pub accepted_yield_delta: Option<Rational>,
    pub reasons: Vec<String>,
    pub savings_claim: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EconomicsError {
    #[error("invalid economics data: {0}")]
    Invalid(String),
    #[error("same observation key has different bytes")]
    RevisionCollision,
    #[error("checked arithmetic overflow")]
    Overflow,
}

pub fn parse_bounded_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, EconomicsError> {
    if bytes.len() > MAX_JSON_BYTES {
        return Err(EconomicsError::Invalid("JSON exceeds 1 MiB".into()));
    }
    serde_json::from_slice(bytes).map_err(|e| EconomicsError::Invalid(format!("invalid JSON: {e}")))
}

pub fn summarize_cohort(
    cohort: &CohortSpec,
    records: &[UsageRecord],
    outcomes: &[RootOutcome],
    maturity_cutoff: u64,
) -> Result<CohortSummary, EconomicsError> {
    cohort.validate()?;
    if records.len() > MAX_RECORDS || outcomes.len() > MAX_RECORDS {
        return Err(EconomicsError::Invalid(
            "record collection exceeds 1024".into(),
        ));
    }
    bounded(&(cohort, records, outcomes))?;
    let roots: BTreeSet<_> = cohort.fixed_assignment_roots.iter().collect();
    let binding_for = |root: &RootId| {
        cohort
            .bindings
            .iter()
            .find(|binding| &binding.root_id == root)
    };
    let mut latest: BTreeMap<&str, &UsageRecord> = BTreeMap::new();
    let mut seen: BTreeMap<(&str, u64), &UsageRecord> = BTreeMap::new();
    for r in records {
        r.validate()?;
        if r.project_id != cohort.project_id
            || r.root_scope != r.binding.root_id
            || binding_for(&r.root_scope) != Some(&r.binding)
        {
            return Err(EconomicsError::Invalid(
                "usage outside exact fixed project/task contract scope".into(),
            ));
        }
        if let Some(previous) = latest.get(r.observation_id.as_str()) {
            let mut old_identity = (*previous).clone();
            let mut new_identity = r.clone();
            old_identity.revision = DecimalU64(1);
            new_identity.revision = DecimalU64(1);
            old_identity.amount = ObservedAmount::Unknown {
                reason: "identity".into(),
            };
            new_identity.amount = old_identity.amount.clone();
            if old_identity != new_identity {
                return Err(EconomicsError::RevisionCollision);
            }
        }
        let key = (r.observation_id.as_str(), r.revision.0);
        if let Some(old) = seen.insert(key, r) {
            if old != r {
                return Err(EconomicsError::RevisionCollision);
            } else {
                continue;
            }
        }
        match latest.get(r.observation_id.as_str()) {
            Some(old) if old.revision.0 > r.revision.0 => {}
            _ => {
                latest.insert(r.observation_id.as_str(), r);
            }
        }
    }
    for record in latest.values() {
        let mut cursor = *record;
        let mut lineage = BTreeSet::new();
        while let ParentInclusion::Inclusive {
            parent_observation_id,
        } = &cursor.parent_inclusion
        {
            if !lineage.insert(cursor.observation_id.as_str()) {
                return Err(EconomicsError::Invalid("usage inclusion cycle".into()));
            }
            let Some(parent) = latest.get(parent_observation_id.as_str()) else {
                break;
            };
            if parent.project_id != record.project_id
                || parent.binding != record.binding
                || parent.provider != record.provider
                || parent.unit != record.unit
            {
                return Err(EconomicsError::Invalid(
                    "usage parent scope/provider/unit mismatch".into(),
                ));
            }
            cursor = parent;
        }
    }
    let mut by_unit: BTreeMap<&str, Vec<&UsageRecord>> = BTreeMap::new();
    for r in latest.values() {
        by_unit.entry(r.unit.as_str()).or_default().push(r);
    }
    let mut units = Vec::new();
    let mut limitations = Vec::new();
    for (unit, rows) in by_unit {
        let mut total = 0u128;
        let mut count = 0u64;
        let mut estimated = false;
        let mut uncertain = false;
        let mut intervals: Vec<(&RootBinding, &BoundedId, &BoundedId, &BoundedId, u64, u64)> =
            Vec::new();
        let ids: BTreeSet<_> = rows.iter().map(|r| r.observation_id.as_str()).collect();
        for row in &rows {
            match &row.parent_inclusion {
                ParentInclusion::Inclusive {
                    parent_observation_id,
                } => {
                    uncertain = true;
                    limitations.push(format!(
                        "{}: parent/child inclusion needs complete coverage reconciliation",
                        row.observation_id.as_str()
                    ));
                    if !ids.contains(parent_observation_id.as_str()) {
                        limitations.push("included parent observation is missing".into());
                    }
                    continue;
                }
                ParentInclusion::Interval { start, end } => {
                    if intervals
                        .iter()
                        .any(|(binding, provider, role, purpose, a, b)| {
                            *binding == &row.binding
                                && *provider == &row.provider
                                && *role == &row.role
                                && *purpose == &row.purpose
                                && start.0 <= *b
                                && *a <= end.0
                        })
                    {
                        uncertain = true;
                        limitations.push(format!(
                            "{}: usage intervals overlap",
                            row.observation_id.as_str()
                        ));
                        continue;
                    }
                    intervals.push((
                        &row.binding,
                        &row.provider,
                        &row.role,
                        &row.purpose,
                        start.0,
                        end.0,
                    ));
                }
                _ => {}
            }
            match row.amount.numeric() {
                Some((v, exact)) => {
                    total = total.checked_add(v).ok_or(EconomicsError::Overflow)?;
                    count = count.checked_add(1).ok_or(EconomicsError::Overflow)?;
                    estimated |= !exact;
                }
                None => {
                    uncertain = true;
                    limitations.push(format!("{}: usage is unknown", row.observation_id.as_str()));
                }
            }
        }
        let full = count as usize == rows.len() && !uncertain;
        units.push(UnitTotal {
            unit: BoundedId::new(unit.to_owned())?,
            amount: (!uncertain || count > 0).then_some(DecimalU128(total)),
            coverage: if uncertain && count == 0 {
                Coverage::Unknown
            } else if uncertain {
                Coverage::Partial
            } else if estimated {
                Coverage::Estimated
            } else {
                Coverage::Complete
            },
            included_records: DecimalU64(count),
        });
        if !full {
            limitations.push(format!(
                "unit {unit}: partial or ambiguous coverage; subtotal is not an all-in total"
            ));
        }
    }
    let mut outcome_map = BTreeMap::new();
    for outcome in outcomes {
        if outcome.project_id != cohort.project_id
            || outcome.root_id != outcome.binding.root_id
            || binding_for(&outcome.root_id) != Some(&outcome.binding)
            || outcome
                .acceptance_receipt_digest
                .as_ref()
                .is_some_and(|value| !digest_valid(value))
        {
            return Err(EconomicsError::Invalid(
                "outcome outside exact fixed project/task contract scope".into(),
            ));
        }
        if roots.contains(&outcome.root_id)
            && outcome_map.insert(&outcome.root_id, outcome).is_some()
        {
            return Err(EconomicsError::Invalid("duplicate root outcome".into()));
        }
    }
    let mut accepted = 0u64;
    let mut failed = 0u64;
    let mut cancelled = 0u64;
    let mut immature = 0u64;
    let mut all_known = true;
    for root in &cohort.fixed_assignment_roots {
        match outcome_map.get(root) {
            Some(o)
                if o.maturity.0 >= maturity_cutoff
                    && matches!(o.status, RootStatus::Succeeded)
                    && o.accepted_stage.as_ref() == Some(&cohort.required_delivery_stage) =>
            {
                if o.acceptance_receipt_digest.is_some() {
                    accepted += 1;
                } else {
                    all_known = false;
                }
            }
            Some(o) => {
                if o.status == RootStatus::Failed {
                    failed += 1;
                }
                if o.status == RootStatus::Cancelled {
                    cancelled += 1;
                }
                if o.maturity.0 < maturity_cutoff
                    || matches!(o.status, RootStatus::Running | RootStatus::Reopened)
                {
                    immature += 1;
                }
            }
            None => {
                all_known = false;
                immature += 1;
            }
        }
    }
    let yield_value = if all_known && immature == 0 {
        Rational::new(
            accepted as u128,
            cohort.fixed_assignment_roots.len() as u128,
        )
    } else {
        None
    };
    if units.iter().any(|u| u.coverage != Coverage::Complete) {
        limitations.push("usage coverage is incomplete; no savings claim is supported".into());
    }
    Ok(CohortSummary {
        cohort_id: cohort.cohort_id.clone(),
        cohort_digest: content_digest(cohort)
            .map_err(|_| EconomicsError::Invalid("cohort digest failed".into()))?,
        assigned_roots: DecimalU64(cohort.fixed_assignment_roots.len() as u64),
        accepted_roots: all_known.then_some(DecimalU64(accepted)),
        failed_roots: DecimalU64(failed),
        cancelled_roots: DecimalU64(cancelled),
        immature_roots: DecimalU64(immature),
        units,
        accepted_yield: yield_value,
        legacy_yield_heuristic: None,
        limitations,
    })
}

pub fn cohort_compare(
    a: &CohortSpec,
    a_summary: &CohortSummary,
    b: &CohortSpec,
    b_summary: &CohortSummary,
    coordination_unit: &str,
) -> CohortComparison {
    let mut reasons = Vec::new();
    for (spec, summary) in [(a, a_summary), (b, b_summary)] {
        let expected = content_digest(spec).ok();
        if spec.validate().is_err()
            || bounded(summary).is_err()
            || summary.cohort_id != spec.cohort_id
            || expected.as_ref() != Some(&summary.cohort_digest)
            || summary.assigned_roots.0 != spec.fixed_assignment_roots.len() as u64
            || summary
                .accepted_roots
                .as_ref()
                .is_none_or(|n| n.0 > summary.assigned_roots.0)
            || summary.immature_roots.0 != 0
            || summary.accepted_yield.as_ref().is_none_or(|ratio| {
                ratio.denominator.0 != u128::from(summary.assigned_roots.0)
                    || ratio.numerator.0 > ratio.denominator.0
                    || summary
                        .accepted_roots
                        .as_ref()
                        .is_none_or(|accepted| ratio.numerator.0 != u128::from(accepted.0))
            })
        {
            reasons.push("summary does not bind a complete consistent original cohort".into());
        }
    }
    if a.project_id != b.project_id
        || a.valuation_digest != b.valuation_digest
        || a.required_delivery_stage != b.required_delivery_stage
    {
        reasons.push("project, valuation, or required delivery stage differs".into());
    }
    if a_summary.accepted_yield.is_none() || b_summary.accepted_yield.is_none() {
        reasons.push("cohort maturity or accepted outcome coverage is incomplete".into());
    }
    let a_unit = a_summary
        .units
        .iter()
        .find(|u| u.unit.as_str() == coordination_unit);
    let b_unit = b_summary
        .units
        .iter()
        .find(|u| u.unit.as_str() == coordination_unit);
    if !matches!(a_unit, Some(u) if u.coverage == Coverage::Complete && u.amount.is_some())
        || !matches!(b_unit, Some(u) if u.coverage == Coverage::Complete && u.amount.is_some())
    {
        reasons.push("coordination usage is missing, partial, or uses incompatible units".into());
    }
    let comparable = reasons.is_empty();
    CohortComparison {
        status: if comparable {
            ComparisonStatus::Comparable
        } else {
            ComparisonStatus::InsufficientEvidence
        },
        coordination_share: None,
        accepted_yield_delta: None,
        reasons,
        savings_claim: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(s: &str) -> BoundedId {
        BoundedId::new(s).expect("fixture identifier is valid")
    }
    fn root(s: &str) -> RootId {
        RootId::new(s).expect("fixture root identifier is valid")
    }
    fn binding(root_name: &str) -> RootBinding {
        RootBinding {
            root_id: root(root_name),
            task_id: TaskId::new(format!("task-{root_name}")).expect("fixture task"),
            contract_revision: 1,
            contract_digest: format!("sha256:{}", "a".repeat(64)),
            protected_oracle_digest: format!("sha256:{}", "b".repeat(64)),
        }
    }
    fn cohort(roots: Vec<RootId>) -> CohortSpec {
        CohortSpec {
            cohort_id: id("cohort"),
            project_id: ProjectId::new("project").expect("fixture project identifier is valid"),
            bindings: roots.iter().map(|root| binding(root.as_str())).collect(),
            fixed_assignment_roots: roots,
            required_delivery_stage: id("release"),
            valuation_digest: format!("sha256:{}", "c".repeat(64)),
            assigned_at: DecimalU64(1),
        }
    }
    fn row(oid: &str, root_name: &str, unit: &str, amount: ObservedAmount) -> UsageRecord {
        UsageRecord {
            project_id: ProjectId::new("project").expect("fixture project"),
            binding: binding(root_name),
            observation_id: id(oid),
            revision: DecimalU64(1),
            root_scope: root(root_name),
            role: id("worker"),
            purpose: id("coding"),
            provider: id("provider"),
            unit: id(unit),
            amount,
            parent_inclusion: ParentInclusion::Standalone,
        }
    }
    fn outcome(
        root_id: &str,
        status: RootStatus,
        stage: Option<&str>,
        maturity: u64,
    ) -> RootOutcome {
        RootOutcome {
            project_id: ProjectId::new("project").expect("fixture project"),
            binding: binding(root_id),
            acceptance_receipt_digest: Some(format!("sha256:{}", "d".repeat(64))),
            root_id: root(root_id),
            status,
            accepted_stage: stage.map(id),
            maturity: DecimalU64(maturity),
        }
    }
    #[test]
    fn scope_and_revision_corrections_cannot_reassign_usage() {
        let c = cohort(vec![root("r")]);
        let original = row(
            "event",
            "r",
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(4),
            },
        );
        let mut foreign = original.clone();
        foreign.project_id = ProjectId::new("foreign").expect("fixture project");
        assert!(summarize_cohort(&c, &[foreign], &[], 0).is_err());
        let mut revised = original.clone();
        revised.revision = DecimalU64(2);
        revised.role = id("captain");
        assert_eq!(
            summarize_cohort(&c, &[original.clone(), revised], &[], 0),
            Err(EconomicsError::RevisionCollision)
        );
        let mut stale = original;
        stale.binding.contract_revision = 2;
        assert!(summarize_cohort(&c, &[stale], &[], 0).is_err());
    }
    #[test]
    fn native_intervals_are_scoped_and_orphan_inclusion_is_unknown() {
        let c = cohort(vec![root("a"), root("b")]);
        let mut a = row(
            "a",
            "a",
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(3),
            },
        );
        let mut b = row(
            "b",
            "b",
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(4),
            },
        );
        a.parent_inclusion = ParentInclusion::Interval {
            start: DecimalU64(1),
            end: DecimalU64(2),
        };
        b.parent_inclusion = a.parent_inclusion.clone();
        let summary = summarize_cohort(&c, &[a, b], &[], 0).expect("independent streams");
        assert_eq!(summary.units[0].amount, Some(DecimalU128(7)));
        assert_eq!(summary.units[0].coverage, Coverage::Complete);
        let mut orphan = row(
            "child",
            "a",
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(2),
            },
        );
        orphan.parent_inclusion = ParentInclusion::Inclusive {
            parent_observation_id: id("absent"),
        };
        let summary = summarize_cohort(&c, &[orphan], &[], 0).expect("explicit unknown");
        assert_eq!(summary.units[0].amount, None);
        assert_eq!(summary.units[0].coverage, Coverage::Unknown);
    }
    #[test]
    fn accepted_stage_needs_evidence_and_summary_binding_cannot_be_swapped() {
        let c = cohort(vec![root("r")]);
        let mut result = outcome("r", RootStatus::Succeeded, Some("release"), 1);
        result.acceptance_receipt_digest = None;
        let rows = [row(
            "a",
            "r",
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(1),
            },
        )];
        let summary = summarize_cohort(&c, &rows, &[result], 1).expect("unconfirmed acceptance");
        assert_eq!(summary.accepted_roots, None);
        assert_eq!(summary.accepted_yield, None);
        let confirmed = summarize_cohort(
            &c,
            &rows,
            &[outcome("r", RootStatus::Succeeded, Some("release"), 1)],
            1,
        )
        .expect("recorded receipt");
        let mut changed = c.clone();
        changed.cohort_id = id("other");
        assert_eq!(
            cohort_compare(&changed, &confirmed, &c, &confirmed, "tokens").status,
            ComparisonStatus::InsufficientEvidence
        );
        let mut oversized = rows[0].clone();
        oversized.amount = ObservedAmount::Unknown {
            reason: "x".repeat(4096),
        };
        assert!(summarize_cohort(&c, &vec![oversized; MAX_RECORDS], &[], 1).is_err());
    }
    #[test]
    fn decimal_counters_roundtrip_without_precision_loss() {
        let value = "340282366920938463463374607431768211455";
        let parsed: DecimalU128 =
            serde_json::from_str(&format!("\"{value}\"")).expect("u128 decimal fixture parses");
        assert_eq!(
            serde_json::to_string(&parsed).expect("counter serializes"),
            format!("\"{value}\"")
        );
        assert!(serde_json::from_str::<DecimalU64>("\"01\"").is_err());
    }
    #[test]
    fn tiny_prices_zero_amount_and_unknown_coverage_stay_explicit() {
        let c = cohort(vec![root("r")]);
        let rows = vec![
            row(
                "tiny",
                "r",
                "USD:6",
                ObservedAmount::Known {
                    amount: DecimalU128(1),
                },
            ),
            row(
                "zero",
                "r",
                "tokens",
                ObservedAmount::Known {
                    amount: DecimalU128(0),
                },
            ),
            row(
                "missing",
                "r",
                "calls",
                ObservedAmount::Unknown {
                    reason: "provider omitted".into(),
                },
            ),
        ];
        let s = summarize_cohort(
            &c,
            &rows,
            &[outcome("r", RootStatus::Succeeded, Some("release"), 5)],
            5,
        )
        .expect("cohort summarizes");
        assert_eq!(
            s.units
                .iter()
                .find(|u| u.unit.as_str() == "USD:6")
                .expect("money unit")
                .amount
                .as_ref()
                .map(|v| v.0),
            Some(1)
        );
        assert_eq!(
            s.units
                .iter()
                .find(|u| u.unit.as_str() == "tokens")
                .expect("token unit")
                .amount
                .as_ref()
                .map(|v| v.0),
            Some(0)
        );
        assert_eq!(
            s.units
                .iter()
                .find(|u| u.unit.as_str() == "calls")
                .expect("unknown unit")
                .coverage,
            Coverage::Unknown
        );
        assert!(s.limitations.iter().any(|v| v.contains("no savings claim")));
    }
    #[test]
    fn assignment_keeps_failed_cancelled_and_rework_roots() {
        let c = cohort(vec![
            root("ok"),
            root("bad"),
            root("cancel"),
            root("rework"),
        ]);
        let outcomes = vec![
            outcome("ok", RootStatus::Succeeded, Some("release"), 10),
            outcome("bad", RootStatus::Failed, None, 10),
            outcome("cancel", RootStatus::Cancelled, None, 10),
            outcome("rework", RootStatus::Reopened, None, 2),
        ];
        let s = summarize_cohort(&c, &[], &outcomes, 5).expect("fixed cohort summarizes");
        assert_eq!(s.assigned_roots.0, 4);
        assert_eq!(s.accepted_roots.as_ref().map(|v| v.0), Some(1));
        assert_eq!(s.failed_roots.0, 1);
        assert_eq!(s.cancelled_roots.0, 1);
        assert_eq!(s.immature_roots.0, 1);
        assert_eq!(s.accepted_yield, None);
    }
    #[test]
    fn duplicates_revisions_overlap_and_overflow_are_handled() {
        let c = cohort(vec![root("r")]);
        let a = row(
            "x",
            "r",
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(u128::MAX),
            },
        );
        let mut b = row(
            "y",
            "r",
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(1),
            },
        );
        b.parent_inclusion = ParentInclusion::Inclusive {
            parent_observation_id: id("x"),
        };
        assert_eq!(
            summarize_cohort(&c, &[a.clone(), a.clone()], &[], 0)
                .expect("exact duplicate idempotent")
                .units[0]
                .amount
                .as_ref()
                .map(|v| v.0),
            Some(u128::MAX)
        );
        assert_eq!(
            summarize_cohort(&c, &[a.clone(), b], &[], 0)
                .expect("overlap remains partial")
                .units[0]
                .coverage,
            Coverage::Partial
        );
        let b = row(
            "y",
            "r",
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(1),
            },
        );
        assert_eq!(
            summarize_cohort(&c, &[a, b], &[], 0),
            Err(EconomicsError::Overflow)
        );
    }
    #[test]
    fn corrections_and_manager_rework_observations_stay_in_the_fixed_cohort() {
        let c = cohort(vec![root("r")]);
        let mut first = row(
            "usage",
            "r",
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(9),
            },
        );
        first.role = id("captain");
        first.purpose = id("review");
        let mut correction = first.clone();
        correction.revision = DecimalU64(2);
        correction.amount = ObservedAmount::Known {
            amount: DecimalU128(4),
        };
        let mut rework = row(
            "retry",
            "r",
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(3),
            },
        );
        rework.role = id("super_captain");
        rework.purpose = id("rework");
        let summary = summarize_cohort(&c, &[first, correction, rework], &[], 0)
            .expect("manager and rework usage is included");
        assert_eq!(summary.units[0].amount.as_ref().map(|v| v.0), Some(7));
    }

    #[test]
    fn interval_overlap_and_incompatible_comparison_remain_unqualified() {
        let c = cohort(vec![root("r")]);
        let mut first = row(
            "one",
            "r",
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(2),
            },
        );
        first.parent_inclusion = ParentInclusion::Interval {
            start: DecimalU64(1),
            end: DecimalU64(5),
        };
        let mut second = row(
            "two",
            "r",
            "tokens",
            ObservedAmount::Known {
                amount: DecimalU128(3),
            },
        );
        second.parent_inclusion = ParentInclusion::Interval {
            start: DecimalU64(5),
            end: DecimalU64(8),
        };
        let summary = summarize_cohort(
            &c,
            &[first, second],
            &[outcome("r", RootStatus::Succeeded, Some("release"), 5)],
            5,
        )
        .expect("overlap does not double count");
        assert_eq!(summary.units[0].coverage, Coverage::Partial);
        let mut incompatible = cohort(vec![root("other")]);
        incompatible.valuation_digest = "sha256:other".into();
        let comparison = cohort_compare(&c, &summary, &incompatible, &summary, "tokens");
        assert_eq!(comparison.status, ComparisonStatus::InsufficientEvidence);
        assert!(!comparison.savings_claim);
    }

    #[test]
    fn split_roots_deduplicate_by_fixed_assignment_and_comparison_requires_evidence() {
        let c = cohort(vec![root("root")]);
        let s = summarize_cohort(&c, &[], &[], 0).expect("incomplete cohort is reportable");
        assert_eq!(s.assigned_roots.0, 1);
        let other = cohort(vec![root("root2")]);
        let comparison = cohort_compare(&c, &s, &other, &s, "tokens");
        assert_eq!(comparison.status, ComparisonStatus::InsufficientEvidence);
        assert!(!comparison.savings_claim);
        assert!(Rational::new(0, 0).is_none());
    }
    #[test]
    fn malformed_and_oversize_json_are_rejected() {
        assert!(parse_bounded_json::<serde_json::Value>(br#"{"extra":1}"#).is_ok());
        assert!(parse_bounded_json::<serde_json::Value>(&vec![b' '; MAX_JSON_BYTES + 1]).is_err());
        assert!(serde_json::from_str::<CohortSpec>(r#"{"unexpected":true}"#).is_err());
    }
}
