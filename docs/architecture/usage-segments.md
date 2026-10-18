# Segmented provider usage

`UsageAccumulator` normalizes cumulative provider counters without estimating
prices or filling missing measurements. It is a pure in-memory component; the
runtime integration owns persistence, provider parsing, and event ordering.

## Identity and segment rules

Every observation identifies a session, segment, source event, model, and one
counter category. A session may contain multiple segments. A segment pins its
model and the fixed counter schema (`input`, `cache_read`, `cache_write`,
`output`, and `reasoning`). A model change or counter reset starts a new segment;
segments are never merged.

Source event IDs are unique within a session. Replaying the exact same event is
idempotent and returns `duplicate: true` with no delta. Reusing an ID with any
changed payload is an error. Deduplication identities are retained for the
accumulator lifetime, so capacity exhaustion is explicit and never evicts an
identity that could later be replayed.

## Cumulative values and unknowns

Counters are monotonic within one model segment. A decrease is rejected. The
first known cumulative value is a left-censored opening balance, so its delta
and segment total are unknown unless that observation supplies an explicit
reset baseline. With a baseline, the initial delta is `cumulative - baseline`.
Later known observations produce the difference from the previous known
cumulative value. A missing value produces no delta and does not erase an
earlier known cumulative value; it is never interpreted as zero. If the
opening balance was unknown, later increments can be reported while the full
segment total remains unknown.

Input, cache-read, cache-write, output, and reasoning counters are tracked
independently. Arithmetic uses checked `u64` operations. APIs expose decimal
string accessors for counters so callers can serialize values without losing
precision in JavaScript-number consumers. No currency, model price, or
cross-segment aggregate is inferred.

## Bounds

Construction requires positive caps for sessions, segments, and source events.
An insertion that would exceed a cap returns a typed capacity error before
changing state. The caller must start a fresh bounded accumulator or persist
and retire the old one under its own retention policy; this component does not
silently evict state.
