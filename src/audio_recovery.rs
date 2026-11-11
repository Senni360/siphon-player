//! Waiting out an output device that another app took away.
//!
//! FL Studio, or anything else that opens the speakers in exclusive mode,
//! preempts Encore's shared-mode stream, and Windows keeps refusing to open
//! the device until that app lets go (or the default output moves to another
//! device). librespot pauses on the failed write, which holds the song at the
//! position it had reached, and never writes again on its own, so the engine
//! has to ask for the device instead.
//!
//! Nothing here touches a device or sleeps: the engine's retry task waits for
//! the delays this module hands out, so the schedule and the decisions can be
//! tested without an output.

use std::time::Duration;

/// How long to wait before each attempt at opening the output again: quickly
/// at first, for a device another app holds for a moment, then every five
/// seconds until that app lets go or Windows moves the default output to
/// another device.
const BACKOFF: [Duration; 4] = [
    Duration::from_millis(500),
    Duration::from_millis(1_000),
    Duration::from_millis(2_000),
    Duration::from_millis(4_000),
];

/// What the attempts settle at, so a device that stays away is asked about
/// no more often than this.
const SETTLED: Duration = Duration::from_secs(5);

/// How long to wait before attempt `attempt`, counting from zero.
#[must_use]
pub fn delay(attempt: u32) -> Duration {
    BACKOFF.get(attempt as usize).copied().unwrap_or(SETTLED)
}

/// Whether a song plays once the output device is back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Wanted {
    /// The listener, or the queue, wants this song: it carries on from where
    /// it stopped.
    Play,
    /// The listener paused or stopped, so their silence outranks the device.
    #[default]
    Stay,
}

impl Wanted {
    /// What a transport command asks for.
    #[must_use]
    pub fn playing(play: bool) -> Self {
        if play { Self::Play } else { Self::Stay }
    }
}

/// A song held at its position while the output will not open, and the
/// retries meant to bring the device back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Waiting {
    held: bool,
    wanted: Wanted,
    /// The pause librespot sends for the write that lost the device is still
    /// to come. A failed write pauses exactly once, from the thread that made
    /// it, so the first pause after a loss is that one; the next is the
    /// listener's.
    own_pause: bool,
    attempts: u32,
}

impl Waiting {
    /// A write found the output gone under the music: the song is held where
    /// it stopped. A failure while already held starts the schedule over, so
    /// a device that slips away again is tried again soon.
    ///
    /// The failure leaves [`Wanted`] as it is: a write only happens while the
    /// decoder plays, so a listener who paused just before it keeps their
    /// silence.
    pub fn lost(&mut self) {
        self.held = true;
        self.own_pause = true;
        self.attempts = 0;
    }

    /// The decoder started a song: a listener who pauses and plays again
    /// while the device is away gets their song back when the device is.
    pub fn playing(&mut self) {
        self.wanted = Wanted::Play;
    }

    /// The decoder paused. The pause a lost write causes is not the
    /// listener's; any other is, and outranks the device coming back.
    pub fn paused(&mut self) {
        if std::mem::take(&mut self.own_pause) {
            return;
        }
        self.chose(Wanted::Stay);
    }

    /// The listener or the queue chose something, before librespot has
    /// answered: a play or a load says where the song picks up when the
    /// device is back, a pause ends the waiting.
    pub fn chose(&mut self, wanted: Wanted) {
        self.wanted = wanted;
        // The listener's answer reorders what librespot is about to say: the
        // pause a loss caused may never follow a pause command.
        self.own_pause = false;
        if wanted == Wanted::Stay {
            self.let_go();
        }
    }

    /// The player stopped: there is no song left to carry on.
    pub fn stopped(&mut self) {
        self.chose(Wanted::Stay);
    }

    /// Nothing waits for the device any more.
    pub fn let_go(&mut self) {
        self.held = false;
        self.own_pause = false;
        self.attempts = 0;
    }

    /// Whether a song is being held for the device.
    #[must_use]
    pub fn held(&self) -> bool {
        self.held
    }

    /// Whether the song plays once the device is back.
    #[must_use]
    pub fn wanted(&self) -> Wanted {
        self.wanted
    }

    /// The wait before the next attempt, counting it. `None` once nothing is
    /// held.
    pub fn wait(&mut self) -> Option<Duration> {
        if !self.held {
            return None;
        }
        let delay = delay(self.attempts);
        self.attempts = self.attempts.saturating_add(1);
        Some(delay)
    }

    /// The device is open again: whether the song carries on. The next loss
    /// starts a fresh schedule.
    pub fn recovered(&mut self) -> bool {
        let wanted = self.wanted == Wanted::Play;
        self.let_go();
        wanted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_retry_schedule_is_quick_then_settles_at_five_seconds() {
        assert_eq!(delay(0), Duration::from_millis(500));
        assert_eq!(delay(1), Duration::from_millis(1_000));
        assert_eq!(delay(2), Duration::from_millis(2_000));
        assert_eq!(delay(3), Duration::from_millis(4_000));
        assert_eq!(delay(4), SETTLED);
        assert_eq!(delay(u32::MAX), SETTLED, "a long wait does not overflow");
    }

    #[test]
    fn nothing_is_tried_when_nothing_is_held() {
        let mut waiting = Waiting::default();
        assert!(!waiting.held());
        assert_eq!(waiting.wait(), None);
        assert_eq!(waiting.wanted(), Wanted::Stay);
    }

    #[test]
    fn a_device_lost_under_the_music_is_retried_until_it_returns() {
        let mut waiting = Waiting::default();
        waiting.playing();
        waiting.lost();
        assert!(waiting.held());
        assert_eq!(waiting.wait(), Some(Duration::from_millis(500)));
        // librespot pauses the write that failed; that pause is not the
        // listener's, so the waiting goes on.
        waiting.paused();
        assert!(waiting.held());
        assert_eq!(waiting.wait(), Some(Duration::from_millis(1_000)));
        assert!(waiting.recovered(), "the song carries on");
        assert!(!waiting.held());
        assert_eq!(waiting.wait(), None);
    }

    #[test]
    fn the_listeners_pause_while_the_device_is_away_ends_the_waiting() {
        let mut waiting = Waiting::default();
        waiting.playing();
        waiting.lost();
        waiting.paused();
        waiting.chose(Wanted::Stay);
        assert!(!waiting.held(), "their silence outranks the device");
        assert_eq!(waiting.wait(), None);
        assert!(
            !waiting.recovered(),
            "the device coming back does not undo a pause"
        );
        // The pause librespot never sent for the loss is not the next one.
        waiting.playing();
        waiting.lost();
        waiting.paused();
        assert!(waiting.held(), "a fresh loss is waited out again");
    }

    #[test]
    fn a_pause_from_another_spotify_client_ends_the_waiting() {
        let mut waiting = Waiting::default();
        waiting.playing();
        waiting.lost();
        waiting.paused();
        // A pause this engine did not command: another Spotify client, say.
        waiting.paused();
        assert!(!waiting.held());
        assert_eq!(waiting.wanted(), Wanted::Stay);
        assert_eq!(waiting.wait(), None);
    }

    #[test]
    fn a_second_failure_starts_the_schedule_again() {
        let mut waiting = Waiting::default();
        waiting.playing();
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

    #[test]
    fn a_song_change_while_the_device_is_away_waits_for_the_new_song() {
        let mut waiting = Waiting::default();
        waiting.playing();
        waiting.lost();
        waiting.paused();
        // The queue moves on while the device is away, and the next row is
        // to play: it waits for the same device, on its own schedule.
        waiting.chose(Wanted::Play);
        assert!(waiting.held());
        assert_eq!(waiting.wait(), Some(Duration::from_millis(500)));
        assert!(waiting.recovered());

        // A row that is loaded paused waits for nothing.
        waiting.playing();
        waiting.lost();
        waiting.chose(Wanted::Stay);
        assert!(!waiting.held());
        assert_eq!(waiting.wait(), None);
    }

    #[test]
    fn a_stopped_player_leaves_nothing_to_carry_on() {
        let mut waiting = Waiting::default();
        waiting.playing();
        waiting.lost();
        waiting.stopped();
        assert!(!waiting.held());
        assert_eq!(waiting.wait(), None);
    }
}
