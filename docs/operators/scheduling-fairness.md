# Fair scheduling rounds

`scheduler::fairness::propose_fair_round` orders validated project queues with
weighted deficit service, starvation aging, and dependency critical paths.
It accepts up to 64 projects and 1,000 total tasks. Task classes, enqueue times,
weights, observations and prior credits are explicit inputs to the read digest.
Unknown inference or compute capacity yields no selection. Each project's
unknown quota, held ownership or blocked dependencies remain blockers.

Default finishing pools retain 20% of available slots for verification and 10%
for repair, rounding down so a single-slot pool can still make progress. Feature work cannot borrow those
slots. Unknown throughput limits a project's new feature WIP to one by default;
known throughput uses the explicitly configured feature WIP cap. These are
proposal limits, separate from current resource and budget proofs.

The default project finish-mode triggers are 20 minutes of red train age, or
60 minutes without acceptance at or above the configured minimum feature WIP.
Separate exit thresholds provide hysteresis. Unknown train/acceptance observations
hold new feature selection. Repair and verification remain eligible, and other
projects continue independently.

`services::coordinator::propose_store_round` compares every supplied task contract,
state, ownership and active feature count with the actual Store projection. It
rejects incomplete or stale queues and reported completion that Store has not
accepted. The returned Store digest can detect a later racing claim.

Selections grant no execution authority. The native admission path must still
recheck current authority, contracts, budget, inference slots, compute permits
and outbox identity in its serialized writer transaction. Persist fairness
credits only with successfully admitted work, under the expected generation and
read set. A partial or failed batch requires replanning; do not commit credits
for selections that were never admitted. This change provides proposal and Store-read composition; it installs
no production scheduler, issuer, fairness-state journal or automatic dispatch.
