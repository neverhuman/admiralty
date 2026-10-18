# Trial assignment and decision protocol

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
Sorting makes output independent of caller order. It hashes the seed with the
cluster ID to choose from that cluster's preregistered variant list; every
member of a cluster therefore receives the same arm. The caller supplies exact integer propensity numerator and denominator
with `0 < numerator <= denominator`; floating-point propensity is not used.
Assignment unit IDs bind trial, scope root, and root. Duplicate roots, clusters,
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
complete result can only be `promotable_proposal`; `promotion_authorized` is
always false. The proposal contains a rollback suggestion for future admissions
only. This component does not infer statistical benefit from example outcomes,
promote bundles, alter evaluators, or affect active work.
