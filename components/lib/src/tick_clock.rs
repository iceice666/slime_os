//! Conversion between the root's raw monotonic ticks and millisecond time,
//! for a component that learned its counter's rate from `CLOCK RATE READ`.
//!
//! Pure arithmetic: the caller reads the counter and the rate through the
//! runtime, and this module only decides what a tick count means. A rate
//! below one kilohertz is refused because a millisecond would then be a
//! fraction of a tick and every conversion would round to nothing.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickClock {
    base: u64,
    ticks_per_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateError {
    TooSlow,
}

impl TickClock {
    /// `rate_hz` ticks per second, with `base` the tick count that becomes
    /// millisecond zero.
    pub const fn new(rate_hz: u64, base: u64) -> Result<Self, RateError> {
        if rate_hz < 1000 {
            return Err(RateError::TooSlow);
        }
        Ok(Self {
            base,
            ticks_per_ms: rate_hz / 1000,
        })
    }
    pub const fn rate_hz(&self) -> u64 {
        self.ticks_per_ms * 1000
    }
    /// Milliseconds since `base`. A counter read before `base` is clamped to
    /// zero rather than going negative.
    pub const fn millis(&self, now_ticks: u64) -> i64 {
        let elapsed = now_ticks.saturating_sub(self.base) / self.ticks_per_ms;
        if elapsed > i64::MAX as u64 {
            i64::MAX
        } else {
            elapsed as i64
        }
    }
    /// The tick count for a millisecond duration, saturating.
    pub const fn ticks(&self, millis: u64) -> u64 {
        millis.saturating_mul(self.ticks_per_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qemu_virt_rate_converts_both_ways() {
        let clock = TickClock::new(62_500_000, 1_000).unwrap();
        assert_eq!(clock.rate_hz(), 62_500_000);
        assert_eq!(clock.millis(1_000), 0);
        assert_eq!(clock.millis(1_000 + 62_500), 1);
        assert_eq!(clock.millis(1_000 + 62_500 * 1500), 1500);
        assert_eq!(clock.ticks(1), 62_500);
        assert_eq!(clock.ticks(3_000), 187_500_000);
    }

    #[test]
    fn a_read_before_the_base_is_zero_not_negative() {
        let clock = TickClock::new(1_000_000, 5_000).unwrap();
        assert_eq!(clock.millis(4_000), 0);
        assert_eq!(clock.millis(u64::MAX), ((u64::MAX - 5_000) / 1_000) as i64);
    }

    #[test]
    fn slow_rates_are_refused() {
        assert_eq!(TickClock::new(999, 0), Err(RateError::TooSlow));
        assert!(TickClock::new(1_000, 0).is_ok());
    }
}
