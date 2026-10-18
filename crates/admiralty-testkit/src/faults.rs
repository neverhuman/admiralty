//! Deterministic, bounded fault schedules for offline callers.
//!
//! Effects are symbolic data: this module never crashes a process, drops a real
//! message, sleeps, performs I/O or decides acceptance. A test caller explicitly
//! interprets each returned effect in its own simulation. Counts describe hits
//! in this in-memory instance, not durable event identities or replay receipts.

use std::{collections::BTreeMap, error::Error, fmt};

pub const MAX_FAULT_RULES: usize = 256;
pub const MAX_FAULT_POINT_BYTES: usize = 64;
pub const MAX_FAULT_DELAY_MS: u64 = 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultEffect {
    Fail,
    Crash,
    Drop,
    Duplicate,
    DelayMs(u64),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultRule {
    pub point: String,
    /// One-based occurrence at this exact point, independent of other points.
    pub occurrence: u64,
    pub effect: FaultEffect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultError {
    TooManyRules,
    InvalidPoint,
    InvalidOccurrence,
    DelayTooLong,
    DuplicateRule,
    UnknownPoint,
    CounterOverflow,
}

impl fmt::Display for FaultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TooManyRules => "fault plan exceeds its hard rule cap",
            Self::InvalidPoint => "fault point must be bounded, nonblank, trimmed and control-free",
            Self::InvalidOccurrence => "fault occurrence must be at least one",
            Self::DelayTooLong => "symbolic fault delay exceeds its hard cap",
            Self::DuplicateRule => "fault point and occurrence already have a rule",
            Self::UnknownPoint => "fault point is not registered in this plan",
            Self::CounterOverflow => "fault point hit counter is exhausted",
        })
    }
}

impl Error for FaultError {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PointState {
    hits: u64,
    effects: BTreeMap<u64, FaultEffect>,
}

/// Rule/counter cardinality is fixed at construction. A clone copies at most
/// 256 registered rules and their bounded names; hits never allocate history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultPlan {
    points: BTreeMap<String, PointState>,
}

fn validate_point(point: &str) -> Result<(), FaultError> {
    if point.is_empty()
        || point.len() > MAX_FAULT_POINT_BYTES
        || point.trim() != point
        || point.chars().any(char::is_control)
    {
        return Err(FaultError::InvalidPoint);
    }
    Ok(())
}

impl FaultPlan {
    /// Construct an explicit schedule with no PRNG, default points or effects.
    /// Empty plans are valid; every hit on an empty plan is UnknownPoint.
    pub fn new(rules: Vec<FaultRule>) -> Result<Self, FaultError> {
        if rules.len() > MAX_FAULT_RULES {
            return Err(FaultError::TooManyRules);
        }
        let mut points = BTreeMap::<String, PointState>::new();
        for rule in rules {
            validate_point(&rule.point)?;
            if rule.occurrence == 0 {
                return Err(FaultError::InvalidOccurrence);
            }
            if matches!(rule.effect, FaultEffect::DelayMs(delay) if delay > MAX_FAULT_DELAY_MS) {
                return Err(FaultError::DelayTooLong);
            }
            let point = points.entry(rule.point).or_default();
            if point.effects.insert(rule.occurrence, rule.effect).is_some() {
                return Err(FaultError::DuplicateRule);
            }
        }
        Ok(Self { points })
    }

    /// Record exactly one hit, returning only its scheduled symbolic effect.
    /// Unknown points and exhausted counters fail without changing any state.
    /// Fired rules remain fixed; no consumed-history structure grows on a hit.
    pub fn hit(&mut self, point: &str) -> Result<Option<FaultEffect>, FaultError> {
        validate_point(point)?;
        let state = self.points.get_mut(point).ok_or(FaultError::UnknownPoint)?;
        let next = state
            .hits
            .checked_add(1)
            .ok_or(FaultError::CounterOverflow)?;
        let effect = state.effects.get(&next).copied();
        state.hits = next;
        Ok(effect)
    }

    /// Read-only counter access; querying an unknown point never registers it.
    pub fn hits(&self, point: &str) -> Result<u64, FaultError> {
        validate_point(point)?;
        self.points
            .get(point)
            .map(|state| state.hits)
            .ok_or(FaultError::UnknownPoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(point: &str, occurrence: u64, effect: FaultEffect) -> FaultRule {
        FaultRule {
            point: point.into(),
            occurrence,
            effect,
        }
    }

    #[test]
    fn effects_fire_only_at_the_exact_one_based_occurrence() {
        let mut plan = FaultPlan::new(vec![
            rule("append", 2, FaultEffect::Fail),
            rule("append", 4, FaultEffect::Crash),
        ])
        .expect("valid explicit schedule");
        assert_eq!(plan.hits("append"), Ok(0));
        for expected in [
            None,
            Some(FaultEffect::Fail),
            None,
            Some(FaultEffect::Crash),
            None,
        ] {
            assert_eq!(plan.hit("append"), Ok(expected));
        }
        assert_eq!(plan.hits("append"), Ok(5));
        assert_eq!(plan.points["append"].effects.len(), 2);
    }

    #[test]
    fn explicit_schedule_and_clones_reproduce_the_same_results() {
        let rules = vec![
            rule("send", 2, FaultEffect::Drop),
            rule("send", 4, FaultEffect::Duplicate),
            rule("append", 1, FaultEffect::Fail),
            rule("delay", 1, FaultEffect::DelayMs(17)),
        ];
        let mut first = FaultPlan::new(rules.clone()).expect("valid schedule");
        let mut second = first.clone();
        for point in ["send", "append", "send", "delay", "send", "send", "append"] {
            assert_eq!(first.hit(point), second.hit(point));
            assert_eq!(first.hits(point), second.hits(point));
        }
        assert_eq!(first, second);
        let mut resumed = first.clone();
        assert_eq!(first.hit("send"), resumed.hit("send"));
        assert_eq!(first, resumed);
        let mut reversed = rules;
        reversed.reverse();
        assert_eq!(
            FaultPlan::new(reversed),
            FaultPlan::new(vec![
                rule("send", 2, FaultEffect::Drop),
                rule("append", 1, FaultEffect::Fail),
                rule("delay", 1, FaultEffect::DelayMs(17)),
                rule("send", 4, FaultEffect::Duplicate)
            ])
        );
    }

    #[test]
    fn counters_for_different_points_are_independent() {
        let mut plan = FaultPlan::new(vec![
            rule("left", 2, FaultEffect::Crash),
            rule("right", 1, FaultEffect::Fail),
        ])
        .expect("valid schedule");
        assert_eq!(plan.hit("left"), Ok(None));
        assert_eq!(plan.hits("right"), Ok(0));
        assert_eq!(plan.hit("right"), Ok(Some(FaultEffect::Fail)));
        assert_eq!(plan.hits("left"), Ok(1));
        assert_eq!(plan.hit("left"), Ok(Some(FaultEffect::Crash)));
        assert_eq!(plan.hits("right"), Ok(1));
    }

    #[test]
    fn duplicate_point_occurrence_is_rejected_even_with_identical_effect() {
        for effect in [FaultEffect::Fail, FaultEffect::Duplicate] {
            assert_eq!(
                FaultPlan::new(vec![
                    rule("send", 1, FaultEffect::Fail),
                    rule("send", 1, effect)
                ]),
                Err(FaultError::DuplicateRule)
            );
        }
        assert!(
            FaultPlan::new(vec![
                rule("send", 1, FaultEffect::Fail),
                rule("other", 1, FaultEffect::Fail),
                rule("send", 2, FaultEffect::Fail)
            ])
            .is_ok()
        );
    }

    #[test]
    fn point_limits_use_utf8_bytes_and_reject_blank_aliases_and_controls() {
        let exact = "π".repeat(MAX_FAULT_POINT_BYTES / 2);
        let mut plan = FaultPlan::new(vec![rule(&exact, 1, FaultEffect::Fail)])
            .expect("exact UTF8 byte limit");
        assert_eq!(plan.hit(&exact), Ok(Some(FaultEffect::Fail)));
        for bad in [
            "".into(),
            " ".into(),
            " prefix".into(),
            "suffix ".into(),
            "\u{2003}".into(),
            "bad\npoint".into(),
            "bad\0point".into(),
            "bad\u{7f}point".into(),
            "π".repeat(MAX_FAULT_POINT_BYTES / 2 + 1),
            "x".repeat(MAX_FAULT_POINT_BYTES + 1),
        ] {
            assert_eq!(
                FaultPlan::new(vec![rule(&bad, 1, FaultEffect::Fail)]),
                Err(FaultError::InvalidPoint)
            );
            let before = plan.clone();
            assert_eq!(plan.hit(&bad), Err(FaultError::InvalidPoint));
            assert_eq!(plan.hits(&bad), Err(FaultError::InvalidPoint));
            assert_eq!(plan, before);
        }
    }

    #[test]
    fn hard_rule_cap_bounds_all_registered_points_and_rules() {
        let rules = (0..MAX_FAULT_RULES)
            .map(|index| rule(&format!("point-{index}"), 1, FaultEffect::Drop))
            .collect::<Vec<_>>();
        let plan = FaultPlan::new(rules.clone()).expect("exact hard cap");
        assert_eq!(plan.points.len(), MAX_FAULT_RULES);
        assert_eq!(
            plan.points
                .values()
                .map(|point| point.effects.len())
                .sum::<usize>(),
            MAX_FAULT_RULES
        );
        let mut oversized = rules;
        oversized.push(rule("overflow", 1, FaultEffect::Drop));
        assert_eq!(FaultPlan::new(oversized), Err(FaultError::TooManyRules));
    }

    #[test]
    fn occurrence_zero_and_excessive_delays_are_rejected() {
        assert_eq!(
            FaultPlan::new(vec![rule("point", 0, FaultEffect::Fail)]),
            Err(FaultError::InvalidOccurrence)
        );
        for delay in [MAX_FAULT_DELAY_MS + 1, u64::MAX] {
            assert_eq!(
                FaultPlan::new(vec![rule("point", 1, FaultEffect::DelayMs(delay))]),
                Err(FaultError::DelayTooLong)
            );
        }
        for delay in [0, MAX_FAULT_DELAY_MS] {
            let mut plan = FaultPlan::new(vec![rule("point", 1, FaultEffect::DelayMs(delay))])
                .expect("bounded symbolic delay");
            assert_eq!(plan.hit("point"), Ok(Some(FaultEffect::DelayMs(delay))));
        }
    }

    #[test]
    fn unknown_points_and_read_queries_never_grow_or_mutate_the_plan() {
        let mut plan =
            FaultPlan::new(vec![rule("known", 3, FaultEffect::Fail)]).expect("known point");
        let before = plan.clone();
        for index in 0..1000 {
            let point = format!("unknown-{index}");
            assert_eq!(plan.hit(&point), Err(FaultError::UnknownPoint));
            assert_eq!(plan.hits(&point), Err(FaultError::UnknownPoint));
        }
        assert_eq!(plan, before);
        let mut empty = FaultPlan::new(vec![]).expect("empty schedule");
        assert_eq!(empty.hit("unknown"), Err(FaultError::UnknownPoint));
        assert!(empty.points.is_empty());
    }

    #[test]
    fn final_counter_boundary_fires_once_then_overflow_is_atomic() {
        let mut plan = FaultPlan::new(vec![
            rule("last", u64::MAX, FaultEffect::Crash),
            rule("other", 1, FaultEffect::Fail),
        ])
        .expect("valid MAX occurrence");
        // Test-only boundary setup: public callers cannot change counters.
        plan.points.get_mut("last").expect("registered point").hits = u64::MAX - 1;
        assert_eq!(plan.hit("last"), Ok(Some(FaultEffect::Crash)));
        assert_eq!(plan.hits("last"), Ok(u64::MAX));
        let before = plan.clone();
        assert_eq!(plan.hit("last"), Err(FaultError::CounterOverflow));
        assert_eq!(plan, before);
        assert_eq!(plan.hits("other"), Ok(0));
        assert_eq!(plan.hit("other"), Ok(Some(FaultEffect::Fail)));
    }

    #[test]
    fn every_effect_is_data_and_never_executes_the_requested_failure() {
        for effect in [
            FaultEffect::Fail,
            FaultEffect::Crash,
            FaultEffect::Drop,
            FaultEffect::Duplicate,
            FaultEffect::DelayMs(MAX_FAULT_DELAY_MS),
        ] {
            let mut plan =
                FaultPlan::new(vec![rule("symbol", 1, effect)]).expect("symbolic effect");
            assert_eq!(plan.hit("symbol"), Ok(Some(effect)));
            assert_eq!(plan.hit("symbol"), Ok(None));
            assert_eq!(plan.hits("symbol"), Ok(2));
        }
    }
}
