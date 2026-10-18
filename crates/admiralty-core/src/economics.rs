//! Pure, bounded accounting primitives for fixed assignment cohorts.
use crate::{ProjectId, RootId};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_JSON_BYTES: usize = 1024 * 1024;
pub const MAX_RECORDS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, JsonSchema)]
pub struct DecimalU64(pub u64);
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
pub struct DecimalU128(pub u128);
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
}
impl CohortSpec {
    pub fn validate(&self) -> Result<(), EconomicsError> {
        if self.fixed_assignment_roots.is_empty() || self.fixed_assignment_roots.len() > MAX_RECORDS
        {
            return Err(EconomicsError::Invalid(
                "cohort roots must contain 1..1024 assignments".into(),
            ));
        }
        if self.valuation_digest.len() > 128 || self.valuation_digest.is_empty() {
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
    let roots: BTreeSet<_> = cohort.fixed_assignment_roots.iter().collect();
    let mut latest: BTreeMap<&str, &UsageRecord> = BTreeMap::new();
    let mut seen: BTreeMap<(&str, u64), &UsageRecord> = BTreeMap::new();
    for r in records {
        r.validate()?;
        if !roots.contains(&r.root_scope) {
            continue;
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
        let mut intervals: Vec<(u64, u64)> = Vec::new();
        let ids: BTreeSet<_> = rows.iter().map(|r| r.observation_id.as_str()).collect();
        for row in &rows {
            match &row.parent_inclusion {
                ParentInclusion::Inclusive {
                    parent_observation_id,
                } if ids.contains(parent_observation_id.as_str()) => {
                    uncertain = true;
                    limitations.push(format!(
                        "{}: parent and child usage overlap",
                        row.observation_id.as_str()
                    ));
                    continue;
                }
                ParentInclusion::Interval { start, end } => {
                    if intervals.iter().any(|(a, b)| start.0 <= *b && *a <= end.0) {
                        uncertain = true;
                        limitations.push(format!(
                            "{}: usage intervals overlap",
                            row.observation_id.as_str()
                        ));
                        continue;
                    }
                    intervals.push((start.0, end.0));
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
                accepted += 1
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
    if !matches!(a_unit, Some(u) if u.coverage == Coverage::Complete)
        || !matches!(b_unit, Some(u) if u.coverage == Coverage::Complete)
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
    fn cohort(roots: Vec<RootId>) -> CohortSpec {
        CohortSpec {
            cohort_id: id("cohort"),
            project_id: ProjectId::new("project").expect("fixture project identifier is valid"),
            fixed_assignment_roots: roots,
            required_delivery_stage: id("release"),
            valuation_digest: "sha256:fixture".into(),
            assigned_at: DecimalU64(1),
        }
    }
    fn row(oid: &str, root_name: &str, unit: &str, amount: ObservedAmount) -> UsageRecord {
        UsageRecord {
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
            root_id: root(root_id),
            status,
            accepted_stage: stage.map(id),
            maturity: DecimalU64(maturity),
        }
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
