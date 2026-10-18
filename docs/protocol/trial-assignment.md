# Trial assignment and decision protocol

`TrialSpec` fixes the complete root-to-cluster cohort and original contract revision/digest before assignment. Digests are complete lowercase SHA-256 references. Seeded scoped cluster ranking plus a seeded arm rotation yields arm cluster counts differing by at most one. Validation recomputes the exact assignments and rejects omitted roots or substituted whole-cluster arms. Unit IDs hash the full immutable specification and scoped member. Typed inputs use streaming JSON bounds.

Analysis has an explicit recorded tick: outcomes must occur after assignment and no later than analysis. Any immature outcome, missing all-in analysis cost, unknown feature-extraction cost or absent control arm prevents a promotable proposal. The typed independent analyst receipt binds analysis time and an explicit recommendation: rejection, insufficient evidence or independent review. Receipt presence alone is insufficient, and rollback proposals must name recorded bundles. Receipt authenticity and statistical validity remain external; every promotion authorization stays false.

This pure component describes a bounded, replayable trial. It does not execute
work, modify active bundles, grant evaluator authority, or accept a candidate.

## Frozen inputs

`TrialSpec` binds the project/root/task scope, immutable bundle digests, protected
oracle digest, preregistered seed, decision tick, explicit cluster-to-variant
map, maturity threshold, and planned look count. Counters and ticks serialize
as canonical decimal strings. Serialized inputs reject unknown fields. The
entire evidence object is bounded to 1 MiB, the cohort to 1,024 roots, and
feature manifests to 256.

`assign_trial` uses only the supplied seed and each root's explicit cluster.
Sorting makes output independent of member delivery order. Clusters share a common preregistered arm list and receive a balanced seeded allocation; every
member of a cluster receives the same arm. The caller supplies exact integer propensity numerator and denominator
with `0 < numerator <= denominator`; floating-point propensity is not used.
Assignment unit IDs bind the full specification and scoped member. Duplicate roots, clusters,
units, or conflicting cluster arms fail closed.

Feature manifests retain digest, availability tick, decision phase, missingness,
and optional extraction cost. A feature available after the decision tick, or a
continuation-phase feature supplied to the initial assignment, invalidates the
evidence. Missing or unknown values remain explicit.

## Validation and proposals

Validation binds analysis evidence to the original bundle and protected-oracle
digests. The assigned roots are the fixed cohort: failed, cancelled, rework,
immature, and missing outcomes cannot be silently removed. Held-out
contamination, immature or missing labels, invalid propensity, and peeks beyond
the preregistered look count prevent a promotable proposal.

An independent analysis receipt must identify its analyst and digest the exact
trial, assignments, outcomes, features, and look count. Without this receipt,
or when it binds different bytes, the result is `insufficient_evidence`. A
complete result follows the independent analysis recommendation as a review, rejection or insufficient-evidence proposal; `promotion_authorized` is
always false. The proposal contains a rollback suggestion for future admissions
only. This component does not infer statistical benefit from example outcomes,
promote bundles, alter evaluators, or affect active work.
