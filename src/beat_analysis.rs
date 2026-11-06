//! Tempo and key detection for instrumentals played through Encore's own
//! SoundCloud and YouTube engine.
//!
//! Producers writing over a beat want its BPM and key. [`analyze`] is pure:
//! mono PCM in, a tempo estimate out and a key estimate with a confidence.
//! The tempo comes from an onset-strength envelope (spectral flux) whose
//! autocorrelation is scored with a comb filter over the integer multiples
//! of each candidate beat period, then folded into the 70–180 BPM range
//! producers name beats in, which handles half and double time. The key
//! comes from a chroma pitch-class profile matched against Krumhansl and
//! Kessler's major and minor tonal hierarchies.
//!
//! [`Collector`] is the session-long half. It taps only the external
//! engine's pre-volume PCM (see `crate::sink`): Spotify's own decoded audio
//! is written by a different branch of the sink and never reaches it. The
//! collector decimates what it hears to 11 kHz mono in a bounded buffer,
//! counts only seconds sound actually played (nothing arrives while the
//! output is paused), and once per track, after the configured number of
//! seconds, runs the analysis on a worker thread. Results stay in memory
//! for the session, keyed by track URI; audio is never written to disk or
//! sent anywhere.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The rate the collector's mono buffer is decimated to: a quarter of the
/// engine's 44.1 kHz, still enough for tempo (onsets) and key (harmonics
/// well above 5 kHz) at a quarter of the memory and analysis work.
pub const RATE: u32 = 11_025;

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

/// Below this Pearson correlation a key match is not named.
const KEY_CONFIDENT: f32 = 0.45;

/// Pitch-class names, sharps only, as producers name keys.
pub const PITCHES: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];

// Krumhansl and Kessler's tonal hierarchies: how well each pitch class
// fits a major key (C major) or a minor key (C minor), from probe-tone
// experiments. A track's chroma is matched against all 24 rotations.
// Values as given in Krumhansl, "Cognitive Foundations of Musical Pitch".
const MAJOR_PROFILE: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR_PROFILE: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// A detected musical key: a tonic pitch class (C = 0, B = 11) and mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub tonic: u8,
    pub minor: bool,
}

impl Key {
    /// "C♯ minor", as producers write it.
    pub fn name(&self) -> String {
        format!(
            "{} {}",
            PITCHES[self.tonic as usize],
            if self.minor { "minor" } else { "major" }
        )
    }

    /// "C♯m", the quiet player-bar form.
    pub fn compact(&self) -> String {
        let tonic = PITCHES[self.tonic as usize];
        if self.minor {
            format!("{tonic}m")
        } else {
            tonic.to_string()
        }
    }
}

/// One track's tempo and key, as the analysis heard them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// Whole beats per minute, folded into 70–180.
    pub bpm: u16,
    /// The key with the Pearson correlation of the track's chroma against
    /// the best Krumhansl profile, when that was confident enough to name.
    pub key: Option<(Key, f32)>,
}

/// The tempo and key of `mono` at `rate`, or [`None`] when there was too
/// little sound to answer.
pub fn analyze(mono: &[f32], rate: u32) -> Option<Reading> {
    if rate == 0 || mono.len() < rate as usize / 2 {
        return None;
    }
    let power: f64 = mono
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum();
    // Digital silence or an all-quiet intro cannot answer either question.
    if (power / mono.len() as f64).sqrt() < 1.0e-6 {
        return None;
    }
    let flux = flux_envelope(mono);
    let bpm = tempo(&flux, rate as f64 / ENV_HOP as f64);
    let key = chroma_key(mono, rate);
    let bpm = bpm.round().max(1.0) as u16;
    (bpm > 1).then_some(Reading { bpm, key })
}

/// Samples in one onset-analysis window, and how far successive windows
/// step. The step keeps the envelope near 43 Hz at [`RATE`], so one lag
/// step spans about 1.4 BPM at 120 bpm — fine enough once refined.
const ENV_WINDOW: usize = 1024;
const ENV_HOP: usize = 256;

/// The onset-strength envelope: spectral flux, the total rise in the
/// magnitude spectrum from one window to the next. Percussion announces
/// itself here whatever pitch it carries.
fn flux_envelope(mono: &[f32]) -> Vec<f32> {
    if mono.len() < ENV_WINDOW + ENV_HOP {
        return Vec::new();
    }
    let mut window: Vec<f32> = vec![0.0; ENV_WINDOW];
    // The first window starts the comparison; it has nothing before it.
    let mut previous: Option<Vec<f32>> = None;
    let mut flux = Vec::new();
    for at in (0..=mono.len() - ENV_WINDOW).step_by(ENV_HOP) {
        for (i, sample) in mono[at..at + ENV_WINDOW].iter().enumerate() {
            // A Hann window keeps the flux from ticking at the frame edges.
            let taper = 0.5 * (1.0 - (std::f32::consts::TAU * i as f32 / ENV_WINDOW as f32).cos());
            window[i] = sample * taper;
        }
        let mut real = window.clone();
        let mut imaginary = vec![0.0_f32; ENV_WINDOW];
        fft_pow2(&mut real, &mut imaginary);
        let magnitudes = real[1..ENV_WINDOW / 2]
            .iter()
            .zip(&imaginary[1..ENV_WINDOW / 2])
            .map(|(re, im)| (re * re + im * im).sqrt())
            .collect::<Vec<f32>>();
        let mut strength = 0.0_f32;
        for (rise, was) in previous
            .as_ref()
            .map_or(&[] as &[f32], |p| p)
            .iter()
            .zip(&magnitudes)
        {
            strength += (rise - was).max(0.0);
        }
        // After the first window, every rise is a note about what just
        // started playing; silence in the comparison keeps the envelope
        // flat, exactly as it should be.
        if previous.is_some() {
            flux.push(strength);
        }
        previous = Some(magnitudes);
    }
    // A hard onset rises across two or three frames of the windowed
    // spectrum; smoothing turns those adjacent spikes into one bump, so the
    // autocorrelation reads the beat, not the skirmish inside each onset.
    let smoothed: Vec<f32> = flux
        .windows(5)
        .map(|five| (five[0] + 2.0 * five[1] + 3.0 * five[2] + 2.0 * five[3] + five[4]) / 9.0)
        .collect();
    smoothed
}

/// The autocorrelation of `centered` at `lag`, linearly interpolated
/// between the two nearest whole lags so comb harmonics off the grid still
/// count. Lags past half the envelope read noise against noise and stay
/// out.
fn autocorrelation(centered: &[f32], lag: f64, usable: usize) -> f32 {
    let whole = lag.floor() as usize;
    let part = (lag - lag.floor()) as f32;
    let value = |at: usize| -> f32 {
        if at == 0 || at >= usable || at >= centered.len() {
            return 0.0;
        }
        let tail = centered.len() - at;
        centered[..tail]
            .iter()
            .zip(&centered[at..])
            .map(|(a, b)| a * b)
            .sum::<f32>()
            / tail as f32
    };
    value(whole) * (1.0 - part) + value(whole + 1) * part
}

/// The tempo, in beats per minute, of an onset envelope sampled
/// `env_rate` times a second. Each candidate period is scored by a comb
/// filter: the autocorrelation at the period and at its first three
/// multiples, discounted, so the rate that keeps explaining the envelope's
/// repeats wins. Lags cover raw tempos of 30 to 230 bpm and the winner is
/// then folded into 70–180, which reads half-time and double-time cases
/// as producers would call the track.
fn tempo(flux: &[f32], env_rate: f64) -> f32 {
    const SLOWEST_BPM: f64 = 30.0;
    const FASTEST_BPM: f64 = 230.0;
    if flux.len() < 32 {
        return 0.0;
    }
    let mean = flux.iter().sum::<f32>() / flux.len() as f32;
    let centered: Vec<f32> = flux.iter().map(|value| value - mean).collect();
    let usable = centered.len() / 2;
    let fastest = env_rate * 60.0 / FASTEST_BPM;
    let slowest = env_rate * 60.0 / SLOWEST_BPM;
    if usable as f64 <= fastest || slowest < fastest {
        return 0.0;
    }
    let last = slowest.min(usable as f64 - 1.0);
    let comb = |lag: f64| -> f32 {
        let mut score = 0.0_f32;
        let mut share = 1.0_f32;
        for multiple in 1..=4 {
            score += autocorrelation(&centered, lag * f64::from(multiple), usable) * share;
            share *= 0.85;
        }
        score
    };
    // The beat period rarely lands on a whole envelope frame, so the comb
    // is read on a fine grid between frames; 0.05 of a frame lands the
    // reading within about a fiftieth of the beat.
    let step = 0.05_f64;
    let mut best_score = f32::NEG_INFINITY;
    let mut lag = fastest;
    while lag <= last {
        best_score = best_score.max(comb(lag));
        lag += step;
    }
    // A perfectly periodic train scores as well at twice and three times
    // the beat period as at the beat itself, so the loudest lag can sit on
    // any of the beat's own multiples. The period a listener would tap is
    // the fastest one that still stands near the loudest; from there, one
    // climb lands on that period's own peak.
    if !best_score.is_finite() {
        return 0.0;
    }
    let near_best = 0.8 * best_score;
    let mut chosen = None;
    let mut lag = fastest;
    while lag <= last {
        if comb(lag) >= near_best {
            chosen = Some(lag);
            break;
        }
        lag += step;
    }
    let Some(found) = chosen else {
        return 0.0;
    };
    let mut found = found;
    while found + step <= last {
        let here = comb(found);
        let ahead = comb(found + step);
        if ahead > here {
            found += step;
        } else {
            break;
        }
    }
    fold_bpm(60.0 * env_rate / found) as f32
}

/// A raw tempo folded into the 70–180 BPM range producers name beats in.
/// The fold's edges sit a couple of beats outside that range, so a track
/// genuinely at 69 or 181 is not thrown to the other end of the octave.
fn fold_bpm(bpm: f64) -> f64 {
    let mut folded = bpm;
    while folded < 68.0 {
        folded *= 2.0;
    }
    while folded > 182.0 {
        folded /= 2.0;
    }
    folded
}

/// The chroma of `mono`: the share of its spectral energy each pitch class
/// carries, across evenly spaced windows over the whole hold. Tuning is
/// assumed to be A4 = 440, as SoundCloud and YouTube streams are.
fn chroma(mono: &[f32], rate: u32) -> Option<[f32; 12]> {
    const WINDOW: usize = 8192;
    const WINDOWS: usize = 12;
    if mono.len() < WINDOW {
        return None;
    }
    let mut profile = [0.0_f32; 12];
    for i in 0..WINDOWS {
        let at = i * (mono.len() - WINDOW) / WINDOWS;
        let mut imaginary = vec![0.0_f32; WINDOW];
        let mut real: Vec<f32> = mono[at..at + WINDOW]
            .iter()
            .enumerate()
            .map(|(i, sample)| {
                let taper = 0.5 * (1.0 - (std::f32::consts::TAU * i as f32 / WINDOW as f32).cos());
                sample * taper
            })
            .collect();
        fft_pow2(&mut real, &mut imaginary);
        for bin in 1..WINDOW / 2 {
            let frequency = bin as f64 * rate as f64 / WINDOW as f64;
            // Rumble below the bass and hiss above the last musical
            // harmonics say nothing about key.
            if !(55.0..5000.0).contains(&frequency) {
                continue;
            }
            // Nearest semitone of A4 = 440 as a MIDI pitch, then its pitch
            // class.
            let midi = 69.0 + 12.0 * (frequency / 440.0).log2();
            let class = midi.round() as i64 % 12;
            let class = class.rem_euclid(12) as usize;
            profile[class] += (real[bin] * real[bin] + imaginary[bin] * imaginary[bin]).sqrt();
        }
    }
    let total: f32 = profile.iter().sum();
    if total <= f32::EPSILON {
        return None;
    }
    for value in &mut profile {
        *value /= total;
    }
    Some(profile)
}

/// The key whose Krumhansl–Kessler profile best matches the track's
/// chroma, with the Pearson correlation of the match as its confidence.
/// [`None`] when the chroma is too flat for that to mean anything.
fn chroma_key(mono: &[f32], rate: u32) -> Option<(Key, f32)> {
    let profile = chroma(mono, rate)?;
    let mut best: Option<(Key, f32)> = None;
    for tonic in 0..12_usize {
        for minor in [false, true] {
            let hierarchy = match minor {
                false => MAJOR_PROFILE,
                true => MINOR_PROFILE,
            };
            // The hierarchies are written for a tonic of C; rotating one
            // lines its tonic up with the candidate's.
            let rotated: [f32; 12] = std::array::from_fn(|i| hierarchy[(i + 12 - tonic) % 12]);
            let confidence = correlate(&profile, &rotated);
            if best.is_none_or(|(_, kept)| confidence > kept) {
                best = Some((
                    Key {
                        tonic: tonic as u8,
                        minor,
                    },
                    confidence,
                ));
            }
        }
    }
    best.filter(|(_, value)| *value >= KEY_CONFIDENT)
}

/// The Pearson correlation of two 12-place vectors.
fn correlate(chroma: &[f32; 12], profile: &[f32; 12]) -> f32 {
    let mean_left = chroma.iter().sum::<f32>() / 12.0;
    let mean_right = profile.iter().sum::<f32>() / 12.0;
    let mut covariance = 0.0_f32;
    let mut squares = (0.0_f32, 0.0_f32);
    for (left, right) in chroma.iter().zip(profile) {
        let (left, right) = (left - mean_left, right - mean_right);
        covariance += left * right;
        squares.0 += left * left;
        squares.1 += right * right;
    }
    let denominator = (squares.0 * squares.1).sqrt();
    if denominator > f32::EPSILON {
        covariance / denominator
    } else {
        0.0
    }
}

/// An in-place radix-2 complex FFT; `n` a power of two. Cooley–Tukey: the
/// samples are unshuffled by bit reversal, then merged by butterflies of
/// doubling size. Only the analysis calls this, off the UI thread.
fn fft_pow2(real: &mut [f32], imaginary: &mut [f32]) {
    let n = real.len();
    if n < 2 || n.count_ones() != 1 {
        return;
    }
    let mut swap = 0_usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while swap & bit != 0 {
            swap &= !bit;
            bit >>= 1;
        }
        swap |= bit;
        if i < swap {
            real.swap(i, swap);
            imaginary.swap(i, swap);
        }
    }
    let mut length = 2_usize;
    while length <= n {
        let step = -std::f32::consts::TAU / length as f32;
        for start in (0..n).step_by(length) {
            let mut angle = 0.0_f32;
            for k in 0..length / 2 {
                let (i, j) = (start + k, start + k + length / 2);
                let (twiddle_re, twiddle_im) = (angle.cos(), angle.sin());
                let (product_re, product_im) = (
                    real[j] * twiddle_re - imaginary[j] * twiddle_im,
                    real[j] * twiddle_im + imaginary[j] * twiddle_re,
                );
                real[j] = real[i] - product_re;
                imaginary[j] = imaginary[i] - product_im;
                real[i] += product_re;
                imaginary[i] += product_im;
                angle += step;
            }
        }
        length *= 2;
    }
}

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

    /// A steady chord's or melody's samples: `notes` as MIDI numbers, each
    /// stepping `step` seconds.
    fn notes(seconds: f64, midi: &[u8], rate: u32) -> Vec<f32> {
        let len = (seconds * rate as f64).round() as usize;
        let mut out = vec![0.0_f32; len];
        for (index, m) in midi.iter().enumerate() {
            let frequency = 440.0 * 2.0_f64.powf((i32::from(*m) - 69) as f64 / 12.0);
            let (start, end) = ((len * index / midi.len()), (len * (index + 1) / midi.len()));
            for (i, sample) in out[start..end].iter_mut().enumerate() {
                let t = (i + start) as f32 / rate as f32;
                // A plucked voice: a fundamental with soft harmonics and a
                // decay, repeating every half second, so there are onsets.
                let phase = t % 0.5;
                let envelope = (-phase * 6.0).exp();
                *sample = 0.6 * envelope * (std::f32::consts::TAU * frequency as f32 * t).sin()
                    + 0.2 * envelope * (std::f32::consts::TAU * frequency as f32 * 2.0 * t).sin()
                    + 0.1 * envelope * (std::f32::consts::TAU * frequency as f32 * 3.0 * t).sin();
            }
        }
        out
    }

    #[test]
    fn a_click_track_reads_back_its_own_tempo() {
        for (bpm, leeway) in [
            (70.0, 1.5),
            (85.0, 1.5),
            (100.0, 1.0),
            (120.0, 1.0),
            (140.0, 1.0),
            (160.0, 1.0),
            (175.0, 1.5),
        ] {
            let samples = clicks(20.0, bpm, RATE);
            let reading = analyze(&samples, RATE).expect("there was plenty of sound");
            assert!(
                (f64::from(reading.bpm) - bpm).abs() <= leeway,
                "clicked at {bpm} bpm, read {reading:?}"
            );
        }
    }

    #[test]
    fn half_and_double_time_tempos_fold_into_the_named_range() {
        // Asking below the range reads back doubled; asking above it,
        // halved, as producers would call each track.
        let slow = analyze(&clicks(30.0, 40.0, RATE), RATE).expect("sound");
        assert!((70..=180).contains(&slow.bpm), "{slow:?}");
        assert!((slow.bpm as f64 - 80.0).abs() <= 2.0, "{slow:?}");
        let fast = analyze(&clicks(30.0, 320.0, RATE), RATE).expect("sound");
        assert!((fast.bpm as f64 - 160.0).abs() <= 3.0, "{fast:?}");
    }

    #[test]
    fn a_major_chord_reads_back_its_key() {
        // C major: C, E, G and an octave of C.
        let samples = notes(12.0, &[60, 64, 67, 72], RATE);
        let reading = analyze(&samples, RATE).expect("sound");
        let (key, confidence) = reading.key.expect("a clear chord has a key");
        assert_eq!(key.name(), "C major");
        assert_eq!(key.compact(), "C");
        assert!(confidence > 0.6, "confidence {confidence}");
    }

    #[test]
    fn a_minor_chord_reads_back_its_key() {
        // A minor: A, C, E.
        let samples = notes(12.0, &[57, 60, 64], RATE);
        let reading = analyze(&samples, RATE).expect("sound");
        let (key, confidence) = reading.key.expect("a clear chord has a key");
        assert_eq!(key.name(), "A minor");
        assert_eq!(key.compact(), "Am");
        assert!(confidence > 0.6, "confidence {confidence}");
    }

    #[test]
    fn a_natural_minor_scale_reads_back_its_key() {
        // C natural minor: C D E♭ F G A♭ B♭, as a melody that keeps coming
        // home to its tonic and ends on its octave.
        let samples = notes(
            14.0,
            &[60, 62, 63, 65, 67, 70, 68, 67, 65, 63, 62, 60, 72],
            RATE,
        );
        let reading = analyze(&samples, RATE).expect("sound");
        let (key, confidence) = reading.key.expect("a scale has a key");
        assert_eq!(key.name(), "C minor");
        assert_eq!(key.compact(), "Cm");
        assert!(confidence > 0.5, "confidence {confidence}");
    }

    #[test]
    fn silence_has_no_answer() {
        let nothing = vec![0.0_f32; RATE as usize * 8];
        assert!(analyze(&nothing, RATE).is_none());
    }

    #[test]
    fn krumhansl_profiles_prefer_a_sensible_tonic() {
        // A chroma of only C, F and G, the harmonic skeleton of C major,
        // lands on C major and not its relative A minor.
        let mut skeleton = [0.0_f32; 12];
        skeleton[0] = 0.5;
        skeleton[5] = 0.3;
        skeleton[7] = 0.2;
        let mut best: Option<(Key, f32)> = None;
        for tonic in 0..12_usize {
            for minor in [false, true] {
                let profile = match minor {
                    false => MAJOR_PROFILE,
                    true => MINOR_PROFILE,
                };
                let rotated: [f32; 12] = std::array::from_fn(|i| profile[(i + 12 - tonic) % 12]);
                let value = correlate(&skeleton, &rotated);
                if best.is_none_or(|(_, kept)| value > kept) {
                    best = Some((
                        Key {
                            tonic: tonic as u8,
                            minor,
                        },
                        value,
                    ));
                }
            }
        }
        let (key, _) = best.expect("some correlation");
        assert_eq!(key.name(), "C major");
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
