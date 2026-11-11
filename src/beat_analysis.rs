//! A session-long collector for the BPM and key of instrumentals played
//! through Encore's own SoundCloud and YouTube engine.
//!
//! The analysis itself lives in [`crate::bpm_key`]; this half taps only the
//! external engine's pre-volume PCM (see `crate::sink`), so Spotify's own
//! decoded audio never reaches it. The collector decimates what it hears to
//! 11 kHz mono in a bounded buffer, counts only seconds sound actually
//! played (nothing arrives while the output is paused), and once per track,
//! after the configured number of seconds, runs the analysis on a worker
//! thread. Results stay in memory for the session, keyed by track URI; audio
//! is never written to disk or sent anywhere.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::bpm_key::{RATE, Reading, analyze};

/// The engine's interleaved stereo sample rate, what [`Collector::push`]
/// receives.
const SOURCE_RATE: f64 = 44_100.0;

/// Frames of source audio averaged into one mono sample.
const DECIMATE: usize = 4;

/// How many seconds of the current track the collector keeps at most.
const KEEP_SECS: u32 = 30;

/// The least sound the analysis will read, whatever the setting asks for.
/// Less than this cannot answer either question credibly.
const MIN_ANALYSIS_SECS: u32 = 6;

/// How many track results are remembered for the session.
const REMEMBERED: usize = 64;

/// Collects the engine's audio for the tracks the app calls beats and
/// remembers their readings. One per session, shared between the audio
/// feeder thread that pushes frames and the interface that reads answers.
pub struct Collector {
    state: Mutex<State>,
    results: Mutex<Vec<(String, Reading)>>,
}

impl Default for Collector {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                threshold: Some(10),
                beats: HashSet::new(),
                current: None,
                hold: None,
                in_flight: HashSet::new(),
            }),
            results: Mutex::new(Vec::new()),
        }
    }
}

impl std::fmt::Debug for Collector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("beat analysis collector")
    }
}

struct State {
    /// After how many seconds of a track the analysis runs; [`None`] never
    /// runs, and any buffered sound is dropped.
    threshold: Option<u32>,
    /// The tracks the app recognized as instrumentals; only these are ever
    /// buffered or analyzed (see [`Collector::arm`]).
    beats: HashSet<String>,
    /// The track the engine last named, from `begin_track`.
    current: Option<String>,
    /// The sound collected for it, once it is a beat and armed.
    hold: Option<Hold>,
    /// Tracks an analysis is running for, so one never starts twice.
    in_flight: HashSet<String>,
}

struct Hold {
    uri: String,
    /// Mono at [`RATE`], newest last, never longer than `KEEP_SECS` seconds.
    mono: Vec<f32>,
    /// The averaging window across write boundaries.
    carry: [f32; DECIMATE],
    carry_filled: usize,
    /// Seconds of sound that actually played into this hold.
    listened: f64,
    /// When the last analysis attempt left, in seconds of this hold.
    attempted_at: Option<f64>,
}

impl Hold {
    fn new(uri: String) -> Self {
        Self {
            uri,
            mono: Vec::with_capacity(KEEP_SECS as usize * RATE as usize),
            carry: [0.0; DECIMATE],
            carry_filled: 0,
            listened: 0.0,
            attempted_at: None,
        }
    }
}

impl Collector {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// After how many listened seconds the analysis runs. [`None`] turns
    /// the feature off; anything buffered is dropped.
    pub fn set_threshold(&self, secs: Option<u32>) {
        let mut state = self.state();
        state.threshold = secs;
        if secs.is_none() {
            state.hold = None;
        }
    }

    /// Marks a track as one to collect (`crate::for_you::is_beat` said it
    /// is an instrumental). The audio feeder only buffers armed tracks, so
    /// other sources, podcasts and ordinary songs never accumulate.
    pub fn arm(&self, uri: &str) {
        let mut state = self.state();
        if state.threshold.is_none() {
            return;
        }
        // Answered tracks never need arming again, which bounds the set.
        if state.beats.len() >= 2 * REMEMBERED && !state.beats.contains(uri) {
            let results = self.results.lock().unwrap_or_else(PoisonError::into_inner);
            let answered: HashSet<&str> = results.iter().map(|(held, _)| held.as_str()).collect();
            state.beats.retain(|held| answered.contains(held.as_str()));
        }
        state.beats.insert(uri.to_string());
    }

    /// The engine named the track it is feeding; `from_start` at position
    /// 0. Called on every load, seek and refresh. A different track, or
    /// the same track started again, forgets what was collected; a refresh
    /// or seek elsewhere inside the same track keeps it, because listening
    /// time is what the setting counts, and a seek does not unhear it.
    pub fn begin_track(&self, uri: &str, from_start: bool) {
        let mut state = self.state();
        let changed = state.current.as_deref() != Some(uri);
        state.current = Some(uri.to_string());
        if from_start || changed {
            state.hold = None;
        }
    }

    /// The external output stopped; nothing more belongs to the hold.
    pub fn stopped(&self) {
        self.state().hold = None;
    }

    /// Adds the engine's pre-volume interleaved stereo frames. Only frames
    /// of an armed track are kept; see the module docs for why Spotify's
    /// audio never arrives here.
    pub fn push(self: &Arc<Self>, frames: &[f64]) {
        let mut state = self.state();
        let Some(threshold) = state.threshold else {
            return;
        };
        let Some(uri) = state.current.clone() else {
            return;
        };
        if !state.beats.contains(&uri) || self.known(&uri).is_some() {
            return;
        }
        if !state.hold.as_ref().is_some_and(|hold| hold.uri == uri) {
            state.hold = Some(Hold::new(uri.clone()));
        }
        // The frames are appended while the hold is borrowed, so the
        // bookkeeping that starts an analysis works on the answers the
        // borrow hands back, not on the state underneath it.
        let mut snapshot = None;
        {
            let hold = state.hold.as_mut().expect("the hold was just set");
            let (frames, _) = frames.as_chunks::<2>();
            for frame in frames {
                let mono = ((frame[0] + frame[1]) * 0.5) as f32;
                hold.carry[hold.carry_filled] = mono;
                hold.carry_filled += 1;
                if hold.carry_filled == DECIMATE {
                    // Four frames averaged: a cheap anti-alias before the
                    // decimation.
                    hold.mono
                        .push(hold.carry.iter().sum::<f32>() / DECIMATE as f32);
                    hold.carry_filled = 0;
                }
                hold.listened += 1.0 / SOURCE_RATE;
            }
            let keep = KEEP_SECS as usize * RATE as usize;
            if hold.mono.len() > keep {
                hold.mono.drain(0..hold.mono.len() - keep);
            }
            // Once per song: after the configured listening, or once more
            // at a full buffer if the first attempt found only quiet.
            let wanted = f64::from(threshold.max(MIN_ANALYSIS_SECS)).min(f64::from(KEEP_SECS));
            let full = f64::from(KEEP_SECS);
            let fresh = hold.listened >= wanted && hold.attempted_at.is_none();
            let more_music = hold.attempted_at.is_some_and(|at| at < full) && hold.listened >= full;
            if fresh || more_music {
                hold.attempted_at = Some(hold.listened);
                snapshot = Some(hold.mono.clone());
            }
        }
        let Some(snapshot) = snapshot else {
            return;
        };
        if state.in_flight.contains(&uri) {
            return;
        }
        state.in_flight.insert(uri.clone());
        drop(state);
        self.analyze_on_thread(uri, snapshot);
    }

    /// This track's BPM and key once the analysis has answered.
    pub fn known(&self, uri: &str) -> Option<Reading> {
        let results = self.results.lock().unwrap_or_else(PoisonError::into_inner);
        results
            .iter()
            .rev()
            .find(|(held, _)| held == uri)
            .map(|(_, reading)| *reading)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs the analysis for `uri` on its own thread, whatever the
    /// interface is doing, and posts the answer to the session's results.
    /// Silence posts nothing, which is what lets the full-buffer retry try
    /// again on music that arrived late.
    fn analyze_on_thread(self: &Arc<Self>, uri: String, snapshot: Vec<f32>) {
        let spare = uri.clone();
        let collector = Arc::clone(self);
        let worker = std::thread::Builder::new()
            .name("beat-analysis".into())
            .spawn(move || {
                if let Some(reading) = analyze(&snapshot, RATE) {
                    let mut results = collector
                        .results
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    results.retain(|(held, _)| held != &uri);
                    results.push((uri.clone(), reading));
                    let over = results.len().saturating_sub(REMEMBERED);
                    results.drain(0..over);
                }
                collector.state().in_flight.remove(&uri);
            });
        if let Err(error) = worker {
            // A failed spawn leaves the track marked in flight forever, so
            // unmark it by hand; the thread itself does this on the way out.
            log::warn!("Could not start the beat analysis thread: {error}");
            self.prune(spare);
        }
    }

    /// Drops `uri` from the in-flight set, only when its thread could not
    /// start; the thread removes its own mark when it finishes.
    fn prune(&self, uri: String) {
        self.state().in_flight.remove(&uri);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// A click track: a burst of filtered noise, decaying like a drum
    /// sample, every beat at `bpm`, and nothing between. Beats per minute
    /// as requested, no more, no less.
    fn clicks(seconds: f64, bpm: f64, rate: u32) -> Vec<f32> {
        let len = (seconds * rate as f64).round() as usize;
        let mut out = vec![0.0_f32; len];
        let period = rate as f64 * 60.0 / bpm;
        let onset = 512_usize;
        let mut next = 0.0_f64;
        while next < len as f64 {
            let at = next.round() as usize;
            for i in 0..onset {
                if at + i < len {
                    let decay = (-(i as f32) / onset as f32 * 7.0).exp();
                    let noise = (((i * 2654435761_usize) % 1000) as f32 / 1000.0) - 0.5;
                    out[at + i] = 2.0 * noise * decay;
                }
            }
            next += period;
        }
        out
    }

    /// Feeds the collector `seconds` of 44.1 kHz stereo made from `mono`,
    /// naming the track as the engine named it when `from_start`. The
    /// signal is held across decimation groups, so the collector's
    /// averaging reconstructs `mono` exactly.
    fn feed(collector: &Arc<Collector>, uri: &str, mono: &[f32], seconds: f64, from_start: bool) {
        collector.arm(uri);
        collector.begin_track(uri, from_start);
        let frames = (seconds * SOURCE_RATE).round() as usize;
        let mut written = 0_usize;
        let sample =
            |frame: usize| -> f64 { f64::from(mono.get(frame / DECIMATE).copied().unwrap_or(0.0)) };
        while written < frames {
            let chunk = (frames - written).min(1000);
            let stereo: Vec<f64> = (written..written + chunk)
                .flat_map(|frame| [sample(frame), sample(frame)])
                .collect();
            collector.push(&stereo);
            written += chunk;
        }
    }

    /// Feeds without naming the track first, as pushes that arrive between
    /// loads do.
    fn feed_without_naming(collector: &Arc<Collector>, mono: &[f32], seconds: f64) {
        let frames = (seconds * SOURCE_RATE).round() as usize;
        let mut written = 0_usize;
        let sample =
            |frame: usize| -> f64 { f64::from(mono.get(frame * DECIMATE).copied().unwrap_or(0.0)) };
        while written < frames {
            let chunk = (frames - written).min(1000);
            let stereo: Vec<f64> = (written..written + chunk)
                .flat_map(|frame| [sample(frame), sample(frame)])
                .collect();
            collector.push(&stereo);
            written += chunk;
        }
    }

    /// Waits up to five seconds at a time for the worker thread to answer.
    fn answered(collector: &Arc<Collector>, uri: &str) -> Reading {
        for _ in 0..250 {
            if let Some(reading) = collector.known(uri) {
                return reading;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("the analysis did not answer for {uri}");
    }

    #[test]
    fn the_collector_answers_an_armed_track_once() {
        let collector = Collector::new();
        collector.set_threshold(Some(8));
        let uri = "soundcloud:track:1";
        feed(&collector, uri, &clicks(10.0, 120.0, RATE), 10.0, true);
        let reading = answered(&collector, uri);
        assert!((reading.bpm as f64 - 120.0).abs() <= 1.5);
        // Again: answered already, no second analysis run.
        feed(&collector, uri, &clicks(2.0, 120.0, RATE), 2.0, false);
        assert!(collector.known(uri).is_some());
    }

    #[test]
    fn a_track_never_armed_is_never_answered() {
        let collector = Collector::new();
        collector.set_threshold(Some(1));
        // Feeds without arming.
        feed_without_naming(&collector, &clicks(4.0, 120.0, RATE), 4.0);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let results = collector.results.lock().unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn turning_the_feature_off_drops_what_was_collected() {
        let collector = Collector::new();
        collector.set_threshold(None);
        let uri = "soundcloud:track:off";
        feed(&collector, uri, &clicks(3.0, 120.0, RATE), 3.0, true);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(collector.known(uri).is_none());
    }

    #[test]
    fn a_seek_does_not_unhear_what_was_heard() {
        let collector = Collector::new();
        collector.set_threshold(Some(10));
        let uri = "soundcloud:track:seek";
        collector.arm(uri);
        collector.begin_track(uri, true);
        let mono = clicks(12.0, 120.0, RATE);
        // Half, then a seek somewhere else, then the rest.
        feed(&collector, uri, &mono, 6.0, true);
        collector.begin_track(uri, false);
        feed(&collector, uri, &mono, 6.0, false);
        let reading = answered(&collector, uri);
        assert!((reading.bpm as f64 - 120.0).abs() <= 2.0);
    }

    #[test]
    fn a_different_track_forgets_what_was_collected() {
        let collector = Collector::new();
        collector.set_threshold(Some(10));
        // Six seconds of one track, four of another; only the one that
        // reaches the threshold is answered.
        feed(
            &collector,
            "soundcloud:track:left",
            &clicks(6.0, 120.0, RATE),
            6.0,
            true,
        );
        feed(
            &collector,
            "soundcloud:track:right",
            &clicks(12.0, 140.0, RATE),
            12.0,
            true,
        );
        let reading = answered(&collector, "soundcloud:track:right");
        assert!((reading.bpm as f64 - 140.0).abs() <= 1.5);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(collector.known("soundcloud:track:left").is_none());
    }
}
