# Legacy metrics and import boundaries

Source pin: `1e8ed17eff27ed3ab80dc6860fc8eda301406aee`. Exact SHA-256,
bytes, row counts, duplicate counts and observed source time bounds live in
[`source-manifest.json`](../../tests/fixtures/legacy/source-manifest.json).
This dictionary records observed source semantics, including limitations. It
does not turn historical observations into trusted task events or certify the
old metric formulas.

| Metric / input | Legacy meaning and unit | Import and display contract |
| --- | --- | --- |
| `model_usage` ledger | One routing/outcome event; 503 JSONL records, CSV mirror | Count source once. `started`, judged result and escalation can describe one attempt; no independently established exact attempt or accepted-task count. Preserve source format and provenance. |
| Older 449-row ledger | Claimed by plans pinned to `9dbbec4451008c73ec1c203fa56ad467abc18d56` | Older object unavailable locally. Hashes/time bounds unverified; no arithmetic comparisons or union with current ledger without verified source identity. |
| `luna_fail_total` | Cumulative events on a Luna/T0/T1 route with fail/loop/timeout/escalated outcome | Legacy routing heuristic. Escalation plus failure can describe one episode. Never infer distinct failed tasks or causal model capability. |
| `luna_fp_rate` | First judged event per task/project, pass share (%) | Self-reported selected ledger cohort; early pre-unification values counted `started` and are not comparable. Do not present as independently verified acceptance. |
| Codex session `input`, `cached`, `output`, `reasoning` | Lifetime counters per session; later export replaces earlier snapshot | Input includes cached input; reasoning is a subset of output when reported, not extra billable output. One cumulative row is not window consumption. Need opening balance, reset handling and native semantic qualification. |
| Claude hourly usage | Deduplicated message usage aggregated by hour/project/model | Legacy `cached` collapses cache-read and cache-creation categories. Keep aggregate as imported observation; cannot reconstruct native categories or exact pricing from it. May overlap session/yield snapshots. |
| `credits_est` | Published Codex price estimate, native credit units | Historical rate version required. Missing price remains unavailable; preserve known subtotal and priced coverage. Never coerce missing credits to zero or equate credits with USD/quota. |
| `credits_h` / window burn | Estimated credits/hour from recent cumulative deltas | Distinguish exact opening balance, absent baseline and resets. Legacy latest-model attribution can misprice mixed-model sessions. Unknown-priced segments require visible gaps. |
| `cost_index` | Cost of same recent Codex tokens versus an all-Luna baseline; fallback is live-agent output-rate average | Preserve `cost_basis`, window and rate source. Token-weighted and agent-count-weighted series differ. Not actual plan-quota multipliers; excludes unknown providers. |
| `quota_pct`, credit balance | Latest provider-reported account-window observation | Shared account activity, not task charges. Preserve observed timestamp and reset/window metadata. Avoid attributing all account activity to managed workers. |
| `ty.tokens`, `ty.output` | Full lifetime session counters selected by recent session end time | Legacy window leakage: selecting an active session does not isolate its recent usage. Keep as historical heuristic; corrected native window metrics require segment deltas. |
| `ty.integration_tokens`, `ty.main_tokens` | Entire session counted landed if any eligible commit lands by ancestry or patch identity | Heuristic landing attribution, not independently verified accepted work. Partial success receives whole-session attribution; preserve method and reference freshness. |
| `ty.coverage_pct` | Attributed token numerator / total tokens (%) | Project and tier aggregation must both preserve numerator. Legacy tier accumulator can lack `attributed_tokens`; an absent numerator is unknown, not zero observed coverage. |
| `credits_per_done_todo` | Estimated selected session credits divided by current mutable done count | Non-cohort heuristic; changed task scope and incomplete costs make historical comparisons unsafe. New metric uses fixed assignment cohorts including failed/reopened roots and manager costs. |
| Project `done`, `counts` | Heterogeneous ledger/claim/assignment observations | Preserve source project/revision/worktree. Separate reported progress, delivery stage, verification and ownership. A claim, commit, PR or green CI alone does not establish acceptance. |
| Seven `stages` | `done`, `on_main`, `in_pr`, `implemented`, `in_progress`, `blocked`, `not_started` | Legacy evidence promotion and project exceptions differ. Stage counts sum to plan total, but migration must not upgrade their trust automatically. |
| `ci`, PR head/review | Cached forge observation, often against integration branch | Preserve exact head and fetch time. A reviewed older commit or PASS from another checkout cannot authorize current-head acceptance/publication. |
| `hygiene` | 0–100 heuristic for stale/dirty/behind/temp assets | Signal for inspection, not cleanup authority. Foreign/unregistered assets remain inventory-only. |
| `tiers`, model mix, screen state | Detected process/footer/default/log hints | Preserve detection source and unknowns. Process names and model defaults do not prove actual model execution or native usage. |
| Resource load | 1-minute host load divided by CPU count (%) | Observational, not CPU utilization; distinguish source-reported CPU count and timestamp. No universal admission threshold inferred from current private policy. |
| Disk used %, bytes and growth | Filesystem usage and rolling difference/hour | Preserve mount/host/time; stale or sparse observations do not prove free capacity. Never make discovered storage disposable automatically. |
| Screen hardcopy text | Captured terminal output | Text is untrusted observation. Require successful fresh capture bound to current process identity; quoted approvals and idle-looking text never grant input authority. |

Current export contains 109 history snapshots, 362 quota samples, 1,072 disk
samples, 12 host snapshots, 179 provider session rows, 253 Claude hourly rows,
five project snapshots and three token-yield reports. Their different grains
must not be summed together. JSONL and CSV ledger/history representations mirror
the same records; exact duplicate counts and timestamps are manifest facts.

The exporter union-merges rolling inputs and normalizes legacy `utc`, `to_tier`
and `note` to `ts`, `escalated_to` and `reason`. A re-import is idempotent only
within a pinned source identity and documented record identity. Changed content
under an existing identity requires explicit correction lineage, not silent
replacement.

The 11 [synthetic regression cases](../../tests/fixtures/legacy/README.md)
exercise the boundaries above without private transcripts or fabricated exact
attempt/acceptance counts. Reference fixture tests establish regression inputs;
runtime packets still need tests against their actual importer, accounting,
provider and acceptance implementations.
