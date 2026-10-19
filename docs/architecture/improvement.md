# Controlled improvement

`ExperimentService` composes the deterministic trial engine with an immutable,
bounded bundle/trial registry, sealed execution datasets, independent decisions
and admission aliases. It is an explicitly composed in-process service. The
application owns serialization, durable custody, authentication and admission
wiring; this component does not install HTTP routes, issue provider work, or
enable experiments at startup.

Register complete role prompts and policy, detector, evaluator and authority
digests before registering a trial. A bundle ID cannot acquire different bytes.
The trial freezes the original contract, protected oracle, seed, full cohort,
manager identities, mode, feature manifests, freshness budget, stopping budget,
champion and one candidate. The first preregistered arm names the champion and
the second names the candidate. Features must be available at assignment and
valid for its decision phase. Shared managers must stay in one cluster.

Replay, fresh and held-out assignments use the same reproducible engine, but
replay cannot establish fresh independent evidence. Controlled comparisons
cannot reuse a candidate or cohort within a project. Only the first evidence
look is sealed; exact retries preserve decisions and changed results fail.
Multiple planned looks require a separately configured multiplicity analysis
and cannot promote through this bounded service. Missing or stale features,
unknown usage/costs, overflow, immature labels, contamination, unexpected peeks
and owner/manager analysis prevent promotion. Original trial validation also
rejects future features, invalid propensities and assignment/oracle drift.

Each assigned root retains its actual execution and usage artifact digests,
execution/training/analysis costs and outcome, including failed, cancelled and
rework records. All-in cost also includes feature extraction. Unknown components
remain unknown; they are never silently counted as zero. Outcome cost must
match known execution costs. Digests establish identity, not authentic execution
or billing: callers must authenticate artifact custody and observations.

An analysis receipt alone cannot change an alias. `ReviewAuthority` must
authenticate an independent reviewer and authorize the exact dossier, verdict,
project, trial, alias revision and operation. The reviewer must differ from the
owner, managers and analyst. Authority/evaluator changes are explicitly passed
to this configured review policy; absent authorization fails. Tests use an
explicit policy double. This service does not infer statistical benefit from
example outcomes or let the model choose its own evaluator. Learned routing
remains ineligible without a separately measured shadow/canary benefit gate.

An accepted candidate advances the project's admission alias with an expected
revision. Existing `AdmissionPin` values retain their bundle. Independent
rollback requires the same trial's current accepted candidate and restores the
champion for new admissions, retaining both decision and rollback review.
Rejection and insufficient evidence retain the champion and their datasets.

`export_dataset` returns reproducible JSON and its SHA-256 identity. Exports
retain bundles, registration, manifests, assignments, actual execution/usage,
negative outcomes, unknowns, analysis, eligibility reasons and review records.
The immutable dossier digest excludes decisions; the export digest includes
them. Limits are 128 bundles, 128 trials, 64 KiB per bundle and 2 MiB per dataset,
in addition to the trial engine's 1,024 roots and 256 features.

The packet gate is `cargo test -p admiralty-testkit --test experiment_validity`.
It exercises real service composition and a bounded independently reviewed
accept/reject/rollback sequence with deterministic evidence. These checks do
not claim a live randomized provider trial, statistical efficacy, persistence
across service restart, production activation or a qualified learned router.
