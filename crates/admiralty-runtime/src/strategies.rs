//! Pure bounded candidate strategy proposals. No launches, leases or acceptance.
use crate::models::{FailureReason, ModelRegistry, SelectionOutcome, SelectionRequest, UsageCost};
use admiralty_core::{TaskContract, content_digest};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const MAX_TOTAL_ATTEMPTS: u16 = 8;
fn default_candidates() -> u8 {
    2
}
fn default_concurrent() -> u16 {
    3
}
fn default_attempts() -> u16 {
    3
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BestOfConfig {
    pub schema_version: u32,
    pub candidate_model_id: String,
    pub synthesis_model_id: String,
    #[serde(default = "default_candidates")]
    pub candidates: u8,
    #[serde(default = "default_concurrent")]
    pub max_concurrent: u16,
    #[serde(default = "default_attempts")]
    pub max_total_attempts: u16,
    pub required_capabilities: BTreeSet<String>,
    pub synthesis_fallback: Option<SynthesisFallback>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SynthesisFallback {
    pub model_id: String,
    pub on_failures: Vec<FailureReason>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyCapacity {
    pub registry_revision: String,
    pub snapshot_revision: String,
    pub observed_tick: u64,
    pub valid_through_tick: u64,
    /// Missing models and null values mean unknown, never zero usage/free slots.
    pub free_slots: BTreeMap<String, Option<u16>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OriginalBinding {
    pub project_id: String,
    pub root_id: String,
    pub task_id: String,
    pub contract_digest: String,
    pub baseline_sha: String,
    pub oracle_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnitRole {
    Variant,
    Synthesis,
    SynthesisFallback,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactRef {
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CandidateRef {
    pub commit_sha: String,
    pub tree_sha: String,
    pub artifact: ArtifactRef,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StrategyWorkUnit {
    pub id: String,
    pub role: UnitRole,
    pub model_id: String,
    pub original: OriginalBinding,
    /// Original contract is unchanged, including scope, negations and authority.
    pub contract: TaskContract,
    pub directory_key: String,
    pub requires_private_checkout: bool,
    pub may_publish: bool,
    pub requires_fresh_original_verification: bool,
    pub candidate_inputs: Vec<CandidateRef>,
    pub diagnostic_inputs: Vec<ArtifactRef>,
    pub usage_cost: UsageCost,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BestOfPlan {
    pub run_id: String,
    pub run_digest: String,
    pub registry_revision: String,
    pub original: OriginalBinding,
    pub contract: TaskContract,
    pub config: BestOfConfig,
    pub variants: Vec<StrategyWorkUnit>,
    /// Includes the finishing reserve for the later synthesis worker.
    pub reserved_slots: BTreeMap<String, u16>,
    pub usage_cost: UsageCost,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StrategyWait {
    CapacityUnknown,
    CapacityInsufficient,
    CapacityStale,
    RegistryChanged,
    UnresolvedVariants,
    AttemptBudget,
    UnqualifiedFailure,
    SetupOrInput,
    FallbackNotConfigured,
    RegistryRouteRequired,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PlanDecision {
    Ready { plan: Box<BestOfPlan> },
    Waiting { reason: StrategyWait },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum VariantOutcome {
    Unresolved,
    Candidate {
        candidate: CandidateRef,
        diagnostic: ArtifactRef,
    },
    ReconciledFailure {
        diagnostic: ArtifactRef,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VariantReport {
    pub unit_id: String,
    pub original: OriginalBinding,
    pub outcome: VariantOutcome,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SynthesisDecision {
    Ready {
        unit: Box<StrategyWorkUnit>,
    },
    Waiting {
        reason: StrategyWait,
    },
    DiagnosticProposal {
        original: OriginalBinding,
        diagnostics: Vec<ArtifactRef>,
        accepted: bool,
        next_action: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QualifiedSynthesisFailure {
    pub unit_id: String,
    pub original: OriginalBinding,
    pub reason: FailureReason,
    pub qualified: bool,
    pub diagnostic: ArtifactRef,
    /// Actual synthesis attempts, not invented counts to force a registry route.
    pub synthesis_attempts_by_tier: BTreeMap<i32, u16>,
    pub total_attempts_used: u16,
}

#[derive(Debug, Error)]
pub enum StrategyError {
    #[error("invalid strategy: {0}")]
    Invalid(&'static str),
    #[error("registry rejected strategy: {0}")]
    Registry(#[from] crate::models::RegistryError),
    #[error("strategy serialization failed: {0}")]
    Json(#[from] serde_json::Error),
}

fn require(value: bool, message: &'static str) -> Result<(), StrategyError> {
    if value {
        Ok(())
    } else {
        Err(StrategyError::Invalid(message))
    }
}
fn hex(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|v| hex(v, &[64]))
}
fn artifact(value: &ArtifactRef) -> Result<(), StrategyError> {
    require(
        digest(&value.sha256),
        "immutable SHA-256 artifact reference required",
    )
}
fn candidate(value: &CandidateRef) -> Result<(), StrategyError> {
    require(
        hex(&value.commit_sha, &[40, 64]) && hex(&value.tree_sha, &[40, 64]),
        "full candidate commit/tree identity required",
    )?;
    artifact(&value.artifact)
}

fn validate_config(config: &BestOfConfig, registry: &ModelRegistry) -> Result<(), StrategyError> {
    registry.validate()?;
    require(
        config.schema_version == 1 && (2..=4).contains(&config.candidates),
        "version 1 and 2..4 candidates required",
    )?;
    require(
        config.max_concurrent > u16::from(config.candidates) && config.max_concurrent <= 5,
        "concurrency must reserve variants plus one later synthesis slot, at most five",
    )?;
    require(
        config.max_total_attempts > u16::from(config.candidates)
            && config.max_total_attempts <= MAX_TOTAL_ATTEMPTS,
        "total attempt budget must cover variants plus synthesis and be at most eight",
    )?;
    require(
        !config.required_capabilities.is_empty(),
        "required capabilities must be explicit",
    )?;
    for id in [&config.candidate_model_id, &config.synthesis_model_id]
        .into_iter()
        .chain(config.synthesis_fallback.iter().map(|f| &f.model_id))
    {
        let model = registry
            .models
            .iter()
            .find(|m| &m.id == id)
            .ok_or(StrategyError::Invalid("configured model does not exist"))?;
        require(
            model.enabled && config.required_capabilities.is_subset(&model.capabilities),
            "configured model must be enabled and capable",
        )?;
    }
    if let Some(fallback) = &config.synthesis_fallback {
        require(
            fallback.model_id != config.synthesis_model_id
                && !fallback.on_failures.is_empty()
                && fallback.on_failures.len() <= 4,
            "fallback needs a distinct model and bounded explicit failure policy",
        )?;
        require(
            !fallback
                .on_failures
                .iter()
                .any(|r| matches!(r, FailureReason::SetupError | FailureReason::BlockedInput)),
            "setup/input failures cannot authorize model escalation",
        )?;
    }
    Ok(())
}

fn capacity_wait(
    registry: &ModelRegistry,
    capacity: &StrategyCapacity,
    now: u64,
    wanted: &BTreeMap<String, u16>,
) -> Result<Option<StrategyWait>, StrategyError> {
    if capacity.registry_revision != registry.revision {
        return Ok(Some(StrategyWait::RegistryChanged));
    }
    require(
        !capacity.snapshot_revision.is_empty() && capacity.snapshot_revision.len() <= 128,
        "explicit bounded capacity snapshot identity required",
    )?;
    if capacity.observed_tick > now
        || capacity.valid_through_tick < now
        || capacity.valid_through_tick < capacity.observed_tick
    {
        return Ok(Some(StrategyWait::CapacityStale));
    }
    for (id, count) in wanted {
        let model = registry
            .models
            .iter()
            .find(|m| &m.id == id)
            .ok_or(StrategyError::Invalid("capacity references unknown model"))?;
        let Some(Some(free)) = capacity.free_slots.get(id) else {
            return Ok(Some(StrategyWait::CapacityUnknown));
        };
        require(
            *free <= model.max_concurrency,
            "capacity cannot exceed configured model limit",
        )?;
        if free < count {
            return Ok(Some(StrategyWait::CapacityInsufficient));
        }
    }
    Ok(None)
}

fn unit(
    plan: &BestOfPlan,
    role: UnitRole,
    ordinal: u16,
    model_id: &str,
) -> Result<StrategyWorkUnit, StrategyError> {
    let key = content_digest(&(&plan.run_digest, &role, ordinal, model_id))?;
    let id = format!("work-{}", key.trim_start_matches("sha256:"));
    Ok(StrategyWorkUnit {
        directory_key: id.clone(),
        id,
        role,
        model_id: model_id.into(),
        original: plan.original.clone(),
        contract: plan.contract.clone(),
        requires_private_checkout: true,
        may_publish: false,
        requires_fresh_original_verification: true,
        candidate_inputs: vec![],
        diagnostic_inputs: vec![],
        usage_cost: UsageCost::Unknown,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn plan_variants(
    config: &BestOfConfig,
    contract: &TaskContract,
    contract_digest: &str,
    baseline_sha: &str,
    oracle_digest: &str,
    run_id: &str,
    registry: &ModelRegistry,
    capacity: &StrategyCapacity,
    now: u64,
) -> Result<PlanDecision, StrategyError> {
    validate_config(config, registry)?;
    contract
        .validate()
        .map_err(|_| StrategyError::Invalid("original task contract invalid"))?;
    require(
        content_digest(contract)? == contract_digest && digest(oracle_digest),
        "exact original contract and protected oracle digests required",
    )?;
    require(
        hex(baseline_sha, &[40, 64]),
        "full common baseline SHA required",
    )?;
    require(
        !run_id.is_empty() && run_id.len() <= 128 && !run_id.chars().any(char::is_control),
        "bounded explicit strategy run identity required",
    )?;
    let original = OriginalBinding {
        project_id: contract.project_id.to_string(),
        root_id: contract.root_id.to_string(),
        task_id: contract.task_id.to_string(),
        contract_digest: contract_digest.into(),
        baseline_sha: baseline_sha.into(),
        oracle_digest: oracle_digest.into(),
    };
    let run_digest = content_digest(&(run_id, &original, config))?;
    let mut slots = BTreeMap::new();
    slots.insert(
        config.candidate_model_id.clone(),
        u16::from(config.candidates),
    );
    *slots.entry(config.synthesis_model_id.clone()).or_insert(0) += 1;
    if let Some(reason) = capacity_wait(registry, capacity, now, &slots)? {
        return Ok(PlanDecision::Waiting { reason });
    }
    let mut plan = BestOfPlan {
        run_id: run_id.into(),
        run_digest,
        registry_revision: registry.revision.clone(),
        original,
        contract: contract.clone(),
        config: config.clone(),
        variants: vec![],
        reserved_slots: slots,
        usage_cost: UsageCost::Unknown,
    };
    for ordinal in 0..config.candidates {
        plan.variants.push(unit(
            &plan,
            UnitRole::Variant,
            u16::from(ordinal),
            &config.candidate_model_id,
        )?);
    }
    Ok(PlanDecision::Ready {
        plan: Box::new(plan),
    })
}

fn validate_plan(plan: &BestOfPlan, registry: &ModelRegistry) -> Result<(), StrategyError> {
    validate_config(&plan.config, registry)?;
    plan.contract
        .validate()
        .map_err(|_| StrategyError::Invalid("original task contract invalid"))?;
    require(
        plan.registry_revision == registry.revision,
        "registry changed; regenerate/reconcile strategy proposal",
    )?;
    require(
        content_digest(&plan.contract)? == plan.original.contract_digest
            && plan.contract.project_id.as_str() == plan.original.project_id
            && plan.contract.root_id.as_str() == plan.original.root_id
            && plan.contract.task_id.as_str() == plan.original.task_id
            && digest(&plan.original.oracle_digest)
            && hex(&plan.original.baseline_sha, &[40, 64]),
        "plan original binding drift",
    )?;
    require(
        content_digest(&(&plan.run_id, &plan.original, &plan.config))? == plan.run_digest,
        "strategy run digest drift",
    )?;
    require(
        plan.variants.len() == usize::from(plan.config.candidates),
        "variant count drift",
    )?;
    for (ordinal, actual) in plan.variants.iter().enumerate() {
        let expected = unit(
            plan,
            UnitRole::Variant,
            ordinal as u16,
            &plan.config.candidate_model_id,
        )?;
        require(
            serde_json::to_value(actual)? == serde_json::to_value(expected)?,
            "variant unit/scope drift",
        )?;
    }
    Ok(())
}

pub fn plan_synthesis(
    plan: &BestOfPlan,
    reports: &[VariantReport],
    registry: &ModelRegistry,
    capacity: &StrategyCapacity,
    now: u64,
) -> Result<SynthesisDecision, StrategyError> {
    validate_plan(plan, registry)?;
    require(
        reports.len() <= plan.variants.len(),
        "too many variant reports",
    )?;
    let known: BTreeSet<_> = plan.variants.iter().map(|u| u.id.as_str()).collect();
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    let mut diagnostics = Vec::new();
    let mut unresolved = reports.len() != plan.variants.len();
    for report in reports {
        require(
            known.contains(report.unit_id.as_str())
                && seen.insert(&report.unit_id)
                && report.original == plan.original,
            "duplicate, foreign or stale variant report",
        )?;
        match &report.outcome {
            VariantOutcome::Unresolved => unresolved = true,
            VariantOutcome::Candidate {
                candidate: value,
                diagnostic,
            } => {
                candidate(value)?;
                artifact(diagnostic)?;
                candidates.push(value.clone());
                diagnostics.push(diagnostic.clone());
            }
            VariantOutcome::ReconciledFailure { diagnostic } => {
                artifact(diagnostic)?;
                diagnostics.push(diagnostic.clone());
            }
        }
    }
    if unresolved {
        return Ok(SynthesisDecision::Waiting {
            reason: StrategyWait::UnresolvedVariants,
        });
    }
    // Canonical ordering makes replay independent of report delivery order.
    candidates.sort_by(|a, b| {
        (&a.commit_sha, &a.tree_sha, &a.artifact.sha256).cmp(&(
            &b.commit_sha,
            &b.tree_sha,
            &b.artifact.sha256,
        ))
    });
    diagnostics.sort_by(|a, b| a.sha256.cmp(&b.sha256));
    if candidates.is_empty() {
        return Ok(SynthesisDecision::DiagnosticProposal { original: plan.original.clone(), diagnostics, accepted: false, next_action: "Review diagnostic receipts and explicitly route a bounded repair or manual decision; no candidate is accepted".into() });
    }
    let slots = BTreeMap::from([(plan.config.synthesis_model_id.clone(), 1)]);
    if let Some(reason) = capacity_wait(registry, capacity, now, &slots)? {
        return Ok(SynthesisDecision::Waiting { reason });
    }
    let mut synthesis = unit(
        plan,
        UnitRole::Synthesis,
        0,
        &plan.config.synthesis_model_id,
    )?;
    synthesis.candidate_inputs = candidates;
    synthesis.diagnostic_inputs = diagnostics;
    Ok(SynthesisDecision::Ready {
        unit: Box::new(synthesis),
    })
}

pub fn plan_synthesis_fallback(
    plan: &BestOfPlan,
    synthesis: &StrategyWorkUnit,
    failure: &QualifiedSynthesisFailure,
    registry: &ModelRegistry,
    capacity: &StrategyCapacity,
    now: u64,
) -> Result<SynthesisDecision, StrategyError> {
    validate_plan(plan, registry)?;
    let expected = unit(
        plan,
        UnitRole::Synthesis,
        0,
        &plan.config.synthesis_model_id,
    )?;
    require(
        synthesis.id == expected.id
            && synthesis.role == UnitRole::Synthesis
            && synthesis.model_id == expected.model_id
            && synthesis.directory_key == expected.directory_key
            && synthesis.contract == plan.contract
            && synthesis.original == plan.original
            && failure.unit_id == synthesis.id
            && failure.original == plan.original
            && synthesis.requires_private_checkout
            && !synthesis.may_publish
            && synthesis.requires_fresh_original_verification,
        "failure must bind unchanged original synthesis unit",
    )?;
    require(
        !synthesis.candidate_inputs.is_empty()
            && synthesis.candidate_inputs.len() <= plan.variants.len()
            && synthesis.diagnostic_inputs.len() <= plan.variants.len(),
        "bounded immutable synthesis inputs required",
    )?;
    for value in &synthesis.candidate_inputs {
        candidate(value)?;
    }
    for value in &synthesis.diagnostic_inputs {
        artifact(value)?;
    }
    artifact(&failure.diagnostic)?;
    if !failure.qualified {
        return Ok(SynthesisDecision::Waiting {
            reason: StrategyWait::UnqualifiedFailure,
        });
    }
    if matches!(
        failure.reason,
        FailureReason::SetupError | FailureReason::BlockedInput
    ) {
        return Ok(SynthesisDecision::Waiting {
            reason: StrategyWait::SetupOrInput,
        });
    }
    let Some(fallback) = &plan.config.synthesis_fallback else {
        return Ok(SynthesisDecision::Waiting {
            reason: StrategyWait::FallbackNotConfigured,
        });
    };
    if !fallback.on_failures.contains(&failure.reason) {
        return Ok(SynthesisDecision::Waiting {
            reason: StrategyWait::RegistryRouteRequired,
        });
    }
    require(
        failure.total_attempts_used > u16::from(plan.config.candidates),
        "failure cannot omit prior variant/synthesis attempts",
    )?;
    let synthesis_attempts = failure
        .synthesis_attempts_by_tier
        .values()
        .try_fold(0_u16, |sum, count| sum.checked_add(*count))
        .ok_or(StrategyError::Invalid("attempt accounting overflow"))?;
    require(
        synthesis_attempts.checked_add(u16::from(plan.config.candidates))
            == Some(failure.total_attempts_used),
        "actual synthesis tier counts must reconcile to total strategy attempts",
    )?;
    if failure.total_attempts_used >= plan.config.max_total_attempts {
        return Ok(SynthesisDecision::Waiting {
            reason: StrategyWait::AttemptBudget,
        });
    }
    let slots = BTreeMap::from([(fallback.model_id.clone(), 1)]);
    if let Some(reason) = capacity_wait(registry, capacity, now, &slots)? {
        return Ok(SynthesisDecision::Waiting { reason });
    }
    let mut registry_capacity = crate::models::CapacitySnapshot::default();
    for model in &registry.models {
        match capacity.free_slots.get(&model.id) {
            Some(Some(free)) if *free <= model.max_concurrency => {
                registry_capacity
                    .in_use
                    .insert(model.id.clone(), model.max_concurrency - free);
            }
            _ => {
                registry_capacity.unavailable.insert(model.id.clone());
            }
        }
    }
    let request = SelectionRequest {
        required_capabilities: plan.config.required_capabilities.clone(),
        current_model: Some(synthesis.model_id.clone()),
        attempts_by_tier: failure.synthesis_attempts_by_tier.clone(),
        last_failure: Some(failure.reason),
        allow_escalation: true,
        ..Default::default()
    };
    let selected = registry.select(&request, &registry_capacity)?;
    if !matches!(selected, SelectionOutcome::Selected { ref model_id, .. } if model_id == &fallback.model_id)
    {
        return Ok(SynthesisDecision::Waiting {
            reason: StrategyWait::RegistryRouteRequired,
        });
    }
    let mut work = unit(
        plan,
        UnitRole::SynthesisFallback,
        failure.total_attempts_used,
        &fallback.model_id,
    )?;
    work.candidate_inputs = synthesis.candidate_inputs.clone();
    work.diagnostic_inputs = synthesis.diagnostic_inputs.clone();
    work.diagnostic_inputs.push(failure.diagnostic.clone());
    Ok(SynthesisDecision::Ready {
        unit: Box::new(work),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ModelSpec, Provider};
    use admiralty_core::{AcceptanceCriterion, CheckSpec, ProjectId, RootId, TaskId};
    fn contract() -> TaskContract {
        TaskContract {
            schema_version: 1,
            project_id: ProjectId::new("project-a").expect("fixture"),
            root_id: RootId::new("root-a").expect("fixture"),
            task_id: TaskId::new("task-a").expect("fixture"),
            revision: 1,
            title: "Bounded task".into(),
            objective: "Implement; do not change oracle".into(),
            non_goals: vec!["No publication".into()],
            owned_paths: vec!["src/owned.rs".into()],
            forbidden_paths: vec!["oracle/**".into()],
            read_first: vec!["README.md".into()],
            dependencies: vec![],
            criteria: vec![AcceptanceCriterion {
                id: "criterion".into(),
                text: "Original acceptance".into(),
                check_ids: vec!["check".into()],
            }],
            checks: vec![CheckSpec {
                id: "check".into(),
                program: "check".into(),
                args: vec![],
                cwd: ".".into(),
                timeout_secs: 10,
            }],
            authority_ref: "owner-policy-v1".into(),
            max_attempts: 1,
            simulated: true,
        }
    }
    fn registry() -> ModelRegistry {
        ModelRegistry {
            schema_version: 1,
            revision: "7".into(),
            models: [
                ("own-model", 0, Some("fallback-model")),
                ("fallback-model", 1, None),
            ]
            .into_iter()
            .map(|(id, tier, next)| ModelSpec {
                id: id.into(),
                label: id.into(),
                provider: Provider::CodexCli,
                provider_model: id.into(),
                tier,
                enabled: true,
                node_id: None,
                endpoint: None,
                credential_file_ref: None,
                max_concurrency: 8,
                capabilities: BTreeSet::from(["coding".into()]),
                task_max_attempts: 1,
                escalate_to: next.map(str::to_owned),
            })
            .collect(),
        }
    }
    fn config() -> BestOfConfig {
        BestOfConfig {
            schema_version: 1,
            candidate_model_id: "own-model".into(),
            synthesis_model_id: "own-model".into(),
            candidates: 2,
            max_concurrent: 3,
            max_total_attempts: 4,
            required_capabilities: BTreeSet::from(["coding".into()]),
            synthesis_fallback: Some(SynthesisFallback {
                model_id: "fallback-model".into(),
                on_failures: vec![FailureReason::TestFail],
            }),
        }
    }
    fn capacity() -> StrategyCapacity {
        StrategyCapacity {
            registry_revision: "7".into(),
            snapshot_revision: "capacity-1".into(),
            observed_tick: 10,
            valid_through_tick: 20,
            free_slots: BTreeMap::from([
                ("own-model".into(), Some(8)),
                ("fallback-model".into(), Some(8)),
            ]),
        }
    }
    fn plan() -> BestOfPlan {
        let c = contract();
        match plan_variants(
            &config(),
            &c,
            &content_digest(&c).expect("digest"),
            &"a".repeat(40),
            &format!("sha256:{}", "b".repeat(64)),
            "run-a",
            &registry(),
            &capacity(),
            10,
        )
        .expect("valid plan")
        {
            PlanDecision::Ready { plan } => *plan,
            _ => panic!("fixture capacity ready"),
        }
    }
    fn reference() -> ArtifactRef {
        ArtifactRef {
            sha256: format!("sha256:{}", "c".repeat(64)),
        }
    }
    fn reports(p: &BestOfPlan, failed: bool) -> Vec<VariantReport> {
        p.variants
            .iter()
            .map(|unit| VariantReport {
                unit_id: unit.id.clone(),
                original: p.original.clone(),
                outcome: if failed {
                    VariantOutcome::ReconciledFailure {
                        diagnostic: reference(),
                    }
                } else {
                    VariantOutcome::Candidate {
                        candidate: CandidateRef {
                            commit_sha: "d".repeat(40),
                            tree_sha: "e".repeat(40),
                            artifact: reference(),
                        },
                        diagnostic: reference(),
                    }
                },
            })
            .collect()
    }
    #[test]
    fn baseline_scope_oracle_and_replay_are_bound() {
        let p = plan();
        assert_eq!(p.reserved_slots["own-model"], 3);
        assert_ne!(p.variants[0].directory_key, p.variants[1].directory_key);
        assert_eq!(p.variants[0].id, plan().variants[0].id);
        for u in &p.variants {
            assert_eq!(u.original, p.original);
            assert_eq!(u.contract, p.contract);
            assert!(!u.may_publish);
            assert!(u.requires_private_checkout && u.requires_fresh_original_verification);
            assert_eq!(u.usage_cost, UsageCost::Unknown);
        }
        let mut bad = p.clone();
        bad.variants[0]
            .contract
            .owned_paths
            .push("foreign.rs".into());
        assert!(plan_synthesis(&bad, &reports(&p, false), &registry(), &capacity(), 10).is_err());
        let mut reports = reports(&p, false);
        reports[0].original.oracle_digest = format!("sha256:{}", "f".repeat(64));
        assert!(plan_synthesis(&p, &reports, &registry(), &capacity(), 10).is_err());
    }
    #[test]
    fn capacity_unknown_insufficient_and_stale_never_fan_out() {
        let c = contract();
        let d = content_digest(&c).expect("digest");
        for (value, want) in [
            (None, StrategyWait::CapacityUnknown),
            (Some(2), StrategyWait::CapacityInsufficient),
        ] {
            let mut cap = capacity();
            cap.free_slots.insert("own-model".into(), value);
            assert!(
                matches!(plan_variants(&config(),&c,&d,&"a".repeat(40),&format!("sha256:{}","b".repeat(64)),"run-a",&registry(),&cap,10).expect("capacity wait"),PlanDecision::Waiting{reason} if reason==want)
            );
        }
        assert!(matches!(
            plan_variants(
                &config(),
                &c,
                &d,
                &"a".repeat(40),
                &format!("sha256:{}", "b".repeat(64)),
                "run-a",
                &registry(),
                &capacity(),
                21
            )
            .expect("stale"),
            PlanDecision::Waiting {
                reason: StrategyWait::CapacityStale
            }
        ));
    }
    #[test]
    fn bounds_and_wrong_contract_rejected() {
        let c = contract();
        for n in [0, 1, 5, 255] {
            let mut cfg = config();
            cfg.candidates = n;
            assert!(
                plan_variants(
                    &cfg,
                    &c,
                    &content_digest(&c).expect("digest"),
                    &"a".repeat(40),
                    &format!("sha256:{}", "b".repeat(64)),
                    "run",
                    &registry(),
                    &capacity(),
                    10
                )
                .is_err()
            );
        }
        for (concurrent, total) in [(2, 4), (6, 4), (3, 2), (3, 9)] {
            let mut cfg = config();
            cfg.max_concurrent = concurrent;
            cfg.max_total_attempts = total;
            assert!(validate_config(&cfg, &registry()).is_err());
        }
        assert!(
            plan_variants(
                &config(),
                &c,
                "sha256:wrong",
                &"a".repeat(40),
                &format!("sha256:{}", "b".repeat(64)),
                "run",
                &registry(),
                &capacity(),
                10
            )
            .is_err()
        );
    }
    #[test]
    fn run_identity_and_original_scope_separate_variant_directories() {
        let original = plan();
        for (run_id, project) in [("run-b", "project-a"), ("run-a", "project-b")] {
            let mut task = contract();
            task.project_id = ProjectId::new(project).expect("fixture project");
            let decision = plan_variants(
                &config(),
                &task,
                &content_digest(&task).expect("digest"),
                &"a".repeat(40),
                &format!("sha256:{}", "b".repeat(64)),
                run_id,
                &registry(),
                &capacity(),
                10,
            )
            .expect("bounded proposal");
            let PlanDecision::Ready { plan: other } = decision else {
                panic!("fixture capacity ready");
            };
            assert!(other.variants.iter().all(|other_unit| {
                original
                    .variants
                    .iter()
                    .all(|unit| unit.directory_key != other_unit.directory_key)
            }));
        }
    }
    #[test]
    fn unresolved_worker_blocks_judge_and_all_failed_only_proposes_diagnosis() {
        let p = plan();
        let mut r = reports(&p, false);
        r[1].outcome = VariantOutcome::Unresolved;
        assert!(matches!(
            plan_synthesis(&p, &r, &registry(), &capacity(), 10).expect("wait"),
            SynthesisDecision::Waiting {
                reason: StrategyWait::UnresolvedVariants
            }
        ));
        assert!(matches!(
            plan_synthesis(&p, &reports(&p, true), &registry(), &capacity(), 10)
                .expect("diagnostic proposal"),
            SynthesisDecision::DiagnosticProposal {
                accepted: false,
                ..
            }
        ));
    }
    #[test]
    fn synthesis_inputs_are_immutable_and_order_independent() {
        let p = plan();
        let r = reports(&p, false);
        let mut reversed = r.clone();
        reversed.reverse();
        let one = plan_synthesis(&p, &r, &registry(), &capacity(), 10).expect("synthesis");
        let two = plan_synthesis(&p, &reversed, &registry(), &capacity(), 10).expect("replay");
        assert_eq!(
            serde_json::to_value(&one).expect("json"),
            serde_json::to_value(&two).expect("json")
        );
        if let SynthesisDecision::Ready { unit } = one {
            assert_eq!(unit.role, UnitRole::Synthesis);
            assert_eq!(unit.candidate_inputs.len(), 2);
            assert!(!unit.may_publish);
            assert!(unit.requires_fresh_original_verification);
            assert_ne!(unit.directory_key, p.variants[0].directory_key);
        } else {
            panic!("synthesis ready")
        }
    }
    #[test]
    fn fallback_requires_qualified_failure_capacity_budget_and_registry_route() {
        let p = plan();
        let synthesis = match plan_synthesis(&p, &reports(&p, false), &registry(), &capacity(), 10)
            .expect("synthesis")
        {
            SynthesisDecision::Ready { unit } => unit,
            _ => panic!("ready"),
        };
        let mut failure = QualifiedSynthesisFailure {
            unit_id: synthesis.id.clone(),
            original: p.original.clone(),
            reason: FailureReason::TestFail,
            qualified: false,
            diagnostic: reference(),
            synthesis_attempts_by_tier: BTreeMap::from([(0, 1)]),
            total_attempts_used: 3,
        };
        assert!(matches!(
            plan_synthesis_fallback(&p, &synthesis, &failure, &registry(), &capacity(), 10)
                .expect("wait"),
            SynthesisDecision::Waiting {
                reason: StrategyWait::UnqualifiedFailure
            }
        ));
        failure.qualified = true;
        let mut changed = (*synthesis).clone();
        changed.contract.owned_paths.push("foreign.rs".into());
        assert!(
            plan_synthesis_fallback(&p, &changed, &failure, &registry(), &capacity(), 10).is_err()
        );
        failure.total_attempts_used = 4;
        assert!(
            plan_synthesis_fallback(&p, &synthesis, &failure, &registry(), &capacity(), 10)
                .is_err()
        );
        failure.total_attempts_used = 3;
        assert!(
            matches!(plan_synthesis_fallback(&p,&synthesis,&failure,&registry(),&capacity(),10).expect("fallback"),SynthesisDecision::Ready{unit} if unit.model_id=="fallback-model"&&!unit.may_publish&&unit.requires_fresh_original_verification)
        );
        failure.reason = FailureReason::SetupError;
        assert!(matches!(
            plan_synthesis_fallback(&p, &synthesis, &failure, &registry(), &capacity(), 10)
                .expect("setup"),
            SynthesisDecision::Waiting {
                reason: StrategyWait::SetupOrInput
            }
        ));
        failure.reason = FailureReason::TestFail;
        failure.total_attempts_used = 4;
        failure.synthesis_attempts_by_tier.insert(0, 2);
        assert!(matches!(
            plan_synthesis_fallback(&p, &synthesis, &failure, &registry(), &capacity(), 10)
                .expect("budget"),
            SynthesisDecision::Waiting {
                reason: StrategyWait::AttemptBudget
            }
        ));
    }
}
