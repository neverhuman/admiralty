//! Bounded, explicitly composed trial registry. Callers authenticate observations
//! and implement independent review authority; no ambient startup grant exists.
use admiralty_core::{PrincipalId, ProjectId, evaluation::*, limits::bounded_json_size, trials::*};
use std::collections::{BTreeMap, BTreeSet};

const MAX_BUNDLES: usize = 128;
const MAX_TRIALS: usize = 128;
const MAX_DATASET_BYTES: usize = 2 * 1024 * 1024;

/// Trusted application adapter: authenticate the reviewer and exact dossier,
/// including configured authority/evaluator review when those digests change.
pub trait ReviewAuthority {
    fn authorize(&self, request: &ReviewRequest) -> bool;
}

#[derive(Debug)]
pub struct ReviewRequest {
    pub project_id: ProjectId,
    pub trial_id: TrialId,
    pub reviewer: PrincipalId,
    pub dossier_digest: String,
    pub verdict: Verdict,
    pub authority_changed: bool,
    pub evaluator_changed: bool,
    pub rollback: bool,
    pub expected_alias_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AdmissionPin {
    project_id: ProjectId,
    bundle_digest: String,
    revision: u64,
}

impl AdmissionPin {
    pub fn bundle_digest(&self) -> &str {
        &self.bundle_digest
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

#[derive(Default)]
pub struct ExperimentService {
    bundles: BTreeMap<TrialId, (String, Bundle)>,
    registrations: BTreeMap<TrialId, TrialRegistration>,
    datasets: BTreeMap<TrialId, TrialDataset>,
    aliases: BTreeMap<ProjectId, AdmissionPin>,
}

fn invalid(message: &str) -> TrialError {
    TrialError::Invalid(message.into())
}

impl ExperimentService {
    pub fn register_bundle(&mut self, bundle: Bundle) -> Result<String, TrialError> {
        let digest = bundle_digest(&bundle)?;
        if let Some((previous, _)) = self.bundles.get(&bundle.id) {
            return if previous == &digest {
                Ok(digest)
            } else {
                Err(TrialError::Collision("immutable bundle ID reused".into()))
            };
        }
        if self.bundles.len() >= MAX_BUNDLES {
            return Err(TrialError::TooLarge("bundle registry full".into()));
        }
        self.bundles
            .insert(bundle.id.clone(), (digest.clone(), bundle));
        Ok(digest)
    }

    fn bundle(&self, digest: &str) -> Result<&Bundle, TrialError> {
        self.bundles
            .values()
            .find(|(d, _)| d == digest)
            .map(|(_, b)| b)
            .ok_or_else(|| invalid("bundle is not registered"))
    }

    pub fn register_trial(
        &mut self,
        registration: TrialRegistration,
    ) -> Result<Vec<Assignment>, TrialError> {
        bounded_json_size(&registration, MAX_DATASET_BYTES)
            .map_err(|_| TrialError::TooLarge("registration exceeds 2 MiB".into()))?;
        let spec = &registration.spec;
        let assignments = assign_trial(spec, &spec.fixed_cohort, 1, 2)?;
        validate_trial(&TrialEvidence {
            trial: spec.clone(),
            bundle_digests_at_analysis: spec.immutable_bundle_digests.clone(),
            protected_oracle_digest_at_analysis: spec.protected_oracle_digest.clone(),
            features: registration.features.clone(),
            assignments: assignments.clone(),
            outcomes: vec![],
            look_count: DecimalU64::new(0),
            analysis_tick: spec.decision_tick.clone(),
            independent_analysis: None,
        })?;
        if let Some(previous) = self.registrations.get(&spec.trial_id) {
            return if previous == &registration {
                Ok(assignments)
            } else {
                Err(TrialError::Collision("immutable trial ID reused".into()))
            };
        }
        if self.registrations.len() >= MAX_TRIALS {
            return Err(TrialError::TooLarge("trial registry full".into()));
        }
        self.bundle(&registration.champion)?;
        self.bundle(&registration.candidate)?;
        if registration.champion == registration.candidate
            || spec.immutable_bundle_digests
                != [
                    registration.champion.clone(),
                    registration.candidate.clone(),
                ]
            || registration.managers.len() != spec.fixed_cohort.len()
        {
            return Err(invalid(
                "trial requires exact champion, candidate and manager cohort",
            ));
        }
        let mut managers = BTreeMap::new();
        for member in &spec.fixed_cohort {
            let manager = registration
                .managers
                .get(&member.root_id)
                .ok_or_else(|| invalid("manager missing from fixed cohort"))?;
            if let Some(cluster) = managers.insert(manager, &member.cluster_id)
                && cluster != &member.cluster_id
            {
                return Err(invalid("shared manager crosses assignment clusters"));
            }
        }
        // One fresh/held-out comparison per candidate and cohort. Replays cannot
        // be relabelled as independent new comparisons or bypass stopping rules.
        if registration.mode != TrialMode::Replay
            && self.registrations.values().any(|previous| {
                previous.mode != TrialMode::Replay
                    && previous.spec.project_id == spec.project_id
                    && (previous.candidate == registration.candidate
                        || previous
                            .spec
                            .fixed_cohort
                            .iter()
                            .any(|m| registration.managers.contains_key(&m.root_id)))
            })
        {
            return Err(invalid(
                "candidate or cohort already used in a controlled comparison",
            ));
        }
        if let Some(alias) = self.aliases.get(&spec.project_id)
            && alias.bundle_digest != registration.champion
        {
            return Err(invalid("champion differs from current admission alias"));
        }
        self.aliases
            .entry(spec.project_id.clone())
            .or_insert_with(|| AdmissionPin {
                project_id: spec.project_id.clone(),
                bundle_digest: registration.champion.clone(),
                revision: 0,
            });
        self.registrations
            .insert(spec.trial_id.clone(), registration);
        Ok(assignments)
    }

    pub fn record(
        &mut self,
        evidence: TrialEvidence,
        mut executions: Vec<ExecutionRecord>,
    ) -> Result<&TrialDataset, TrialError> {
        bounded_json_size(&(&evidence, &executions), MAX_DATASET_BYTES)
            .map_err(|_| TrialError::TooLarge("dataset input exceeds 2 MiB".into()))?;
        validate_trial(&evidence)?;
        let registration = self
            .registrations
            .get(&evidence.trial.trial_id)
            .ok_or_else(|| invalid("trial must be registered before observing outcomes"))?
            .clone();
        if registration.spec != evidence.trial || registration.features != evidence.features {
            return Err(invalid("registered trial drift"));
        }
        executions.sort_by(|a, b| a.root_id.cmp(&b.root_id));
        if executions.len() != evidence.assignments.len()
            || executions
                .iter()
                .map(|x| &x.root_id)
                .collect::<BTreeSet<_>>()
                .len()
                != executions.len()
        {
            return Err(invalid(
                "execution records must cover the complete fixed cohort",
            ));
        }
        let arms = &registration.spec.cluster_assignments[0].variants;
        let mut total = Some(0_u64);
        for execution in &executions {
            let assignment = evidence
                .assignments
                .iter()
                .find(|a| a.root_id == execution.root_id)
                .ok_or_else(|| invalid("execution root is outside the fixed cohort"))?;
            validate_digest(&execution.execution_digest)?;
            if let Some(digest) = &execution.usage_digest {
                validate_digest(digest)?;
            }
            let bundle = if assignment.variant == arms[0] {
                &registration.champion
            } else {
                &registration.candidate
            };
            if execution.root_id != assignment.root_id || &execution.bundle_digest != bundle {
                return Err(invalid(
                    "actual execution differs from preregistered assignment",
                ));
            }
            let outcome = evidence
                .outcomes
                .iter()
                .find(|o| o.root_id == execution.root_id);
            if let (Some(cost), Some(value)) = (
                execution.cost(),
                outcome.and_then(|o| o.analysis_value_micros.as_ref()),
            ) && cost != value.get()
            {
                return Err(invalid("outcome cost differs from actual execution costs"));
            }
            total = total
                .zip(execution.cost())
                .and_then(|(a, b)| a.checked_add(b));
            if execution.usage_digest.is_none() {
                total = None;
            }
        }
        for feature in &evidence.features {
            total = total
                .zip(feature.extraction_cost_micros.as_ref())
                .and_then(|(a, b)| a.checked_add(b.get()));
        }
        let mut ineligibility = Vec::new();
        if registration.mode == TrialMode::Replay {
            ineligibility.push("replay is not fresh independent evidence".into());
        }
        if registration.spec.planned_looks.get() != 1 {
            ineligibility.push("multiplicity-adjusted analysis is not configured".into());
        }
        if evidence.features.iter().any(|f| {
            evidence.trial.decision_tick.get() - f.available_tick.get()
                > registration.max_feature_age_ticks.get()
                || f.missingness != Missingness::Known
        }) {
            ineligibility.push("stale or missing decision feature".into());
        }
        if total.is_none() {
            ineligibility.push("unknown usage, all-in cost or cost overflow".into());
        }
        let analyst = evidence.independent_analysis.as_ref().map(|a| &a.analyst);
        if analyst.is_none_or(|a| {
            a == &registration.owner || registration.managers.values().any(|m| m == a)
        }) {
            ineligibility.push("analysis is not independent of the owner and managers".into());
        }
        // A learned router requires measured benefit with a separately configured
        // shadow/canary gate. Ordinary trial review cannot enable it.
        if self.bundle(&registration.candidate)?.learned_routing {
            ineligibility.push("learned routing benefit gate is not configured".into());
        }
        let proposal = propose_trial_decision(&evidence, vec![registration.champion.clone()])?;
        let dataset = TrialDataset {
            schema_version: 1,
            bundles: vec![
                self.bundle(&registration.champion)?.clone(),
                self.bundle(&registration.candidate)?.clone(),
            ],
            registration,
            evidence,
            executions,
            all_in_cost_micros: total.map(DecimalU64::new),
            proposal,
            ineligibility,
            decision: None,
            rollback_review: None,
        };
        bounded_json_size(&dataset, MAX_DATASET_BYTES)
            .map_err(|_| TrialError::TooLarge("dataset exceeds 2 MiB".into()))?;
        let id = dataset.registration.spec.trial_id.clone();
        if let Some(previous) = self.datasets.get(&id) {
            if previous.dossier_digest()? != dataset.dossier_digest()? {
                return Err(TrialError::Collision(
                    "trial evidence is sealed after its first look".into(),
                ));
            }
        } else {
            self.datasets.insert(id.clone(), dataset);
        }
        self.dataset(&id)
    }

    pub fn dataset(&self, id: &TrialId) -> Result<&TrialDataset, TrialError> {
        self.datasets
            .get(id)
            .ok_or_else(|| invalid("trial dataset unavailable"))
    }

    pub fn pin_new_work(&self, project: &ProjectId) -> Result<AdmissionPin, TrialError> {
        self.aliases
            .get(project)
            .cloned()
            .ok_or_else(|| invalid("admission alias unavailable"))
    }

    pub fn decide(
        &mut self,
        id: &TrialId,
        expected_revision: u64,
        reviewer: PrincipalId,
        verdict: Verdict,
        authority: &dyn ReviewAuthority,
    ) -> Result<&TrialDataset, TrialError> {
        self.apply_review(id, expected_revision, reviewer, verdict, false, authority)
    }

    pub fn rollback(
        &mut self,
        id: &TrialId,
        expected_revision: u64,
        reviewer: PrincipalId,
        authority: &dyn ReviewAuthority,
    ) -> Result<&TrialDataset, TrialError> {
        self.apply_review(
            id,
            expected_revision,
            reviewer,
            Verdict::Accept,
            true,
            authority,
        )
    }

    fn apply_review(
        &mut self,
        id: &TrialId,
        expected_revision: u64,
        reviewer: PrincipalId,
        verdict: Verdict,
        rollback: bool,
        authority: &dyn ReviewAuthority,
    ) -> Result<&TrialDataset, TrialError> {
        let dataset = self.dataset(id)?;
        let registration = &dataset.registration;
        if reviewer == registration.owner
            || registration.managers.values().any(|m| m == &reviewer)
            || dataset
                .evidence
                .independent_analysis
                .as_ref()
                .is_some_and(|a| a.analyst == reviewer)
        {
            return Err(invalid(
                "reviewer must be independent of owner, managers and analyst",
            ));
        }
        let alias = self.pin_new_work(&registration.spec.project_id)?;
        if alias.revision != expected_revision {
            return Err(invalid("stale admission alias revision"));
        }
        if rollback {
            if !dataset.decision.as_ref().is_some_and(|d| {
                d.verdict == Verdict::Accept && !d.rolled_back && d.alias_revision == alias.revision
            }) || alias.bundle_digest != registration.candidate
            {
                return Err(invalid(
                    "rollback requires this trial's current accepted candidate",
                ));
            }
        } else {
            if dataset.decision.is_some() {
                return Err(invalid("trial already decided"));
            }
            if verdict == Verdict::Accept
                && (!dataset.ineligibility.is_empty()
                    || !matches!(dataset.proposal, TrialDecision::PromotableProposal { .. })
                    || alias.bundle_digest != registration.champion)
            {
                return Err(invalid("trial evidence is not eligible for promotion"));
            }
        }
        let champion = self.bundle(&registration.champion)?;
        let candidate = self.bundle(&registration.candidate)?;
        let request = ReviewRequest {
            project_id: registration.spec.project_id.clone(),
            trial_id: id.clone(),
            reviewer: reviewer.clone(),
            dossier_digest: dataset.dossier_digest()?,
            verdict,
            authority_changed: champion.authority_digest != candidate.authority_digest,
            evaluator_changed: champion.evaluator_digest != candidate.evaluator_digest,
            rollback,
            expected_alias_revision: expected_revision,
        };
        let revision = if verdict == Verdict::Accept {
            alias
                .revision
                .checked_add(1)
                .ok_or_else(|| invalid("alias revision overflow"))?
        } else {
            alias.revision
        };
        if !authority.authorize(&request) {
            return Err(invalid("independent exact-dossier review denied"));
        }
        let next_bundle = if rollback {
            registration.champion.clone()
        } else {
            registration.candidate.clone()
        };
        if verdict == Verdict::Accept {
            self.aliases.insert(
                request.project_id.clone(),
                AdmissionPin {
                    project_id: request.project_id,
                    bundle_digest: next_bundle,
                    revision,
                },
            );
        }
        let dataset = self
            .datasets
            .get_mut(id)
            .ok_or_else(|| invalid("trial dataset unavailable"))?;
        if rollback {
            if let Some(decision) = dataset.decision.as_mut() {
                decision.rolled_back = true;
            }
            dataset.rollback_review = Some(DecisionRecord {
                reviewer,
                dossier_digest: request.dossier_digest,
                verdict,
                alias_revision: revision,
                rolled_back: true,
            });
        } else {
            dataset.decision = Some(DecisionRecord {
                reviewer,
                dossier_digest: request.dossier_digest,
                verdict,
                alias_revision: revision,
                rolled_back: false,
            });
        }
        self.dataset(id)
    }
}
