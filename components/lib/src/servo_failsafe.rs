//! The servo driver's silence window: a channel driving above its idle width
//! is returned to idle when no request for that channel has been accepted for
//! the window, and a return the device refuses is retried on a shorter
//! interval rather than deferred a whole window.
//!
//! Pure bookkeeping over caller-supplied millisecond times: the driver reads the
//! clock, programs the registers, and reports each outcome here. A deadline
//! moves only through `accepted` for its own channel, so traffic for another
//! channel, or a request the driver refused, never extends it.

/// One channel's last accepted frame and, while it drives above idle, the time
/// its silence window expires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Channel {
    period_us: u32,
    due_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Failsafe<const N: usize> {
    channels: [Channel; N],
    idle_pulse_us: u32,
    window_ms: i64,
    retry_ms: i64,
}

impl<const N: usize> Failsafe<N> {
    /// Every channel starts stopped at `period_us`. A channel whose pulse is
    /// at or below `idle_pulse_us` is never due; one above it is due
    /// `window_ms` after its last accepted request, and `retry_ms` after a
    /// refused return.
    pub const fn new(period_us: u32, idle_pulse_us: u32, window_ms: i64, retry_ms: i64) -> Self {
        Self {
            channels: [Channel {
                period_us,
                due_ms: None,
            }; N],
            idle_pulse_us,
            window_ms,
            retry_ms,
        }
    }

    /// The device accepted `pulse_us` in `period_us` frames on `channel` at
    /// `now_ms`. Only that channel's window restarts, and only when it now
    /// drives above idle; otherwise it is disarmed.
    pub fn accepted(&mut self, channel: usize, period_us: u32, pulse_us: u32, now_ms: i64) {
        let live = pulse_us > self.idle_pulse_us;
        self.channels[channel] = Channel {
            period_us,
            due_ms: live.then(|| now_ms.saturating_add(self.window_ms)),
        };
    }

    /// Whether any channel drives above idle, so the caller must keep reading
    /// the clock.
    pub fn any_live(&self) -> bool {
        self.channels.iter().any(|channel| channel.due_ms.is_some())
    }

    /// Each channel whose deadline has passed at `now_ms`, in index order, with
    /// the frame it was last programmed with.
    pub fn due(&self, now_ms: i64) -> impl Iterator<Item = (usize, u32)> + '_ {
        self.channels
            .iter()
            .enumerate()
            .filter(move |(_, channel)| channel.due_ms.is_some_and(|due| now_ms >= due))
            .map(|(index, channel)| (index, channel.period_us))
    }

    /// The device accepted the idle width on `channel`: nothing is live there.
    pub fn returned(&mut self, channel: usize) {
        self.channels[channel].due_ms = None;
    }

    /// The device refused the idle width on `channel` at `now_ms`: the channel
    /// is still driving its last pulse and is due again after the retry
    /// interval.
    pub fn deferred(&mut self, channel: usize, now_ms: i64) {
        self.channels[channel].due_ms = Some(now_ms.saturating_add(self.retry_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: u32 = 1000;
    const WINDOW: i64 = 10_000;
    const RETRY: i64 = 1_000;

    fn failsafe() -> Failsafe<6> {
        Failsafe::new(2000, IDLE, WINDOW, RETRY)
    }

    fn due(failsafe: &Failsafe<6>, now_ms: i64) -> ([(usize, u32); 6], usize) {
        let mut out = [(0, 0); 6];
        let mut count = 0;
        for entry in failsafe.due(now_ms) {
            out[count] = entry;
            count += 1;
        }
        (out, count)
    }

    #[test]
    fn a_channel_above_idle_is_due_after_the_window_and_one_at_idle_never_is() {
        let mut failsafe = failsafe();
        assert!(!failsafe.any_live());
        failsafe.accepted(0, 20000, 1600, 100);
        failsafe.accepted(1, 20000, IDLE, 100);
        assert!(failsafe.any_live());
        assert_eq!(due(&failsafe, 100 + WINDOW - 1).1, 0);
        let (entries, count) = due(&failsafe, 100 + WINDOW);
        assert_eq!(&entries[..count], &[(0, 20000)]);
        assert_eq!(due(&failsafe, i64::MAX).1, 1);
    }

    #[test]
    fn traffic_for_another_channel_does_not_extend_a_deadline() {
        let mut failsafe = failsafe();
        failsafe.accepted(0, 20000, 1600, 0);
        for now in (1_000..=9_000).step_by(1_000) {
            failsafe.accepted(1, 20000, 1200, now);
        }
        let (entries, count) = due(&failsafe, WINDOW);
        assert_eq!(&entries[..count], &[(0, 20000)]);
    }

    #[test]
    fn disabling_a_live_channel_disarms_it() {
        let mut failsafe = failsafe();
        failsafe.accepted(2, 20000, 1800, 0);
        failsafe.accepted(2, 20000, 0, 50);
        assert!(!failsafe.any_live());
        assert_eq!(due(&failsafe, i64::MAX).1, 0);
    }

    #[test]
    fn a_refused_return_is_retried_after_the_retry_interval_not_a_window() {
        let mut failsafe = failsafe();
        failsafe.accepted(3, 20000, 2000, 0);
        failsafe.deferred(3, WINDOW);
        assert!(failsafe.any_live());
        assert_eq!(due(&failsafe, WINDOW + RETRY - 1).1, 0);
        let (entries, count) = due(&failsafe, WINDOW + RETRY);
        assert_eq!(&entries[..count], &[(3, 20000)]);
        failsafe.returned(3);
        assert!(!failsafe.any_live());
        assert_eq!(due(&failsafe, i64::MAX).1, 0);
    }

    #[test]
    fn every_overdue_channel_is_due_in_index_order_and_nothing_else() {
        let mut failsafe = failsafe();
        failsafe.accepted(4, 5000, 1500, 0);
        failsafe.accepted(1, 20000, 1100, 0);
        failsafe.accepted(5, 20000, 1900, 5_000);
        let (entries, count) = due(&failsafe, WINDOW);
        assert_eq!(&entries[..count], &[(1, 20000), (4, 5000)]);
    }
}
