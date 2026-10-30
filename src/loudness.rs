//! Source-independent loudness matching for interleaved stereo 44.1 kHz PCM.
//!
//! Feed raw decoded audio, before EQ, volume and limiting. Spotify retains
//! its supplied track gain; its zero-pregain reference is -14 LUFS too.

use std::path::{Path, PathBuf};

use ebur128::{EbuR128, Mode};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

pub const SAMPLE_RATE: u32 = 44_100;
pub const TARGET_LUFS: f64 = -14.0;
pub const PREGAIN_DB: f64 = 0.0;
/// Enough for gated integrated loudness without holding an entire song.
pub const LOOKAHEAD_FRAMES: usize = SAMPLE_RATE as usize * 3;
const INITIAL_DB: f64 = -14.0;
const UPDATE_FRAMES: u64 = SAMPLE_RATE as u64;
const SAVE_FRAMES: u64 = SAMPLE_RATE as u64 * 10;

fn gain_db(lufs: f64) -> f64 {
    (TARGET_LUFS + PREGAIN_DB - lufs).clamp(-30.0, 12.0)
}

fn valid_lufs(lufs: f64) -> bool {
    lufs.is_finite() && (-70.0..=10.0).contains(&lufs)
}

/// Small readable files, keyed by a hash of the complete source URI. No
/// signed stream URLs or credentials belong here. Partial prefixes never
/// replace a longer measurement or a completed measurement.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Measurement {
    version: u32,
    uri: String,
    lufs: f64,
    frames: u64,
    complete: bool,
}

impl Measurement {
    fn valid(&self, uri: &str) -> bool {
        self.version == 1
            && self.uri == uri
            && valid_lufs(self.lufs)
            && self.frames >= SAMPLE_RATE as u64 * 2 / 5
    }
}

struct Cache {
    directory: Option<PathBuf>,
    writer: fastframe_file::Writer,
}

impl Cache {
    fn path(&self, uri: &str) -> Option<PathBuf> {
        Some(
            self.directory
                .as_ref()?
                .join(format!("{:x}.json", Sha1::digest(uri.as_bytes()))),
        )
    }

    fn load(&self, uri: &str) -> Option<Measurement> {
        let bytes = std::fs::read(self.path(uri)?).ok()?;
        let value: Measurement = serde_json::from_slice(&bytes).ok()?;
        value.valid(uri).then_some(value)
    }

    fn save(&self, measurement: &Measurement) {
        if let Some(path) = self.path(&measurement.uri)
            && let Ok(bytes) = serde_json::to_vec_pretty(measurement)
        {
            self.writer
                .write(&path, bytes, fastframe_file::Access::Private);
        }
    }
}

/// Slew in dB, once per stereo frame. Slow upward correction preserves
/// dynamics; downward correction settles sooner, with no packet boundary
/// steps. The final peak limiter remains downstream.
struct Ramp {
    db: f64,
    target: f64,
}

impl Ramp {
    fn next(&mut self) -> f64 {
        let rate = if self.target < self.db { 3.0 } else { 1.0 };
        let step = rate / f64::from(SAMPLE_RATE);
        self.db += (self.target - self.db).clamp(-step, step);
        10.0_f64.powf(self.db / 20.0)
    }
}

struct Track {
    uri: String,
    meter: EbuR128,
    frames: u64,
    update_at: u64,
    save_at: u64,
    /// Primed PCM will be replayed through process, not measured twice.
    primed_samples: usize,
    from_start: bool,
    cached: Option<Measurement>,
    ramp: Ramp,
    started: bool,
    lookahead: bool,
}

impl Track {
    fn new(uri: &str, from_start: bool, cached: Option<Measurement>) -> Self {
        let db = cached.as_ref().map_or(INITIAL_DB, |m| gain_db(m.lufs));
        Self {
            uri: uri.into(),
            // Histogram mode bounds memory even for multi-hour uploads.
            meter: EbuR128::new(2, SAMPLE_RATE, Mode::I | Mode::HISTOGRAM)
                .expect("fixed stereo R128 format"),
            frames: 0,
            update_at: UPDATE_FRAMES * 3,
            save_at: SAVE_FRAMES,
            primed_samples: 0,
            from_start,
            lookahead: cached.is_none(),
            cached,
            ramp: Ramp { db, target: db },
            started: false,
        }
    }

    fn measure(&mut self, samples: &[f64]) {
        if self.cached.as_ref().is_some_and(|m| m.complete) {
            return;
        }
        if self.meter.add_frames_f64(samples).is_err() {
            return;
        }
        self.frames += samples.len() as u64 / 2;
        if self.frames >= self.update_at {
            self.update_at = self.frames + UPDATE_FRAMES;
            self.retarget();
        }
    }

    fn retarget(&mut self) {
        // A replay's short intro is less informative than its cached prefix.
        if self
            .cached
            .as_ref()
            .is_some_and(|m| m.complete || self.frames < m.frames)
        {
            return;
        }
        if let Ok(lufs) = self.meter.loudness_global()
            && valid_lufs(lufs)
        {
            // Don't boost a short quiet intro. Only a sufficiently long
            // measurement (or a previous play) may raise above unity.
            let target = if self.frames < SAMPLE_RATE as u64 * 30 {
                gain_db(lufs).min(0.0)
            } else {
                gain_db(lufs)
            };
            if (target - self.ramp.target).abs() >= 0.5 {
                self.ramp.target = target;
            }
        }
    }

    fn snapshot(&self, complete: bool) -> Option<Measurement> {
        if !self.from_start || self.uri.is_empty() {
            return None;
        }
        let value = Measurement {
            version: 1,
            uri: self.uri.clone(),
            lufs: self.meter.loudness_global().ok()?,
            frames: self.frames,
            complete,
        };
        if !value.valid(&self.uri)
            || self
                .cached
                .as_ref()
                .is_some_and(|old| old.complete || (!complete && old.frames >= value.frames))
        {
            return None;
        }
        Some(value)
    }
}

/// One external producer's state. Construct and call only on playback
/// workers: loading a cached URI reads disk; writes use a background Writer.
/// Disabled processing is an exact bypass, including measurement and cache IO.
pub struct Processor {
    enabled: bool,
    cache: Cache,
    track: Track,
}

impl Processor {
    pub fn new(enabled: bool, directory: Option<&Path>) -> Self {
        Self {
            enabled,
            cache: Cache {
                directory: directory.map(Path::to_owned),
                writer: fastframe_file::Writer::new(),
            },
            // Unlabelled producers still get protection and measurement.
            track: Track::new("", false, None),
        }
    }

    /// Begin a new track, or restart measurement after a seek. Only a
    /// contiguous decode from zero may populate the persistent cache.
    pub fn begin_track(&mut self, uri: &str, from_start: bool) {
        self.finish_track(false);
        let cached = self.enabled.then(|| self.cache.load(uri)).flatten();
        self.track = Track::new(uri, from_start, cached);
    }

    pub fn needs_lookahead(&self) -> bool {
        self.enabled && self.track.lookahead && !self.track.started
    }

    /// Measure buffered raw PCM before the first write. Replay these exact
    /// samples through process afterwards, in order. No extra gain ramp is
    /// necessary before any samples have reached the device.
    pub fn prime(&mut self, samples: &[f64]) {
        if !self.needs_lookahead() {
            return;
        }
        self.track.lookahead = false;
        self.track.measure(samples);
        self.track.retarget();
        self.track.primed_samples = samples.len();
        self.track.ramp.db = self.track.ramp.target;
    }

    /// Measure and apply one gain to both channels, before EQ and limiting.
    pub fn process(&mut self, samples: &mut [f64]) {
        if !self.enabled {
            return;
        }
        let skip = self.track.primed_samples.min(samples.len());
        self.track.primed_samples -= skip;
        self.track.measure(&samples[skip..]);
        self.track.started = true;
        for frame in samples.chunks_exact_mut(2) {
            let gain = self.track.ramp.next();
            frame[0] *= gain;
            frame[1] *= gain;
        }
        if self.track.frames >= self.track.save_at {
            self.track.save_at = self.track.frames + SAVE_FRAMES;
            self.finish_track(false);
        }
    }

    /// `complete` means actual decoder EOF, never Stop, seek or URL refresh.
    /// Saving partial progress makes interrupted first plays useful too.
    pub fn finish_track(&mut self, complete: bool) {
        if self.enabled
            && let Some(value) = self.track.snapshot(complete)
        {
            self.cache.save(&value);
            self.track.cached = Some(value);
        }
    }
}

impl Drop for Processor {
    fn drop(&mut self) {
        self.finish_track(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amplitude: f64, seconds: usize) -> Vec<f64> {
        (0..SAMPLE_RATE as usize * seconds)
            .flat_map(|i| {
                let sample = amplitude
                    * (std::f64::consts::TAU * 1_000.0 * i as f64 / f64::from(SAMPLE_RATE)).sin();
                [sample; 2]
            })
            .collect()
    }

    #[test]
    fn r128_stereo_sine_has_known_loudness_and_rejects_silence() {
        let mut track = Track::new("test:1", true, None);
        track.measure(&sine(0.1, 3));
        // 1 kHz stereo sine at -20 dBFS peak: approximately -20 LUFS.
        assert!((track.meter.loudness_global().unwrap() + 20.0).abs() < 0.1);
        let mut silent = Track::new("test:2", true, None);
        silent.measure(&vec![0.0; SAMPLE_RATE as usize * 6]);
        assert!(silent.snapshot(true).is_none());
        assert_eq!(silent.ramp.target, INITIAL_DB);
    }

    #[test]
    fn gain_targets_spotify_reference_and_has_bounded_boost() {
        assert_eq!(gain_db(-8.0), -6.0);
        assert_eq!(gain_db(-20.0), 6.0);
        assert_eq!(gain_db(-60.0), 12.0);
        assert_eq!(gain_db(10.0), -24.0);
    }

    #[test]
    fn ramp_is_sample_clocked_slow_and_shared_by_both_channels() {
        let mut ramp = Ramp {
            db: -14.0,
            target: 0.0,
        };
        for _ in 0..SAMPLE_RATE {
            ramp.next();
        }
        assert!((ramp.db + 13.0).abs() < 1e-8);
        ramp.target = -20.0;
        for _ in 0..SAMPLE_RATE {
            ramp.next();
        }
        assert!((ramp.db + 16.0).abs() < 1e-8);
        let mut processor = Processor::new(true, None);
        let mut pcm = vec![0.5; 2_048];
        processor.process(&mut pcm);
        assert!(pcm.chunks_exact(2).all(|frame| frame[0] == frame[1]));
        assert!(pcm.iter().all(|v| *v < 0.11));
    }

    #[test]
    fn lookahead_is_not_measured_twice_and_matches_target() {
        let mut processor = Processor::new(true, None);
        processor.begin_track("soundcloud:tracks:1", true);
        let mut pcm = sine(0.5, 3);
        processor.prime(&pcm);
        assert!(!processor.needs_lookahead());
        processor.process(&mut pcm);
        assert_eq!(processor.track.frames, SAMPLE_RATE as u64 * 3);
        let mut output = Track::new("test:output", true, None);
        output.measure(&pcm);
        assert!((output.meter.loudness_global().unwrap() - TARGET_LUFS).abs() < 0.1);
    }

    #[test]
    fn cache_round_trip_is_atomic_and_completed_gain_stays_fixed() {
        let directory = tempfile::tempdir().unwrap();
        let uri = "youtube:video:example";
        let mut processor = Processor::new(true, Some(directory.path()));
        processor.begin_track(uri, true);
        let mut pcm = sine(0.5, 4);
        processor.process(&mut pcm);
        processor.finish_track(true);
        assert!(
            processor
                .cache
                .writer
                .flush(std::time::Duration::from_secs(5))
        );
        let first = processor.cache.load(uri).unwrap();
        assert!(first.complete);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        let mut loaded = Processor::new(true, Some(directory.path()));
        loaded.begin_track(uri, true);
        assert!(!loaded.needs_lookahead());
        assert_eq!(loaded.track.ramp.db, gain_db(first.lufs));
        loaded.process(&mut sine(0.01, 4));
        loaded.finish_track(true);
        assert_eq!(loaded.track.ramp.target, gain_db(first.lufs));
        assert_eq!(loaded.cache.load(uri).unwrap().lufs, first.lufs);
        // Neither a different URI nor a bad cache record is accepted.
        assert!(loaded.cache.load("soundcloud:tracks:example").is_none());
        std::fs::write(loaded.cache.path(uri).unwrap(), b"not json").unwrap();
        assert!(loaded.cache.load(uri).is_none());
    }

    #[test]
    fn seeks_and_shorter_prefixes_do_not_replace_a_better_measurement() {
        let mut track = Track::new("test:1", true, None);
        track.measure(&sine(0.5, 4));
        track.cached = track.snapshot(false);
        track.frames = SAMPLE_RATE as u64 * 3;
        assert!(track.snapshot(false).is_none());
        track.cached = None;
        track.from_start = false;
        assert!(track.snapshot(true).is_none());
    }

    #[test]
    fn cached_prefix_gain_survives_a_quiet_intro_and_silence_is_not_boosted() {
        let cached = Measurement {
            version: 1,
            uri: "test:1".into(),
            lufs: -8.0,
            frames: SAMPLE_RATE as u64 * 20,
            complete: false,
        };
        let mut track = Track::new("test:1", true, Some(cached));
        track.measure(&sine(0.01, 4));
        assert_eq!(track.ramp.target, -6.0);
        let mut fresh = Track::new("test:2", true, None);
        fresh.measure(&sine(0.01, 4));
        assert_eq!(fresh.ramp.target, 0.0);
        assert!(fresh.ramp.db < fresh.ramp.target);
    }

    #[test]
    fn partial_cache_is_replaced_by_a_longer_or_completed_measurement() {
        let directory = tempfile::tempdir().unwrap();
        let mut processor = Processor::new(true, Some(directory.path()));
        processor.begin_track("test:1", true);
        processor.process(&mut sine(0.5, 10));
        assert!(
            processor
                .cache
                .writer
                .flush(std::time::Duration::from_secs(5))
        );
        assert!(!processor.cache.load("test:1").unwrap().complete);
        processor.process(&mut sine(0.5, 1));
        processor.finish_track(true);
        assert!(
            processor
                .cache
                .writer
                .flush(std::time::Duration::from_secs(5))
        );
        let complete = processor.cache.load("test:1").unwrap();
        assert!(complete.complete);
        assert_eq!(complete.frames, SAMPLE_RATE as u64 * 11);
        processor.begin_track("test:1", false);
        processor.process(&mut sine(0.1, 4));
        processor.finish_track(true);
        assert_eq!(processor.cache.load("test:1").unwrap().lufs, complete.lufs);
    }

    #[test]
    fn setting_off_is_exact_unity_with_no_measurement_or_files() {
        let directory = tempfile::tempdir().unwrap();
        let mut processor = Processor::new(false, Some(directory.path()));
        processor.begin_track("soundcloud:tracks:1", true);
        let mut pcm = sine(0.8, 3);
        let original = pcm.clone();
        processor.prime(&pcm);
        processor.process(&mut pcm);
        processor.finish_track(true);
        assert_eq!(pcm, original);
        assert_eq!(processor.track.frames, 0);
        assert!(!processor.needs_lookahead());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
