# Pure cohort economics component

This document describes the bounded, pure API in `admiralty_core::economics`. It is a component candidate: it does not collect provider usage, assign work, reserve budgets, determine acceptance, or make execution decisions.

## Fixed assignment and evidence

`CohortSpec` freezes a unique set of `RootId`s, the required delivery stage, assignment time, project, and valuation digest. The set is the denominator: failed, cancelled, retried, reopened, reviewed, reworked, captain, and super-captain activity remains attributable to those assignments. Splitting work beneath an assigned root does not add accepted root value. A root counts as accepted only when its current outcome is succeeded, reaches the required stage, and meets the supplied maturity cutoff. Missing or immature outcomes leave yield unavailable.

`UsageRecord` captures an observation key and revision, root, role, purpose, provider, unit, amount, and parent inclusion relation. A repeated identical `(observation_id, revision)` is idempotent; different bytes at the same key are an error. Only the newest revision for an observation is used. Inclusive parent/child observations and intersecting intervals are not added together: ambiguous contributions leave unit coverage partial and are disclosed. Unknown values remain unknown. Known partial subtotals are labeled partial and cannot support savings claims.

Units are independent buckets. For example, `USD:6`, provider quota, and token categories must never be summed. Money uses integer minor units with the scale embedded in the unit identifier; this component does not infer prices or convert currencies. `DecimalU64` and `DecimalU128` serialize only canonical decimal strings, preserving large values across JavaScript clients. Checked arithmetic reports overflow. `Rational` represents a ratio without floating point; a zero denominator has no value.

`parse_bounded_json` rejects inputs above 1 MiB. Collection inputs are capped at 1024; identifiers are bounded to 128 characters and unknown reasons to 4096. Structs reject unknown fields so schema drift is visible.

## Summary and comparison limits

`summarize_cohort` returns fixed-root counts, per-unit totals and coverage, accepted yield when fully mature, and limitations. It does not treat zero accepted roots as a zero denominator or an unknown amount as zero. The `legacy_yield_heuristic` field is deliberately separate and unset by this component.

`cohort_compare` checks project, valuation digest, required stage, mature accepted yield, and complete coverage for the requested coordination unit. It reports insufficient evidence when these conditions fail and always leaves `savings_claim` false. Even a comparable pair is not a causal experiment: cohort assignment, quality guardrails, uncertainty, costs, and latency require a higher-level preregistered evaluation. A caller must not infer coordination savings from missing usage, incompatible currency or quota units, or a lower manager share alone.

The functions are deterministic and have no clock, randomness, filesystem, provider, or process effects. Native provider usage categories and model/effort segments should remain distinct in the unit/purpose dimensions supplied by a future collector.
