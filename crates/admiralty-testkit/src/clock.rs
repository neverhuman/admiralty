//! Deterministic logical milliseconds for offline fixtures.
//!
//! Time changes only through an explicit call. A clone is an independent clock;
//! it does not share state with the original. These values do not represent wall
//! time, process liveness, or qualification of any real-time deadline.

use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClockError {
    Overflow { now_ms: u64, delta_ms: u64 },
    Backwards { now_ms: u64, target_ms: u64 },
}

impl fmt::Display for ClockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow { now_ms, delta_ms } => write!(
                formatter,
                "logical time overflow: {now_ms} ms plus {delta_ms} ms"
            ),
            Self::Backwards { now_ms, target_ms } => write!(
                formatter,
                "logical time cannot move backwards from {now_ms} ms to {target_ms} ms"
            ),
        }
    }
}

impl Error for ClockError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeClock {
    now_ms: u64,
}

impl FakeClock {
    pub const fn new(initial_ms: u64) -> Self {
        Self { now_ms: initial_ms }
    }

    pub const fn now_ms(&self) -> u64 {
        self.now_ms
    }

    /// Advances logical time, leaving it unchanged if addition overflows.
    pub fn advance_ms(&mut self, delta_ms: u64) -> Result<u64, ClockError> {
        let next = self.checked_after_ms(delta_ms)?;
        self.now_ms = next;
        Ok(next)
    }

    /// Equal timestamps are allowed; a backwards target leaves time unchanged.
    pub fn advance_to_ms(&mut self, target_ms: u64) -> Result<u64, ClockError> {
        if target_ms < self.now_ms {
            return Err(ClockError::Backwards {
                now_ms: self.now_ms,
                target_ms,
            });
        }
        self.now_ms = target_ms;
        Ok(target_ms)
    }

    /// Takes an immutable timestamp snapshot without advancing this clock.
    pub fn deadline_after_ms(&self, delta_ms: u64) -> Result<FakeDeadline, ClockError> {
        Ok(FakeDeadline {
            at_ms: self.checked_after_ms(delta_ms)?,
        })
    }

    fn checked_after_ms(&self, delta_ms: u64) -> Result<u64, ClockError> {
        self.now_ms
            .checked_add(delta_ms)
            .ok_or(ClockError::Overflow {
                now_ms: self.now_ms,
                delta_ms,
            })
    }
}

/// A fixed logical timestamp, independent of subsequent clock changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FakeDeadline {
    at_ms: u64,
}

impl FakeDeadline {
    pub const fn at_ms(&self) -> u64 {
        self.at_ms
    }

    /// A deadline is due at equality as well as after its timestamp.
    pub const fn is_due(&self, now_ms: u64) -> bool {
        now_ms >= self.at_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_and_creating_deadlines_do_not_advance_time() {
        let clock = FakeClock::new(42);
        assert_eq!(clock.now_ms(), 42);
        let deadline = clock.deadline_after_ms(8).expect("bounded deadline");
        assert_eq!(deadline.at_ms(), 50);
        assert_eq!(clock.now_ms(), 42);
        assert_eq!(clock.now_ms(), 42);
    }

    #[test]
    fn cloned_clocks_advance_independently_and_deadlines_stay_fixed() {
        let mut original = FakeClock::new(10);
        let mut cloned = original.clone();
        let deadline = original.deadline_after_ms(5).expect("bounded deadline");
        assert_eq!(cloned.advance_ms(7), Ok(17));
        assert_eq!(original.now_ms(), 10);
        assert_eq!(original.advance_to_ms(12), Ok(12));
        assert_eq!(cloned.now_ms(), 17);
        assert_eq!(deadline.at_ms(), 15);
        assert!(!deadline.is_due(original.now_ms()));
        assert!(deadline.is_due(cloned.now_ms()));
    }

    #[test]
    fn same_recorded_operations_replay_identically() {
        fn replay() -> (Vec<u64>, FakeDeadline, FakeClock) {
            let mut clock = FakeClock::new(101);
            let mut observed = vec![clock.now_ms()];
            observed.push(clock.advance_ms(7).expect("recorded bounded step"));
            let deadline = clock.deadline_after_ms(12).expect("bounded deadline");
            observed.push(clock.advance_to_ms(120).expect("recorded forward target"));
            observed.push(clock.advance_ms(0).expect("recorded zero step"));
            (observed, deadline, clock)
        }
        let first = replay();
        assert_eq!(first, replay());
        assert_eq!(first.0, vec![101, 108, 120, 120]);
        assert_eq!(first.1.at_ms(), 120);
        assert!(first.1.is_due(first.2.now_ms()));
    }

    #[test]
    fn deadline_equality_is_due_and_zero_delay_is_immediate() {
        let clock = FakeClock::new(5);
        let deadline = clock.deadline_after_ms(3).expect("bounded deadline");
        assert!(!deadline.is_due(7));
        assert!(deadline.is_due(8));
        assert!(deadline.is_due(9));
        let immediate = clock.deadline_after_ms(0).expect("zero delay");
        assert_eq!(immediate.at_ms(), 5);
        assert!(!immediate.is_due(4));
        assert!(immediate.is_due(clock.now_ms()));
    }

    #[test]
    fn zero_and_maximum_values_are_representable_without_wrapping() {
        let mut clock = FakeClock::new(0);
        assert_eq!(clock.advance_ms(0), Ok(0));
        assert_eq!(clock.advance_to_ms(0), Ok(0));
        let zero = clock.deadline_after_ms(0).expect("zero timestamp");
        assert_eq!(zero.at_ms(), 0);
        assert!(zero.is_due(0));
        let maximum = clock.deadline_after_ms(u64::MAX).expect("maximum deadline");
        assert_eq!(maximum.at_ms(), u64::MAX);
        assert!(!maximum.is_due(u64::MAX - 1));
        assert!(maximum.is_due(u64::MAX));
        assert_eq!(clock.advance_ms(u64::MAX), Ok(u64::MAX));
        assert_eq!(clock.advance_ms(0), Ok(u64::MAX));
        assert_eq!(clock.advance_to_ms(u64::MAX), Ok(u64::MAX));
        assert_eq!(
            clock.deadline_after_ms(0).expect("maximum timestamp"),
            maximum
        );
    }

    #[test]
    fn overflow_rejects_both_advances_and_deadlines_without_mutation() {
        let mut clock = FakeClock::new(u64::MAX - 1);
        let before = clock.clone();
        let error = ClockError::Overflow {
            now_ms: u64::MAX - 1,
            delta_ms: 2,
        };
        assert_eq!(clock.advance_ms(2), Err(error));
        assert_eq!(clock, before);
        assert_eq!(clock.deadline_after_ms(2), Err(error));
        assert_eq!(clock, before);
        assert_eq!(clock.advance_ms(1), Ok(u64::MAX));
        assert_eq!(
            clock.advance_ms(1),
            Err(ClockError::Overflow {
                now_ms: u64::MAX,
                delta_ms: 1
            })
        );
        assert_eq!(clock.now_ms(), u64::MAX);
    }

    #[test]
    fn backwards_targets_reject_without_poisoning_future_forward_advances() {
        let mut clock = FakeClock::new(100);
        let before = clock.clone();
        assert_eq!(
            clock.advance_to_ms(99),
            Err(ClockError::Backwards {
                now_ms: 100,
                target_ms: 99
            })
        );
        assert_eq!(clock, before);
        assert_eq!(clock.advance_to_ms(100), Ok(100));
        assert_eq!(clock.advance_ms(1), Ok(101));
        assert_eq!(clock.advance_to_ms(u64::MAX), Ok(u64::MAX));
        assert_eq!(
            clock.advance_to_ms(0),
            Err(ClockError::Backwards {
                now_ms: u64::MAX,
                target_ms: 0
            })
        );
        assert_eq!(clock.now_ms(), u64::MAX);
    }

    #[test]
    fn typed_errors_have_stable_diagnostic_fields_and_standard_error_support() {
        let backwards = ClockError::Backwards {
            now_ms: 5,
            target_ms: 4,
        };
        let error: &dyn Error = &backwards;
        assert_eq!(
            error.to_string(),
            "logical time cannot move backwards from 5 ms to 4 ms"
        );
        assert!(error.source().is_none());
        assert_eq!(
            ClockError::Overflow {
                now_ms: u64::MAX,
                delta_ms: 1,
            }
            .to_string(),
            "logical time overflow: 18446744073709551615 ms plus 1 ms"
        );
    }
}
