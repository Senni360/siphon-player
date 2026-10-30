//! Source-independent loudness matching for interleaved stereo 44.1 kHz PCM.
//!
//! Feed raw decoded audio, before EQ, volume and limiting. Spotify retains
//! its supplied track gain; its zero-pregain reference is -14 LUFS too.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

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
    generation: Arc<CacheGeneration>,
    writer: fastframe_file::Writer,
}

struct CacheGeneration {
    // Low bit: reads/writes enabled. Other bits: invalidation epoch.
    // One atomic prevents an old clear from re-enabling a newer epoch.
    state: AtomicU64,
    write_barrier: Mutex<()>,
}

impl Default for CacheGeneration {
    fn default() -> Self {
        Self {
            state: AtomicU64::new(1),
            write_barrier: Mutex::new(()),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct PendingMeasurement {
    generation: u64,
    measurement: Measurement,
}

// Old sinks can still have queued writes when a replacement sink starts.
// Share the invalidation barrier across processors using the same directory.
fn cache_generation(directory: Option<&Path>) -> Arc<CacheGeneration> {
    type Generations = HashMap<PathBuf, Arc<CacheGeneration>>;
    static GENERATIONS: OnceLock<Mutex<Generations>> = OnceLock::new();
    let Some(directory) = directory else {
        return Arc::default();
    };
    let mut generations = GENERATIONS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    // Retain disabled epochs even with no sink or queued writer. A failed or
    // abandoned clear must not let a later sink reopen the previous account's
    // files. The application uses one fixed loudness directory per profile.
    Arc::clone(generations.entry(directory.to_owned()).or_default())
}

/// Prepared without disk IO; finish on a blocking worker. Dropping an
/// unfinished job safely leaves this directory's cache disabled.
pub struct Invalidation {
    directory: Option<PathBuf>,
    generation: Arc<CacheGeneration>,
    ticket: u64,
}

impl Invalidation {
    fn prepare(directory: Option<&Path>, generation: Arc<CacheGeneration>) -> Self {
        let old = generation
            .state
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some((state & !1).wrapping_add(2))
            })
            .expect("invalidation always advances its epoch");
        Self {
            directory: directory.map(Path::to_owned),
            generation,
            ticket: (old & !1).wrapping_add(2),
        }
    }

    /// Delete old records, then enable only this job's generation. A newer
    /// account transition supersedes this job even while deletion is running.
    pub fn finish(self) -> std::io::Result<()> {
        let _guard = self
            .generation
            .write_barrier
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.generation.state.load(Ordering::Acquire) != self.ticket {
            return Ok(());
        }
        if let Some(directory) = &self.directory {
            match std::fs::remove_dir_all(directory) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                result => result?,
            }
        }
        let _ = self.generation.state.compare_exchange(
            self.ticket,
            self.ticket | 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        Ok(())
    }
}

/// Disable old-account cache reads and writes synchronously, without waiting
/// for disk or an active sink. Call before installing the replacement account,
/// then hand the returned job to a blocking worker for `finish()`.
pub fn prepare_invalidation(directory: &Path) -> Invalidation {
    Invalidation::prepare(Some(directory), cache_generation(Some(directory)))
}

impl Cache {
    fn new(directory: Option<&Path>) -> Self {
        let generation = cache_generation(directory);
        let writes = Arc::clone(&generation);
        let writer = fastframe_file::Writer::with_write(move |path, bytes, access| {
            let pending: PendingMeasurement =
                serde_json::from_slice(bytes).map_err(std::io::Error::other)?;
            let _guard = writes
                .write_barrier
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let state = writes.state.load(Ordering::Acquire);
            if state >> 1 != pending.generation || state & 1 == 0 {
                return Ok(());
            }
            // Keep the barrier through atomic replacement: invalidation waits
            // for an in-flight write, removes it, then rejects queued old jobs.
            fastframe_file::write_with(path, access, |file| {
                serde_json::to_writer_pretty(file, &pending.measurement)
                    .map_err(std::io::Error::other)
            })
        });
        Self {
            directory: directory.map(Path::to_owned),
            generation,
            writer,
        }
    }

    fn generation(&self) -> u64 {
        self.generation.state.load(Ordering::Acquire) >> 1
    }

    fn path(&self, uri: &str) -> Option<PathBuf> {
        Some(
            self.directory
                .as_ref()?
                .join(format!("{:x}.json", Sha1::digest(uri.as_bytes()))),
        )
    }

    fn load(&self, uri: &str) -> Option<Measurement> {
        if self.generation.state.load(Ordering::Acquire) & 1 == 0 {
            return None;
        }
        let _guard = self
            .generation
            .write_barrier
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.generation.state.load(Ordering::Acquire) & 1 == 0 {
            return None;
        }
        let path = self.path(uri)?;
        if std::fs::metadata(&path).ok()?.len() > 4096 {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        let value: Measurement = serde_json::from_slice(&bytes).ok()?;
        value.valid(uri).then_some(value)
    }

    fn save(&self, measurement: &Measurement, generation: u64) {
        if let Some(path) = self.path(&measurement.uri)
            && let Ok(bytes) = serde_json::to_vec(&PendingMeasurement {
                generation,
                measurement: measurement.clone(),
            })
        {
            self.writer
                .write(&path, bytes, fastframe_file::Access::Private);
        }
    }

    fn invalidate(&self) -> std::io::Result<()> {
        Invalidation::prepare(self.directory.as_deref(), Arc::clone(&self.generation)).finish()
    }
}

/// Slew in dB, once per stereo frame. Slow upward correction preserves
/// dynamics; downward correction settles sooner, with no packet boundary
/// steps. The final peak limiter remains downstream.
#[derive(Clone, Copy)]
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
    generation: u64,
    track: Track,
}

impl Processor {
    pub fn new(enabled: bool, directory: Option<&Path>) -> Self {
        let cache = Cache::new(directory);
        let generation = cache.generation();
        Self {
            enabled,
            cache,
            generation,
            // Unlabelled producers still get protection and measurement.
            track: Track::new("", false, None),
        }
    }

    /// Begin a new track, or restart measurement after a seek. Only a
    /// contiguous decode from zero may populate the persistent cache.
    pub fn begin_track(&mut self, uri: &str, from_start: bool) {
        self.sync_generation();
        self.finish_track(false);
        let same_track = self.track.uri == uri && !uri.is_empty();
        let cached = self
            .enabled
            .then(|| {
                // A seek can precede the Writer's checkpoint reaching disk.
                same_track
                    .then(|| self.track.cached.clone())
                    .flatten()
                    .or_else(|| self.cache.load(uri))
            })
            .flatten();
        let continued_ramp =
            (same_track && !from_start && self.track.started).then_some(self.track.ramp);
        self.track = Track::new(uri, from_start, cached);
        if let Some(ramp) = continued_ramp {
            self.track.ramp = ramp;
            self.track.lookahead = false;
            self.track.started = true;
        }
    }

    /// Clear account-private measurements and invalidate old queued writes.
    /// This waits for disk IO and must run on a backend/playback worker.
    /// A decoder that keeps playing loses its cache identity until its next
    /// explicit begin_track; its audible gain is preserved.
    pub fn invalidate_cache(&mut self) -> std::io::Result<()> {
        let result = self.cache.invalidate();
        self.sync_generation();
        result
    }

    fn sync_generation(&mut self) {
        let generation = self.cache.generation();
        if self.generation != generation {
            let ramp = self.track.ramp;
            let started = self.track.started;
            self.track = Track::new("", false, None);
            self.track.ramp = ramp;
            self.track.started = started;
            self.track.lookahead = !started;
            self.generation = generation;
        }
    }

    pub fn needs_lookahead(&self) -> bool {
        self.enabled && self.track.lookahead && !self.track.started
    }

    /// Measure buffered raw PCM before the first write. Replay these exact
    /// samples through process afterwards, in order. No extra gain ramp is
    /// necessary before any samples have reached the device.
    pub fn prime(&mut self, samples: &[f64]) {
        self.sync_generation();
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
        self.sync_generation();
        if !self.enabled {
            return;
        }
        let skip = self.track.primed_samples.min(samples.len());
        self.track.primed_samples -= skip;
        self.track.measure(&samples[skip..]);
        self.track.started = true;
        for frame in samples.as_chunks_mut::<2>().0 {
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
        self.sync_generation();
        if self.enabled
            && let Some(value) = self.track.snapshot(complete)
        {
            self.cache.save(&value, self.generation);
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

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "siphon-loudness-{}-{:016x}",
                std::process::id(),
                rand::random::<u64>()
            ));
            std::fs::create_dir(&directory).unwrap();
            Self(directory)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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
        assert!(
            pcm.as_chunks::<2>()
                .0
                .iter()
                .all(|frame| frame[0] == frame[1])
        );
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
        let directory = TestDirectory::new();
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
        let directory = TestDirectory::new();
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
        let directory = TestDirectory::new();
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

    #[test]
    fn a_seek_keeps_the_current_gain_before_the_checkpoint_reaches_disk() {
        let mut processor = Processor::new(true, None);
        processor.begin_track("soundcloud:track:1", true);
        let mut pcm = sine(0.5, 3);
        processor.prime(&pcm);
        processor.process(&mut pcm);
        let prior_gain = processor.track.ramp.db;
        let prior_target = processor.track.ramp.target;
        processor.begin_track("soundcloud:track:1", false);
        assert_eq!(processor.track.ramp.db, prior_gain);
        assert_eq!(processor.track.ramp.target, prior_target);
        assert!(!processor.needs_lookahead());
        assert_eq!(
            processor.track.cached.as_ref().unwrap().frames,
            3 * u64::from(SAMPLE_RATE)
        );
        processor.process(&mut sine(0.01, 1));
        assert!((processor.track.ramp.db - prior_gain).abs() < 1e-10);
        assert!(processor.track.snapshot(true).is_none());
        processor.begin_track("youtube:video:other", true);
        assert_eq!(processor.track.ramp.db, INITIAL_DB);
        assert!(processor.needs_lookahead());
    }

    #[test]
    fn account_invalidation_rejects_queued_writes_and_other_sinks_old_state() {
        let directory = TestDirectory::new();
        let mut old = Processor::new(true, Some(directory.path()));
        old.begin_track("soundcloud:track:1", true);
        old.process(&mut sine(0.5, 4));
        old.finish_track(true);
        assert!(old.cache.writer.flush(std::time::Duration::from_secs(5)));
        let measurement = old.cache.load("soundcloud:track:1").unwrap();
        let old_generation = old.generation;
        let mut replacement = Processor::new(true, Some(directory.path()));
        replacement.invalidate_cache().unwrap();
        // A delayed job from an old sink must not resurrect deleted data.
        old.cache.save(&measurement, old_generation);
        assert!(old.cache.writer.flush(std::time::Duration::from_secs(5)));
        old.finish_track(true);
        assert!(old.track.uri.is_empty());
        assert!(old.track.cached.is_none());
        assert!(!directory.path().exists());
        replacement.begin_track("youtube:video:new", true);
        replacement.process(&mut sine(0.3, 4));
        replacement.finish_track(true);
        assert!(
            replacement
                .cache
                .writer
                .flush(std::time::Duration::from_secs(5))
        );
        assert!(replacement.cache.load("youtube:video:new").is_some());
        assert!(replacement.cache.load("soundcloud:track:1").is_none());
    }

    #[test]
    fn failed_invalidation_disables_old_cache_reads_until_a_successful_retry() {
        let directory = TestDirectory::new();
        let path = directory.path().join("cache");
        let backup = directory.path().join("saved");
        let mut processor = Processor::new(true, Some(&path));
        processor.begin_track("soundcloud:track:1", true);
        processor.process(&mut sine(0.5, 4));
        processor.finish_track(true);
        assert!(
            processor
                .cache
                .writer
                .flush(std::time::Duration::from_secs(5))
        );
        std::fs::rename(&path, &backup).unwrap();
        std::fs::write(&path, b"not a directory").unwrap();
        assert!(processor.invalidate_cache().is_err());
        drop(processor);
        std::fs::remove_file(&path).unwrap();
        std::fs::rename(&backup, &path).unwrap();
        // No old sink or invalidation job is alive. Its failed epoch must
        // still stop a newly created sink reopening these private records.
        let mut processor = Processor::new(true, Some(&path));
        assert!(processor.cache.load("soundcloud:track:1").is_none());
        assert!(processor.track.uri.is_empty());
        processor.invalidate_cache().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn preparing_without_a_sink_disables_reads_without_waiting_for_disk() {
        let directory = TestDirectory::new();
        let generation = cache_generation(Some(directory.path()));
        let barrier = generation.write_barrier.lock().unwrap();
        let invalidation = prepare_invalidation(directory.path());
        // Both preparation and an ensuing new-account load must return even
        // while an old durable write still owns the disk barrier.
        let mut new_account = Processor::new(true, Some(directory.path()));
        new_account.begin_track("soundcloud:track:1", true);
        assert!(new_account.needs_lookahead());
        assert_eq!(generation.state.load(Ordering::Acquire) & 1, 0);
        drop(barrier);
        invalidation.finish().unwrap();
        assert_eq!(generation.state.load(Ordering::Acquire) & 1, 1);
    }

    #[test]
    fn superseded_clears_cannot_enable_or_delete_a_newer_accounts_cache() {
        let directory = TestDirectory::new();
        let first = prepare_invalidation(directory.path());
        let generation = Arc::clone(&first.generation);
        let barrier = generation.write_barrier.lock().unwrap();
        let worker = std::thread::spawn(move || first.finish());
        let second = prepare_invalidation(directory.path());
        drop(barrier);
        worker.join().unwrap().unwrap();
        assert_eq!(generation.state.load(Ordering::Acquire), second.ticket);
        second.finish().unwrap();
        // Also exercise reversed completion order after a new account has
        // already successfully written its own measurement.
        let obsolete = prepare_invalidation(directory.path());
        prepare_invalidation(directory.path()).finish().unwrap();
        let mut current = Processor::new(true, Some(directory.path()));
        current.begin_track("youtube:video:new", true);
        current.process(&mut sine(0.5, 4));
        current.finish_track(true);
        assert!(
            current
                .cache
                .writer
                .flush(std::time::Duration::from_secs(5))
        );
        obsolete.finish().unwrap();
        assert!(current.cache.load("youtube:video:new").is_some());
    }

    #[test]
    fn processing_does_not_wait_for_a_pending_durable_cache_write() {
        let directory = TestDirectory::new();
        let mut processor = Processor::new(true, Some(directory.path()));
        processor.begin_track("soundcloud:track:1", true);
        let generation = Arc::clone(&processor.cache.generation);
        let barrier = generation.write_barrier.lock().unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            processor.process(&mut [0.0; 256]);
            sent.send(()).unwrap();
            processor
        });
        // Hold the disk writer's barrier while PCM runs. Only account
        // invalidation and cache loads may wait for this barrier.
        received
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        drop(barrier);
        drop(worker.join().unwrap());
    }

    #[test]
    fn a_late_loud_section_stays_finite_and_bounded_by_the_final_limiter() {
        let mut processor = Processor::new(true, None);
        processor.begin_track("soundcloud:track:1", true);
        let mut intro = sine(0.01, 3);
        processor.prime(&intro);
        processor.process(&mut intro);
        let mut limiter = crate::limiter::Limiter::new(f64::from(SAMPLE_RATE));
        limiter.process(&mut intro, 1.0);
        for _ in 0..4 {
            let mut chorus = sine(4.0, 1);
            let previous_db = processor.track.ramp.db;
            processor.process(&mut chorus);
            assert!(processor.track.ramp.db >= previous_db - 3.0 - 1e-8);
            limiter.process(&mut chorus, 1.0);
            assert!(
                chorus
                    .iter()
                    .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
            );
        }
    }
}
