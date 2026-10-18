//! Pure cumulative-usage normalization with immutable session/segment identity.
//!
//! An opening cumulative value is left-censored unless the provider supplies an
//! explicit reset baseline. In that case the first delta is measured from the
//! baseline. Missing readings stay unknown; they are never treated as zero.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CounterKind {
    Input,
    CacheRead,
    CacheWrite,
    Output,
    Reasoning,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CumulativeUsage {
    pub session_id: String,
    pub segment_id: String,
    pub source_event_id: String,
    pub model: String,
    pub counter_kind: CounterKind,
    pub cumulative_value: Option<u64>,
    /// Known counter value immediately before this segment began.
    pub reset_baseline: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsageLimits {
    pub max_sessions: usize,
    pub max_segments: usize,
    pub max_source_events: usize,
}

impl UsageLimits {
    pub const fn new(max_sessions: usize, max_segments: usize, max_source_events: usize) -> Self {
        Self {
            max_sessions,
            max_segments,
            max_source_events,
        }
    }

    fn validate(self) -> Result<Self, UsageError> {
        if self.max_sessions == 0 {
            return Err(UsageError::InvalidCapacity("sessions"));
        }
        if self.max_segments == 0 {
            return Err(UsageError::InvalidCapacity("segments"));
        }
        if self.max_source_events == 0 {
            return Err(UsageError::InvalidCapacity("source events"));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CounterTotals {
    /// Latest reported cumulative value; missing observations retain the last known value.
    pub cumulative_value: Option<u64>,
    /// Sum from a known zero/reset baseline. Remains unknown for a left-censored segment.
    pub total: Option<u64>,
}

impl CounterTotals {
    pub fn cumulative_decimal(self) -> Option<String> {
        self.cumulative_value.map(|value| value.to_string())
    }

    pub fn total_decimal(self) -> Option<String> {
        self.total.map(|value| value.to_string())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentTotals {
    /// Model is pinned when the segment is created and cannot change in-place.
    pub model: String,
    counters: BTreeMap<CounterKind, CounterTotals>,
}

impl SegmentTotals {
    pub fn counter(&self, kind: CounterKind) -> Option<CounterTotals> {
        self.counters.get(&kind).copied()
    }

    /// Counter categories observed in this segment, in stable enum order.
    pub fn counter_kinds(&self) -> impl Iterator<Item = CounterKind> + '_ {
        self.counters.keys().copied()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageDelta {
    pub session_id: String,
    pub segment_id: String,
    pub source_event_id: String,
    pub counter_kind: CounterKind,
    /// `None` means either an unknown observation or an unknown opening balance.
    /// Check `duplicate` to distinguish an idempotent replay.
    pub delta: Option<u64>,
    /// Fully known segment total, if a zero/reset baseline has established one.
    pub segment_total: Option<u64>,
    pub duplicate: bool,
}

impl UsageDelta {
    /// Decimal-string representation suitable for JavaScript/JSON consumers.
    pub fn delta_decimal(&self) -> Option<String> {
        self.delta.map(|value| value.to_string())
    }

    /// Decimal-string representation suitable for JavaScript/JSON consumers.
    pub fn segment_total_decimal(&self) -> Option<String> {
        self.segment_total.map(|value| value.to_string())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapacityKind {
    Sessions,
    Segments,
    SourceEvents,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UsageError {
    InvalidCapacity(&'static str),
    EmptyIdentity(&'static str),
    CapacityExceeded(CapacityKind),
    EventIdCollision,
    ModelChanged { segment_id: String },
    CounterDecreased { previous: u64, observed: u64 },
    ResetBaselineAboveValue { baseline: u64, observed: u64 },
    ResetBaselineNotAllowed,
    Overflow,
}

impl fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCapacity(name) => write!(formatter, "{name} capacity must be positive"),
            Self::EmptyIdentity(name) => write!(
                formatter,
                "{name} must be nonblank, control-free and at most 256 bytes"
            ),
            Self::CapacityExceeded(kind) => write!(formatter, "{kind:?} capacity exceeded"),
            Self::EventIdCollision => {
                formatter.write_str("source event ID was reused with changed payload")
            }
            Self::ModelChanged { segment_id } => {
                write!(formatter, "model changed inside segment {segment_id}")
            }
            Self::CounterDecreased { previous, observed } => {
                write!(
                    formatter,
                    "cumulative counter decreased from {previous} to {observed}"
                )
            }
            Self::ResetBaselineAboveValue { baseline, observed } => {
                write!(
                    formatter,
                    "reset baseline {baseline} exceeds observed value {observed}"
                )
            }
            Self::ResetBaselineNotAllowed => {
                formatter.write_str("reset baseline is only valid on a counter's first known value")
            }
            Self::Overflow => formatter.write_str("usage total overflowed u64"),
        }
    }
}

impl std::error::Error for UsageError {}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct EventKey {
    session_id: String,
    source_event_id: String,
}

#[derive(Default)]
struct SessionState {
    segments: BTreeMap<String, SegmentState>,
}

struct SegmentState {
    model: String,
    counters: BTreeMap<CounterKind, CounterState>,
}

#[derive(Clone, Copy, Default)]
struct CounterState {
    latest_cumulative: Option<u64>,
    known_total: Option<u64>,
}

pub struct UsageAccumulator {
    limits: UsageLimits,
    sessions: BTreeMap<String, SessionState>,
    segment_count: usize,
    events: BTreeMap<EventKey, CumulativeUsage>,
}

impl UsageAccumulator {
    pub fn new(limits: UsageLimits) -> Result<Self, UsageError> {
        Ok(Self {
            limits: limits.validate()?,
            sessions: BTreeMap::new(),
            segment_count: 0,
            events: BTreeMap::new(),
        })
    }

    pub fn observe(&mut self, usage: CumulativeUsage) -> Result<UsageDelta, UsageError> {
        validate_identity("session ID", &usage.session_id)?;
        validate_identity("segment ID", &usage.segment_id)?;
        validate_identity("source event ID", &usage.source_event_id)?;
        validate_identity("model", &usage.model)?;
        if usage.cumulative_value.is_none() && usage.reset_baseline.is_some() {
            return Err(UsageError::ResetBaselineNotAllowed);
        }

        let event_key = EventKey {
            session_id: usage.session_id.clone(),
            source_event_id: usage.source_event_id.clone(),
        };
        if let Some(previous) = self.events.get(&event_key) {
            if previous != &usage {
                return Err(UsageError::EventIdCollision);
            }
            let totals = self.segment(&usage.session_id, &usage.segment_id);
            let counter = totals.and_then(|segment| segment.counter(usage.counter_kind));
            return Ok(UsageDelta {
                session_id: usage.session_id,
                segment_id: usage.segment_id,
                source_event_id: usage.source_event_id,
                counter_kind: usage.counter_kind,
                delta: None,
                segment_total: counter.and_then(|value| value.total),
                duplicate: true,
            });
        }

        let existing_segment = self.segment_state(&usage.session_id, &usage.segment_id);
        if let Some(segment) = existing_segment
            && segment.model != usage.model
        {
            return Err(UsageError::ModelChanged {
                segment_id: usage.segment_id,
            });
        }

        let prior_counter = existing_segment
            .and_then(|segment| segment.counters.get(&usage.counter_kind))
            .copied()
            .unwrap_or_default();
        let (delta, next_counter) = next_counter(prior_counter, &usage)?;

        let new_session = !self.sessions.contains_key(&usage.session_id);
        let new_segment = existing_segment.is_none();
        if new_session && self.sessions.len() == self.limits.max_sessions {
            return Err(UsageError::CapacityExceeded(CapacityKind::Sessions));
        }
        if new_segment && self.segment_count == self.limits.max_segments {
            return Err(UsageError::CapacityExceeded(CapacityKind::Segments));
        }
        if self.events.len() == self.limits.max_source_events {
            return Err(UsageError::CapacityExceeded(CapacityKind::SourceEvents));
        }

        let session = self.sessions.entry(usage.session_id.clone()).or_default();
        let segment = session
            .segments
            .entry(usage.segment_id.clone())
            .or_insert_with(|| SegmentState {
                model: usage.model.clone(),
                counters: BTreeMap::new(),
            });
        segment.counters.insert(usage.counter_kind, next_counter);
        if new_segment {
            self.segment_count += 1;
        }
        self.events.insert(event_key, usage.clone());

        Ok(UsageDelta {
            session_id: usage.session_id,
            segment_id: usage.segment_id,
            source_event_id: usage.source_event_id,
            counter_kind: usage.counter_kind,
            delta,
            segment_total: next_counter.known_total,
            duplicate: false,
        })
    }

    pub fn segment(&self, session_id: &str, segment_id: &str) -> Option<SegmentTotals> {
        let segment = self.segment_state(session_id, segment_id)?;
        Some(SegmentTotals {
            model: segment.model.clone(),
            counters: segment
                .counters
                .iter()
                .map(|(kind, value)| {
                    (
                        *kind,
                        CounterTotals {
                            cumulative_value: value.latest_cumulative,
                            total: value.known_total,
                        },
                    )
                })
                .collect(),
        })
    }

    fn segment_state(&self, session_id: &str, segment_id: &str) -> Option<&SegmentState> {
        self.sessions.get(session_id)?.segments.get(segment_id)
    }
}

fn validate_identity(name: &'static str, value: &str) -> Result<(), UsageError> {
    if value.trim().is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(UsageError::EmptyIdentity(name));
    }
    Ok(())
}

fn next_counter(
    previous: CounterState,
    usage: &CumulativeUsage,
) -> Result<(Option<u64>, CounterState), UsageError> {
    let Some(observed) = usage.cumulative_value else {
        return Ok((None, previous));
    };

    let (delta, known_total) = match previous.latest_cumulative {
        Some(prior) => {
            if usage.reset_baseline.is_some() {
                return Err(UsageError::ResetBaselineNotAllowed);
            }
            if observed < prior {
                return Err(UsageError::CounterDecreased {
                    previous: prior,
                    observed,
                });
            }
            let delta = observed - prior;
            let total = match previous.known_total {
                Some(total) => Some(total.checked_add(delta).ok_or(UsageError::Overflow)?),
                None => None,
            };
            (Some(delta), total)
        }
        None => match usage.reset_baseline {
            Some(baseline) if baseline <= observed => {
                let delta = observed - baseline;
                (Some(delta), Some(delta))
            }
            Some(baseline) => {
                return Err(UsageError::ResetBaselineAboveValue { baseline, observed });
            }
            None => (None, None),
        },
    };
    Ok((
        delta,
        CounterState {
            latest_cumulative: Some(observed),
            known_total,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        CapacityKind, CounterKind, CounterState, CumulativeUsage, UsageAccumulator, UsageError,
        UsageLimits, next_counter,
    };

    fn accumulator() -> UsageAccumulator {
        UsageAccumulator::new(UsageLimits::new(4, 8, 32)).expect("usage fixture operation succeeds")
    }

    fn usage(
        event: &str,
        segment: &str,
        model: &str,
        kind: CounterKind,
        value: Option<u64>,
        baseline: Option<u64>,
    ) -> CumulativeUsage {
        CumulativeUsage {
            session_id: "session-1".into(),
            segment_id: segment.into(),
            source_event_id: event.into(),
            model: model.into(),
            counter_kind: kind,
            cumulative_value: value,
            reset_baseline: baseline,
        }
    }

    #[test]
    fn identical_event_is_deduplicated_and_changed_reuse_is_rejected() {
        let mut accumulator = accumulator();
        let first = usage(
            "event-1",
            "segment-1",
            "model-a",
            CounterKind::Input,
            Some(12),
            Some(10),
        );
        assert_eq!(
            accumulator
                .observe(first.clone())
                .expect("usage fixture operation succeeds")
                .delta,
            Some(2)
        );
        let replay = accumulator
            .observe(first)
            .expect("usage fixture operation succeeds");
        assert!(replay.duplicate);
        assert_eq!(replay.delta, None);
        let changed = usage(
            "event-1",
            "segment-1",
            "model-a",
            CounterKind::Input,
            Some(13),
            Some(10),
        );
        assert_eq!(
            accumulator.observe(changed),
            Err(UsageError::EventIdCollision)
        );
    }

    #[test]
    fn explicit_reset_baseline_opens_a_segment_and_new_segment_resets_counters() {
        let mut accumulator = accumulator();
        let first = accumulator
            .observe(usage(
                "e1",
                "s1",
                "m1",
                CounterKind::Output,
                Some(50),
                Some(40),
            ))
            .expect("usage fixture operation succeeds");
        assert_eq!(first.delta, Some(10));
        assert_eq!(first.segment_total, Some(10));
        assert!(matches!(
            accumulator.observe(usage("e2", "s1", "m1", CounterKind::Output, Some(1), None)),
            Err(UsageError::CounterDecreased { .. })
        ));
        let next = accumulator
            .observe(usage(
                "e3",
                "s2",
                "m1",
                CounterKind::Output,
                Some(1),
                Some(0),
            ))
            .expect("usage fixture operation succeeds");
        assert_eq!(next.delta, Some(1));
    }

    #[test]
    fn model_is_immutable_within_a_segment() {
        let mut accumulator = accumulator();
        accumulator
            .observe(usage(
                "e1",
                "s1",
                "model-a",
                CounterKind::Input,
                Some(1),
                Some(0),
            ))
            .expect("usage fixture operation succeeds");
        assert!(matches!(
            accumulator.observe(usage(
                "e2",
                "s1",
                "model-b",
                CounterKind::Input,
                Some(2),
                None
            )),
            Err(UsageError::ModelChanged { .. })
        ));
    }

    #[test]
    fn unknown_opening_balance_stays_unknown_until_a_new_segment() {
        let mut accumulator = accumulator();
        let unknown = accumulator
            .observe(usage("e1", "s1", "m1", CounterKind::Input, None, None))
            .expect("usage fixture operation succeeds");
        assert_eq!(unknown.delta, None);
        let opening = accumulator
            .observe(usage("e2", "s1", "m1", CounterKind::Input, Some(10), None))
            .expect("usage fixture operation succeeds");
        assert_eq!(opening.delta, None);
        let increment = accumulator
            .observe(usage("e3", "s1", "m1", CounterKind::Input, Some(13), None))
            .expect("usage fixture operation succeeds");
        assert_eq!(increment.delta, Some(3));
        assert_eq!(increment.segment_total, None);
    }

    #[test]
    fn checked_total_rejects_overflow() {
        // Exercise checked-add directly; valid monotonic observations cannot
        // normally exceed their u64 cumulative counter.
        let previous = CounterState {
            latest_cumulative: Some(0),
            known_total: Some(u64::MAX),
        };
        let observation = usage("e2", "s1", "m1", CounterKind::Output, Some(1), None);
        assert!(matches!(
            next_counter(previous, &observation),
            Err(UsageError::Overflow)
        ));
    }

    #[test]
    fn limits_are_positive_and_capacity_exhaustion_is_explicit() {
        assert_eq!(
            UsageAccumulator::new(UsageLimits::new(0, 1, 1)).err(),
            Some(UsageError::InvalidCapacity("sessions"))
        );
        let mut accumulator = UsageAccumulator::new(UsageLimits::new(1, 1, 1))
            .expect("usage fixture operation succeeds");
        accumulator
            .observe(usage("e1", "s1", "m1", CounterKind::Input, None, None))
            .expect("usage fixture operation succeeds");
        assert_eq!(
            accumulator.observe(usage("e2", "s1", "m1", CounterKind::Input, None, None)),
            Err(UsageError::CapacityExceeded(CapacityKind::SourceEvents))
        );
        assert_eq!(
            accumulator.observe(usage("e2", "s2", "m1", CounterKind::Input, None, None)),
            Err(UsageError::CapacityExceeded(CapacityKind::Segments))
        );
        assert_eq!(
            accumulator.observe(CumulativeUsage {
                session_id: "session-2".into(),
                ..usage("e2", "s1", "m1", CounterKind::Input, None, None)
            }),
            Err(UsageError::CapacityExceeded(CapacityKind::Sessions))
        );
    }

    #[test]
    fn cache_read_and_write_are_separate_and_large_values_have_string_forms() {
        let mut accumulator = accumulator();
        accumulator
            .observe(usage(
                "e1",
                "s1",
                "m1",
                CounterKind::CacheRead,
                Some(u64::MAX),
                Some(0),
            ))
            .expect("usage fixture operation succeeds");
        accumulator
            .observe(usage(
                "e2",
                "s1",
                "m1",
                CounterKind::CacheWrite,
                Some(7),
                Some(0),
            ))
            .expect("usage fixture operation succeeds");
        let segment = accumulator
            .segment("session-1", "s1")
            .expect("usage fixture operation succeeds");
        assert_eq!(
            segment.counter_kinds().collect::<Vec<_>>(),
            vec![CounterKind::CacheRead, CounterKind::CacheWrite]
        );
        assert_eq!(
            segment
                .counter(CounterKind::CacheRead)
                .expect("usage fixture operation succeeds")
                .total_decimal()
                .as_deref(),
            Some("18446744073709551615")
        );
        assert_eq!(
            segment
                .counter(CounterKind::CacheWrite)
                .expect("usage fixture operation succeeds")
                .total,
            Some(7)
        );
    }
}
