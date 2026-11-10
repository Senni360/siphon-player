//! The device's power, session and network state, and what the app may
//! spend because of it.
//!
//! fastframe-power reads the system: energy saver, battery, display, lock,
//! the network's reach and cost, sleep and wake. The app reads its
//! [`Conditions`] and [`Event`]s in the logic pass, which runs with or
//! without a window, and turns the conditions into a [`Budget`] for its
//! drawing and polling here.

pub use fastframe_power::{Ack, Activity, Conditions, Event, KeepAwake, Power};

use std::time::Duration;

use crate::player::{LocalState, Playback};

/// How much the app may spend on drawing and polling.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Budget {
    /// As designed.
    #[default]
    Normal,
    /// The user asked to save energy, or the window is not in focus:
    /// animations run at half rate.
    Saver,
    /// Nobody can see the window (none shown, the display off or the session
    /// locked): nothing is drawn and polling slows down.
    Background,
}

/// The budget for `conditions`, given whether a window is shown and focused.
pub fn budget(conditions: Conditions, window_shown: bool, focused: bool) -> Budget {
    if !window_shown || conditions.display_off || conditions.locked {
        Budget::Background
    } else if conditions.saver || !focused {
        Budget::Saver
    } else {
        Budget::Normal
    }
}

/// The frame rate of the main window's visualizers (the player bar's and
/// the mini player's) within `budget`, from the user's `fps` (0 is uncapped,
/// the display's own rate): as chosen in focus, half of it out of focus or
/// while saving energy, and at most 30 while saving energy. `None` while
/// nobody can see the window: no frames. Uncapped halves to 30, the half
/// rate these visualizers always had.
pub fn visualiser_fps(budget: Budget, conditions: Conditions, fps: u32) -> Option<u32> {
    match budget {
        Budget::Background => None,
        Budget::Normal => Some(fps),
        Budget::Saver => {
            let half = if fps == 0 { 30 } else { (fps / 2).max(1) };
            Some(if conditions.saver { half.min(30) } else { half })
        }
    }
}

/// How often the progress readout, and anything else that merely ticks along
/// with a playing song, needs a new frame: four times a second is plenty for a
/// line that moves a few pixels a second.
pub const PROGRESS_FRAME: Duration = Duration::from_millis(250);

/// The gap until a continuous animation's next frame, when it wants one every
/// `interval`, within `budget`: as asked in focus, half as often out of focus
/// or while saving energy, and no frame at all while nobody can see the
/// window.
pub fn animation_interval(budget: Budget, interval: Duration) -> Option<Duration> {
    match budget {
        Budget::Background => None,
        Budget::Saver => Some(interval.saturating_mul(2)),
        Budget::Normal => Some(interval),
    }
}

/// The soonest the drawing pass needs another frame for what merely moves with
/// the song: the progress readout's pace while one plays. `None` when nothing
/// needs a frame, so a paused window stays still.
pub fn progress_frame(budget: Budget, playing: bool) -> Option<Duration> {
    if budget == Budget::Background || !playing {
        return None;
    }
    Some(PROGRESS_FRAME)
}

/// Keeps this pass's budget where the drawing helpers can read it.
pub fn note_budget(ctx: &egui::Context, budget: Budget) {
    ctx.data_mut(|data| data.insert_temp(budget_id(), budget));
}

/// The budget of the pass being drawn, as [`note_budget`] last set it.
pub fn frame_budget(ctx: &egui::Context) -> Budget {
    ctx.data(|data| data.get_temp(budget_id()))
        .unwrap_or_default()
}

fn budget_id() -> egui::Id {
    egui::Id::new("power-frame-budget")
}

/// Whether local playback is doing work the user is waiting for, which
/// macOS must not slow down with App Nap while another app is in front,
/// and during which the computer must not go to sleep on its own (#962):
/// playing or loading, including a song playing on through a dropped
/// session while the next one connects (#268). Pause, stop and an output
/// error let the app nap again; remote playback never holds it awake.
pub fn playback_needs_activity(local: &LocalState) -> bool {
    local.error.is_none()
        && (local.loading || matches!(local.playback, Playback::Playing | Playback::Loading))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The player bar and mini player visualizers keep the rate chosen in
    /// focus, halve it out of focus, draw at most 30 saving energy, and
    /// none while nobody can see them.
    #[test]
    fn the_visualizers_halve_their_rate_out_of_focus_and_stop_unseen() {
        let normal = Conditions::default();
        let saving = Conditions {
            saver: true,
            ..normal
        };
        assert_eq!(visualiser_fps(Budget::Normal, normal, 60), Some(60));
        assert_eq!(visualiser_fps(Budget::Normal, normal, 0), Some(0));
        assert_eq!(
            visualiser_fps(Budget::Saver, normal, 144),
            Some(72),
            "out of focus"
        );
        assert_eq!(visualiser_fps(Budget::Saver, normal, 0), Some(30));
        assert_eq!(
            visualiser_fps(Budget::Saver, saving, 144),
            Some(30),
            "saving energy"
        );
        assert_eq!(visualiser_fps(Budget::Saver, saving, 30), Some(15));
        assert_eq!(visualiser_fps(Budget::Background, normal, 60), None);
        assert_eq!(visualiser_fps(Budget::Background, normal, 0), None);
    }

    #[test]
    fn nothing_is_drawn_when_nobody_can_see_it() {
        let normal = Conditions::default();
        assert_eq!(budget(normal, true, true), Budget::Normal);
        assert_eq!(budget(normal, false, true), Budget::Background, "no window");
        for conditions in [
            Conditions {
                display_off: true,
                ..normal
            },
            Conditions {
                locked: true,
                ..normal
            },
        ] {
            assert_eq!(budget(conditions, true, true), Budget::Background);
        }
    }

    /// Demo runs and tests get a fixed `Power`, and the device's own state
    /// (Low Power Mode on a Mac) must not leak into what they see.
    #[test]
    fn a_fixed_monitor_keeps_its_conditions() {
        let given = Conditions {
            online: false,
            ..Conditions::default()
        };
        let power = Power::fixed(given);
        assert_eq!(power.conditions(), given);
    }

    #[test]
    fn saving_energy_or_losing_focus_halves_the_animations() {
        let normal = Conditions::default();
        let saving = Conditions {
            saver: true,
            ..normal
        };
        assert_eq!(budget(saving, true, true), Budget::Saver);
        assert_eq!(budget(normal, true, false), Budget::Saver);
        assert_eq!(
            budget(
                Conditions {
                    on_battery: true,
                    ..normal
                },
                true,
                true
            ),
            Budget::Normal,
            "battery alone is not a request to save"
        );
    }

    /// #789: local playback holds off App Nap while it plays or loads, and
    /// lets go on pause, stop, an output error or a disconnection.
    #[test]
    fn only_local_playback_at_work_needs_an_activity() {
        let mut local = LocalState {
            connected: true,
            playback: Playback::Playing,
            ..LocalState::default()
        };
        assert!(playback_needs_activity(&local));
        local.playback = Playback::Loading;
        assert!(playback_needs_activity(&local));
        local.playback = Playback::Paused;
        local.loading = true;
        assert!(playback_needs_activity(&local), "the next track is loading");
        local.loading = false;
        for playback in [Playback::Paused, Playback::Stopped] {
            local.playback = playback;
            assert!(!playback_needs_activity(&local));
        }
        local.playback = Playback::Playing;
        local.error = Some("output unavailable".into());
        assert!(!playback_needs_activity(&local));
        local.error = None;
        local.connected = false;
        assert!(
            playback_needs_activity(&local),
            "a song playing on through a dropped session"
        );
        assert!(!playback_needs_activity(&LocalState::default()));
    }

    /// A decoration that wants a frame every 66 ms gets one in focus, one every
    /// 132 ms out of focus or saving energy, and none while nobody can see the
    /// window, so hidden and minimised windows go quiet.
    #[test]
    fn animations_go_quiet_unseen_and_stretch_out_of_focus() {
        let wanted = Duration::from_millis(66);
        assert_eq!(animation_interval(Budget::Normal, wanted), Some(wanted));
        assert_eq!(
            animation_interval(Budget::Saver, wanted),
            Some(Duration::from_millis(132))
        );
        assert_eq!(animation_interval(Budget::Background, wanted), None);
    }

    /// A playing song only needs the four-times-a-second progress pace; paused
    /// or unseen, nothing asks for a frame on its behalf.
    #[test]
    fn a_playing_song_needs_only_the_progress_pace() {
        assert!(PROGRESS_FRAME >= Duration::from_millis(100));
        assert!(PROGRESS_FRAME <= Duration::from_millis(250));
        assert_eq!(progress_frame(Budget::Normal, true), Some(PROGRESS_FRAME));
        assert_eq!(progress_frame(Budget::Saver, true), Some(PROGRESS_FRAME));
        assert_eq!(progress_frame(Budget::Normal, false), None);
        assert_eq!(progress_frame(Budget::Background, true), None);
    }

    /// The budget noted for the pass is the one the drawing helpers read, and
    /// a context that was never told anything draws as designed.
    #[test]
    fn the_pass_budget_is_readable_from_the_context() {
        let ctx = egui::Context::default();
        assert_eq!(frame_budget(&ctx), Budget::Normal);
        note_budget(&ctx, Budget::Background);
        assert_eq!(frame_budget(&ctx), Budget::Background);
        note_budget(&ctx, Budget::Saver);
        assert_eq!(frame_budget(&ctx), Budget::Saver);
    }
}
