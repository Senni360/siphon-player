//! Waiting for a sound device that another app took away.
//!
//! FL Studio, or anything else that opens the speakers in exclusive mode,
//! preempts Encore's shared-mode stream, and Windows keeps refusing to open
//! the device until that app lets go (or the default output moves to another
//! device). librespot pauses on the failed write, which holds the song at the
//! position it had reached, and never writes again on its own, so the engine
//! has to ask for the device instead.
//!
//! Nothing here touches a device: the engine's retry task sleeps for the
//! delays this module hands out, so the schedule can be tested without one.

use std::time::Duration;

/// How long to wait before each attempt at opening the device again: quickly
/// at first, for a device another app holds for a moment, then every three
/// seconds until it comes back.
const BACKOFF: [Duration; 4] = [
    Duration::from_millis(500),
    Duration::from_millis(1_000),
    Duration::from_millis(2_000),
    Duration::from_millis(3_000),
];

/// What the attempts settle at, so a device that stays away is not polled
/// faster than this.
const SETTLED: Duration = Duration::from_secs(3);

/// How long to wait before attempt `attempt`, counting from zero.
#[must_use]
pub fn delay(attempt: u32) -> Duration {
    BACKOFF.get(attempt as usize).copied().unwrap_or(SETTLED)
}

/// A song held at its position while the device cannot be opened, and the
/// retries that are supposed to bring it back.
///
/// The engine owns one of these for its recovery task: `lost` when a write
/// finds the output gone, `wait` for each pause before another try, and
/// `resumed` or `let_go` when the waiting is over.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Waiting {
    held: bool,
    attempts: u32,
}

impl Waiting {
    /// The device went away under the music. A failure while already held
    /// starts the schedule over, so a device that keeps slipping away is
    /// still tried again soon.
    pub fn lost(&mut self) {
        self.held = true;
        self.attempts = 0;
    }

    /// Whether a song is being held for the device.
    #[must_use]
    pub fn held(&self) -> bool {
        self.held
    }

    /// The wait before the next attempt, and counts it. `None` once nothing
    /// is held: the listener paused or chose something else.
    pub fn wait(&mut self) -> Option<Duration> {
        if !self.held {
            return None;
        }
        let delay = delay(self.attempts);
        self.attempts = self.attempts.saturating_add(1);
        Some(delay)
    }

    /// The device is back and the song carries on from where it stopped.
    pub fn resumed(&mut self) {
        self.let_go();
    }

    /// Stops waiting without playing: the listener's choice outranks the
    /// device coming back.
    pub fn let_go(&mut self) {
        self.held = false;
        self.attempts = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_retry_schedule_is_quick_then_settles_at_three_seconds() {
        assert_eq!(delay(0), Duration::from_millis(500));
        assert_eq!(delay(1), Duration::from_millis(1_000));
        assert_eq!(delay(2), Duration::from_millis(2_000));
        assert_eq!(delay(3), Duration::from_millis(3_000));
        assert_eq!(delay(4), Duration::from_millis(3_000));
        assert_eq!(delay(u32::MAX), SETTLED, "a long wait does not overflow");
    }

    #[test]
    fn nothing_is_tried_when_nothing_is_held() {
        let mut waiting = Waiting::default();
        assert!(!waiting.held());
        assert_eq!(waiting.wait(), None);
    }

    #[test]
    fn a_held_song_is_tried_until_the_listener_lets_go() {
        let mut waiting = Waiting::default();
        waiting.lost();
        assert!(waiting.held());
        assert_eq!(waiting.wait(), Some(Duration::from_millis(500)));
        assert_eq!(waiting.wait(), Some(Duration::from_millis(1_000)));
        waiting.let_go();
        assert!(!waiting.held());
        assert_eq!(waiting.wait(), None, "a pause ends the retries");
    }

    #[test]
    fn the_device_coming_back_ends_the_waiting() {
        let mut waiting = Waiting::default();
        waiting.lost();
        assert_eq!(waiting.wait(), Some(Duration::from_millis(500)));
        waiting.resumed();
        assert!(!waiting.held());
        assert_eq!(waiting.wait(), None);
    }

    #[test]
    fn a_second_failure_starts_the_schedule_again() {
        let mut waiting = Waiting::default();
        waiting.lost();
        assert_eq!(waiting.wait(), Some(Duration::from_millis(500)));
        assert_eq!(waiting.wait(), Some(Duration::from_millis(1_000)));
        waiting.lost();
        assert_eq!(
            waiting.wait(),
            Some(Duration::from_millis(500)),
            "a fresh loss does not wait on the old schedule"
        );
    }
}
