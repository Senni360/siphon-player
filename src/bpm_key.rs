//! A song's BPM and key, worked out from its own audio, and the state a row
//! shows while that happens.
//!
//! [`analyze`] is pure: mono PCM in, a tempo estimate and a key estimate with
//! a confidence out. The tempo comes from the autocorrelation of a
//! spectral-flux envelope, read with a comb filter over the beat period's
//! multiples, then checked against each frequency band so a hi-hat
//! subdivision is not mistaken for the beat. The key comes from a chroma
//! built at spectral peaks, with the tuning estimated first so a detuned 808
//! still lands on its own semitone, matched against Krumhansl and Kessler's
//! major and minor tonal hierarchies with the bass root breaking ties
//! between a key and its relative.
//!
//! [`State`] is the per-song half the interface reads: one request per song,
//! its stage, its reading or the reason it failed. [`Cache`] keeps readings
//! across launches in a small versioned JSON file that the app hands to its
//! `fastframe_file::Writer`, never to the drawing thread.
//!
//! The passive collector in [`crate::beat_analysis`] uses the same
//! [`analyze`], so a beat heard while it plays and a beat asked for from a
//! row read the same way.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::model::Loadable;

/// The rate the analysis works at: a quarter of the engine's 44.1 kHz, still
/// enough for tempo (onsets) and key (harmonics well above 5 kHz) at a
/// quarter of the work. Audio at another rate is resampled to this.
pub const RATE: u32 = 11_025;

/// Pitch-class names, sharps only, as producers name keys.
pub const PITCHES: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];

/// Below this Pearson correlation a key match is not named.
const KEY_CONFIDENT: f32 = 0.45;

/// How much a bass note on the tonic counts when two keys share a chroma.
const BASS_WEIGHT: f32 = 0.25;

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    #[serde(default)]
    pub tonic: u8,
    #[serde(default)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    /// Whole beats per minute, folded into 68–182.
    #[serde(default)]
    pub bpm: u16,
    /// The key with the Pearson correlation of the track's chroma against
    /// the best Krumhansl profile, when that was confident enough to name.
    #[serde(default)]
    pub key: Option<(Key, f32)>,
}

/// The tempo and key of `mono` at `rate`, or [`None`] when there was too
/// little sound to answer.
pub fn analyze(mono: &[f32], rate: u32) -> Option<Reading> {
    static NEVER: AtomicBool = AtomicBool::new(false);
    analyze_cancellable(mono, rate, &NEVER)
}

/// As [`analyze`], stopping early when `cancel` is set. A cancelled reading
/// is [`None`], exactly like silence: the caller that set the flag discards
/// the answer anyway.
pub fn analyze_cancellable(mono: &[f32], rate: u32, cancel: &AtomicBool) -> Option<Reading> {
    if rate == 0 || mono.len() < rate as usize / 2 || cancel.load(Ordering::Relaxed) {
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
    let resampled = (rate != RATE).then(|| resample(mono, rate, RATE));
    let samples = resampled.as_deref().unwrap_or(mono);
    let bpm = tempo(samples, cancel)?;
    let key = chroma_key(samples, cancel);
    let bpm = bpm.round().max(1.0) as u16;
    (bpm > 1).then_some(Reading { bpm, key })
}

/// `mono` moved to `to` hertz, averaging when it comes down and
/// interpolating when it goes up. The analysis only needs a rough 11 kHz
/// picture, so a box filter before decimation is enough anti-aliasing.
fn resample(mono: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || from == 0 || to == 0 || mono.is_empty() {
        return mono.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let length = (mono.len() as f64 / ratio).floor().max(1.0) as usize;
    let mut out = Vec::with_capacity(length);
    if from > to {
        let half = ratio / 2.0;
        for index in 0..length {
            let center = index as f64 * ratio;
            let start = (center - half).max(0.0) as usize;
            let end = ((center + half).ceil() as usize).min(mono.len());
            let end = end.max(start + 1).min(mono.len());
            let sum: f32 = mono[start..end].iter().sum();
            out.push(sum / (end - start) as f32);
        }
    } else {
        for index in 0..length {
            let position = index as f64 * ratio;
            let left = position.floor() as usize;
            let fraction = (position - left as f64) as f32;
            let first = mono.get(left).copied().unwrap_or(0.0);
            let second = mono.get(left + 1).copied().unwrap_or(first);
            out.push(first + (second - first) * fraction);
        }
    }
    out
}

/// Samples in one onset-analysis window, and how far successive windows
/// step. The step keeps the envelope near 43 Hz at [`RATE`], so one lag
/// step spans about 1.4 BPM at 120 bpm — fine enough once refined.
const ENV_WINDOW: usize = 1024;
const ENV_HOP: usize = 256;

/// The four onset-strength envelopes: everything audible, then the low band
/// (kick and 808), the mid band (snare body and melody) and the high band
/// (hi-hats). A subdivision shows in one band while the beat shows in
/// another, which is what stops a hat from doubling the tempo.
struct Envelopes {
    broad: Vec<f32>,
    low: Vec<f32>,
    mid: Vec<f32>,
    high: Vec<f32>,
}

/// The onset-strength envelopes: spectral flux, the total rise in the
/// magnitude spectrum from one window to the next, in four frequency bands.
/// Percussion announces itself here whatever pitch it carries.
fn flux_envelopes(mono: &[f32], cancel: &AtomicBool) -> Option<Envelopes> {
    if mono.len() < ENV_WINDOW + ENV_HOP {
        return None;
    }
    let taper: Vec<f32> = (0..ENV_WINDOW)
        .map(|i| 0.5 * (1.0 - (std::f32::consts::TAU * i as f32 / ENV_WINDOW as f32).cos()))
        .collect();
    let mut window = vec![0.0_f32; ENV_WINDOW];
    let mut previous: Option<[f32; 4]> = None;
    let mut broad = Vec::new();
    let mut low = Vec::new();
    let mut mid = Vec::new();
    let mut high = Vec::new();
    for (frame, at) in (0..=mono.len() - ENV_WINDOW).step_by(ENV_HOP).enumerate() {
        if frame % 32 == 0 && cancel.load(Ordering::Relaxed) {
            return None;
        }
        for (i, sample) in mono[at..at + ENV_WINDOW].iter().enumerate() {
            window[i] = sample * taper[i];
        }
        let mut real = window.clone();
        let mut imaginary = vec![0.0_f32; ENV_WINDOW];
        fft_pow2(&mut real, &mut imaginary);
        let mut sums = [0.0_f32; 3];
        for bin in 1..ENV_WINDOW / 2 {
            let frequency = bin as f64 * RATE as f64 / ENV_WINDOW as f64;
            // Rumble below the bass and hiss above the last musical
            // harmonics say nothing about either question.
            if !(30.0..5500.0).contains(&frequency) {
                continue;
            }
            let magnitude = (real[bin] * real[bin] + imaginary[bin] * imaginary[bin]).sqrt();
            // Log compression keeps a loud kick from drowning a quiet hat,
            // and the flux only counts a rise, so a fade adds nothing.
            let level = (1.0 + magnitude).ln();
            let band = if frequency < 150.0 {
                0
            } else if frequency < 2000.0 {
                1
            } else {
                2
            };
            sums[band] += level;
        }
        let current = [sums[0] + sums[1] + sums[2], sums[0], sums[1], sums[2]];
        if let Some(was) = previous {
            broad.push((current[0] - was[0]).max(0.0));
            low.push((current[1] - was[1]).max(0.0));
            mid.push((current[2] - was[2]).max(0.0));
            high.push((current[3] - was[3]).max(0.0));
        }
        previous = Some(current);
    }
    Some(Envelopes {
        broad: normalize(&smooth5(&broad)),
        low: normalize(&smooth5(&low)),
        mid: normalize(&smooth5(&mid)),
        high: normalize(&smooth5(&high)),
    })
}

/// A hard onset rises across two or three frames of the windowed spectrum;
/// smoothing turns those adjacent spikes into one bump, so the
/// autocorrelation reads the beat, not the skirmish inside each onset.
fn smooth5(values: &[f32]) -> Vec<f32> {
    values
        .windows(5)
        .map(|five| (five[0] + 2.0 * five[1] + 3.0 * five[2] + 2.0 * five[3] + five[4]) / 9.0)
        .collect()
}

/// Mean removed and standard deviation scaled to one, so bands with
/// different loudness compare on the same footing.
fn normalize(values: &[f32]) -> Vec<f32> {
    if values.is_empty() {
        return Vec::new();
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let variance = values
        .iter()
        .map(|value| (value - mean) * (value - mean))
        .sum::<f32>()
        / values.len() as f32;
    let deviation = variance.sqrt().max(1.0e-9);
    values
        .iter()
        .map(|value| (value - mean) / deviation)
        .collect()
}

/// The autocorrelation of `centered` at every whole lag up to `max_lag`.
/// Lags past half the envelope read noise against noise and stay out.
fn autocorrelation(values: &[f32], max_lag: usize, cancel: &AtomicBool) -> Option<Vec<f32>> {
    let usable = values.len() / 2;
    let max_lag = max_lag.min(usable);
    let mut out = vec![0.0_f32; max_lag + 1];
    for (lag, slot) in out.iter_mut().enumerate() {
        if lag % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return None;
        }
        let tail = values.len() - lag;
        if tail == 0 {
            break;
        }
        *slot = values[..tail]
            .iter()
            .zip(&values[lag..])
            .map(|(a, b)| a * b)
            .sum::<f32>()
            / tail as f32;
    }
    Some(out)
}

/// The autocorrelation at a fractional lag, interpolated between the two
/// nearest whole lags so comb harmonics off the grid still count.
fn autocorrelation_at(autocorrelation: &[f32], lag: f64) -> f32 {
    let whole = lag.floor() as usize;
    let part = (lag - whole as f64) as f32;
    if whole + 1 >= autocorrelation.len() {
        return 0.0;
    }
    autocorrelation[whole] * (1.0 - part) + autocorrelation[whole + 1] * part
}

/// A candidate period's score: the autocorrelation at the period and at its
/// first three multiples, discounted and averaged. Averaging rather than
/// summing keeps a slow lag from winning just because it has more teeth.
fn comb(autocorrelation: &[f32], lag: f64) -> f32 {
    let mut score = 0.0_f32;
    let mut share = 1.0_f32;
    let mut total = 0.0_f32;
    for multiple in 1..=4 {
        score += autocorrelation_at(autocorrelation, lag * f64::from(multiple)) * share;
        total += share;
        share *= 0.85;
    }
    score / total
}

/// The beat period of one envelope, in frames. The fastest lag that still
/// stands near the best one wins, because a perfectly periodic train scores
/// as well at twice the beat as at the beat itself. From there one climb
/// lands on that period's own peak.
fn best_period(envelope: &[f32], env_rate: f64, cancel: &AtomicBool) -> Option<f64> {
    best_period_near(envelope, env_rate, 0.97, cancel)
}

/// As [`best_period`], with the share of the best score a slower period must
/// keep to still count. The combined envelope uses a looser share so a fast
/// subdivision is not mistaken for the beat; a single band keeps the strict
/// one so an accent two beats apart is not taken for the beat.
fn best_period_near(
    envelope: &[f32],
    env_rate: f64,
    near_share: f32,
    cancel: &AtomicBool,
) -> Option<f64> {
    const SLOWEST_BPM: f64 = 40.0;
    const FASTEST_BPM: f64 = 240.0;
    if envelope.len() < 32 || envelope.iter().all(|value| value.abs() < 1.0e-6) {
        return None;
    }
    let fastest = env_rate * 60.0 / FASTEST_BPM;
    let slowest = env_rate * 60.0 / SLOWEST_BPM;
    let max_lag = (slowest * 4.0).ceil() as usize;
    let autocorrelation = autocorrelation(envelope, max_lag, cancel)?;
    let step = 0.02;
    let mut best = f32::NEG_INFINITY;
    let mut lag = fastest;
    while lag <= slowest {
        best = best.max(comb(&autocorrelation, lag));
        lag += step;
    }
    if !best.is_finite() {
        return None;
    }
    // The old 0.8 was a subdivision bias; even 0.97 is too eager on a single
    // band, where an accent two beats apart lines its harmonics up with the
    // beat.
    let near = near_share * best;
    let mut found = slowest;
    let mut lag = fastest;
    while lag <= slowest {
        if comb(&autocorrelation, lag) >= near {
            found = lag;
            break;
        }
        lag += step;
    }
    while found + step <= slowest
        && comb(&autocorrelation, found + step) > comb(&autocorrelation, found)
    {
        found += step;
    }
    Some(found)
}

/// The best phase for `lag`, then a weighted line through the onset peaks
/// nearest each grid point. The line's slope is the period from the events
/// themselves, accurate well inside a frame even at a fast tempo.
fn refine_grid(envelope: &[f32], lag: f64, cancel: &AtomicBool) -> Option<f64> {
    let length = envelope.len();
    if length < 32 || lag <= 0.0 {
        return None;
    }
    let mut best_phase = 0.0_f64;
    let mut best = f32::NEG_INFINITY;
    for phase_index in 0..lag.ceil() as usize {
        if phase_index % 64 == 0 && cancel.load(Ordering::Relaxed) {
            return None;
        }
        let phase = phase_index as f64;
        let mut on = 0.0_f32;
        let mut count = 0_usize;
        let mut at = phase;
        while at + lag < length as f64 {
            let index = at.round() as usize;
            let before = index.saturating_sub(1);
            let after = (index + 1).min(length - 1);
            on += (envelope[before] + envelope[index] + envelope[after]) / 3.0;
            count += 1;
            at += lag;
        }
        if count > 2 {
            let value = on / count as f32;
            if value > best {
                best = value;
                best_phase = phase;
            }
        }
    }
    let mut points: Vec<(f64, f64, f64)> = Vec::new();
    let mut index = 0_usize;
    let mut at = best_phase;
    while at + lag < length as f64 {
        let center = at.round() as usize;
        let start = center.saturating_sub(2);
        let end = (center + 2).min(length - 1);
        let mut peak = center;
        let mut peak_value = f32::NEG_INFINITY;
        for (i, value) in envelope.iter().enumerate().take(end + 1).skip(start) {
            if *value > peak_value {
                peak_value = *value;
                peak = i;
            }
        }
        if peak_value > 0.2 {
            let before = envelope[peak.saturating_sub(1)];
            let here = envelope[peak];
            let after = envelope[(peak + 1).min(length - 1)];
            let denominator = before - 2.0 * here + after;
            let delta = if denominator.abs() > 1.0e-9 {
                0.5 * (before - after) / denominator
            } else {
                0.0
            };
            let position = peak as f64 + f64::from(delta.clamp(-0.5, 0.5));
            points.push((index as f64, position, f64::from(peak_value)));
        }
        index += 1;
        at += lag;
    }
    if points.len() < 6 {
        return None;
    }
    let fit = |points: &[(f64, f64, f64)]| -> Option<f64> {
        let weight: f64 = points.iter().map(|point| point.2).sum();
        if weight <= 0.0 {
            return None;
        }
        let mean_index = points.iter().map(|point| point.2 * point.0).sum::<f64>() / weight;
        let mean_position = points.iter().map(|point| point.2 * point.1).sum::<f64>() / weight;
        let numerator: f64 = points
            .iter()
            .map(|point| point.2 * (point.0 - mean_index) * (point.1 - mean_position))
            .sum();
        let denominator: f64 = points
            .iter()
            .map(|point| point.2 * (point.0 - mean_index) * (point.0 - mean_index))
            .sum();
        (denominator > 0.0).then_some(numerator / denominator)
    };
    let mut slope = fit(&points)?;
    for _ in 0..2 {
        let kept: Vec<(f64, f64, f64)> = points
            .iter()
            .copied()
            .filter(|point| (point.1 - (point.0 * slope)).abs() <= 1.5)
            .collect();
        if kept.len() < 6 {
            break;
        }
        if let Some(next) = fit(&kept) {
            slope = next;
        }
    }
    (slope > 0.5 && slope < 60.0 * 43.0 / 30.0).then_some(slope)
}

/// The tempo, in beats per minute, of `mono` at [`RATE`].
fn tempo(mono: &[f32], cancel: &AtomicBool) -> Option<f32> {
    let envelopes = flux_envelopes(mono, cancel)?;
    let env_rate = f64::from(RATE) / ENV_HOP as f64;
    let broad = &envelopes.broad;
    let low = &envelopes.low;
    let mid = &envelopes.mid;
    let high = &envelopes.high;
    if broad.len() < 32 {
        return None;
    }
    let all = normalize(
        &(0..broad.len())
            .map(|i| broad[i] + 0.8 * low[i] + 0.8 * mid[i] + 0.25 * high[i])
            .collect::<Vec<f32>>(),
    );
    let all_period = best_period_near(&all, env_rate, 0.9, cancel)?;
    let low_period = best_period(low, env_rate, cancel);
    let mid_period = best_period(mid, env_rate, cancel);
    let close = |left: f64, right: f64| (left / right - 1.0).abs() < 0.08;
    // A click on the beat with a hat between: the low band keeps the beat
    // while the mids follow the subdivision. That is an accent, not a
    // second beat, so the low band's period wins. When the mids also sit at
    // half the combined rate, as a kick and snare alternating on the beat
    // do, the two bands are filling each other's gaps and the combined
    // period is the beat itself.
    let low_is_half = low_period.is_some_and(|low| close(low, 2.0 * all_period));
    let mid_is_half = mid_period.is_some_and(|mid| close(mid, 2.0 * all_period));
    let chosen = match low_period {
        Some(low) if low_is_half && !mid_is_half => low,
        _ => all_period,
    };
    let refined = refine_grid(&all, chosen, cancel).unwrap_or(chosen);
    let bpm = 60.0 * env_rate / refined;
    Some(fold_bpm(bpm))
}

/// A raw tempo folded into the 68–182 BPM range producers name beats in.
/// The fold's edges sit a couple of beats outside that range, so a track
/// genuinely at 69 or 181 is not thrown to the other end of the octave.
fn fold_bpm(bpm: f64) -> f32 {
    let mut folded = bpm;
    while folded < 68.0 {
        folded *= 2.0;
    }
    while folded > 182.0 {
        folded /= 2.0;
    }
    folded as f32
}

/// The chroma of `mono`: the share of its spectral energy each pitch class
/// carries, from spectral peaks across evenly spaced windows, and the same
/// restricted to the bass. Tuning is measured first and taken out, so a
/// detuned 808 lands on its own semitone.
fn chroma(mono: &[f32], cancel: &AtomicBool) -> Option<([f32; 12], [f32; 12])> {
    const WINDOW: usize = 16_384;
    const WINDOWS: usize = 8;
    if mono.len() < WINDOW {
        return None;
    }
    let shift = tuning(mono, cancel);
    let mut full = [0.0_f64; 12];
    let mut bass = [0.0_f64; 12];
    for window in 0..WINDOWS {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let at = window * (mono.len() - WINDOW) / (WINDOWS - 1);
        let mut real: Vec<f32> = mono[at..at + WINDOW]
            .iter()
            .enumerate()
            .map(|(i, sample)| {
                let taper = 0.5 * (1.0 - (std::f32::consts::TAU * i as f32 / WINDOW as f32).cos());
                sample * taper
            })
            .collect();
        let mut imaginary = vec![0.0_f32; WINDOW];
        fft_pow2(&mut real, &mut imaginary);
        let mut peaks: Vec<(f64, f64)> = Vec::new();
        for bin in 2..WINDOW / 2 {
            let frequency = bin as f64 * RATE as f64 / WINDOW as f64;
            if frequency > 5000.0 {
                break;
            }
            let magnitude =
                f64::from((real[bin] * real[bin] + imaginary[bin] * imaginary[bin]).sqrt());
            let before = f64::from(
                (real[bin - 1] * real[bin - 1] + imaginary[bin - 1] * imaginary[bin - 1]).sqrt(),
            );
            let after = f64::from(
                (real[bin + 1] * real[bin + 1] + imaginary[bin + 1] * imaginary[bin + 1]).sqrt(),
            );
            if magnitude <= before || magnitude <= after {
                continue;
            }
            // Parabolic interpolation on the log magnitude gives a peak
            // frequency between bins, which the tuning estimate needs.
            let (a, b, c) = (
                before.max(1.0e-12).ln(),
                magnitude.max(1.0e-12).ln(),
                after.max(1.0e-12).ln(),
            );
            let denominator = a - 2.0 * b + c;
            let delta = if denominator.abs() > 1.0e-12 {
                0.5 * (a - c) / denominator
            } else {
                0.0
            };
            let interpolated = (bin as f64 + delta.clamp(-0.5, 0.5)) * RATE as f64 / WINDOW as f64;
            let amplitude = (b - 0.25 * (a - c) * delta).exp();
            peaks.push((interpolated, amplitude));
        }
        let total: f64 = peaks.iter().map(|(_, amplitude)| amplitude).sum();
        if total <= 0.0 {
            continue;
        }
        // One pitch class's nominal share, so a loud partial is compressed
        // against the whole window rather than against itself.
        let mean = total / 12.0;
        for (frequency, amplitude) in peaks {
            let midi = 69.0 + 12.0 * (frequency / 440.0).log2() - shift;
            let class = (midi.round() as i64).rem_euclid(12) as usize;
            let weight = (1.0 + amplitude / mean).ln();
            full[class] += weight;
            if frequency < 250.0 {
                bass[class] += weight;
            }
        }
    }
    let normalized = |values: &[f64; 12]| -> Option<[f32; 12]> {
        let total: f64 = values.iter().sum();
        (total > 0.0).then(|| std::array::from_fn(|i| (values[i] / total) as f32))
    };
    Some((normalized(&full)?, normalized(&bass)?))
}

/// How far the track is from A440, in semitones, from the bass peaks only.
/// The 808 and the bass line are what a detuned sample moves; the drums
/// would only flatten the histogram.
fn tuning(mono: &[f32], cancel: &AtomicBool) -> f64 {
    const WINDOW: usize = 16_384;
    const WINDOWS: usize = 4;
    if mono.len() < WINDOW {
        return 0.0;
    }
    let mut histogram = [0.0_f64; 100];
    for window in 0..WINDOWS {
        if cancel.load(Ordering::Relaxed) {
            return 0.0;
        }
        let at = window * (mono.len() - WINDOW) / WINDOWS.max(1);
        let mut real: Vec<f32> = mono[at..at + WINDOW]
            .iter()
            .enumerate()
            .map(|(i, sample)| {
                let taper = 0.5 * (1.0 - (std::f32::consts::TAU * i as f32 / WINDOW as f32).cos());
                sample * taper
            })
            .collect();
        let mut imaginary = vec![0.0_f32; WINDOW];
        fft_pow2(&mut real, &mut imaginary);
        for bin in 2..WINDOW / 2 {
            let frequency = bin as f64 * RATE as f64 / WINDOW as f64;
            if !(30.0..500.0).contains(&frequency) {
                continue;
            }
            let magnitude =
                f64::from((real[bin] * real[bin] + imaginary[bin] * imaginary[bin]).sqrt());
            let before = f64::from(
                (real[bin - 1] * real[bin - 1] + imaginary[bin - 1] * imaginary[bin - 1]).sqrt(),
            );
            let after = f64::from(
                (real[bin + 1] * real[bin + 1] + imaginary[bin + 1] * imaginary[bin + 1]).sqrt(),
            );
            if magnitude <= before || magnitude <= after {
                continue;
            }
            let midi = 69.0 + 12.0 * (frequency / 440.0).log2();
            let residual = midi - midi.round();
            let index = ((residual + 0.5) * 100.0).floor().clamp(0.0, 99.0) as usize;
            histogram[index] += magnitude;
        }
    }
    let total: f64 = histogram.iter().sum();
    if total <= 0.0 {
        return 0.0;
    }
    let mut best = (0_usize, 0.0_f64);
    for index in 0..100 {
        let mut sum = 0.0;
        for offset in -2_i32..=2 {
            sum += histogram[(index as i32 + offset).rem_euclid(100) as usize];
        }
        if sum > best.1 {
            best = (index, sum);
        }
    }
    // A flat histogram says the peaks agree on no tuning; leaving the
    // correction at zero is safer than following noise.
    if best.1 / total < 0.12 {
        return 0.0;
    }
    (best.0 as f64 + 0.5) / 100.0 - 0.5
}

fn chroma_key(mono: &[f32], cancel: &AtomicBool) -> Option<(Key, f32)> {
    let (profile, bass) = chroma(mono, cancel)?;
    let mut best: Option<(Key, f32, f32)> = None;
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
            let score = confidence + BASS_WEIGHT * bass[tonic];
            if best.is_none_or(|(_, _, kept)| score > kept) {
                best = Some((
                    Key {
                        tonic: tonic as u8,
                        minor,
                    },
                    confidence,
                    score,
                ));
            }
        }
    }
    best.filter(|(_, confidence, _)| *confidence >= KEY_CONFIDENT)
        .map(|(key, confidence, _)| (key, confidence))
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

/// Why Encore cannot hear a song at all, when that is settled without asking
/// the backend. A downloaded file is always worth trying, so callers check
/// for one first. Spotify is not refused here: the backend can still stream
/// a saved download's match, and says why when there is none.
pub fn unavailable(uri: &str) -> Option<&'static str> {
    if crate::util::uri_kind(uri) == Some("local") {
        return Some("That file isn't in the folder Encore scanned.");
    }
    if crate::shared_songs::is_song(uri) {
        return Some(
            "A song the group shared can be measured once it is downloaded here; \
             downloading it also counts against the group's monthly limit.",
        );
    }
    if uri.starts_with("spotify:")
        || crate::soundcloud::is_track_uri(uri)
        || crate::youtube::is_track_uri(uri)
    {
        return None;
    }
    Some("Encore can't hear this song's audio to measure BPM and key.")
}

/// A Spotify song with no downloaded copy and no remembered match: its audio
/// never reaches Encore, so there is nothing to measure.
pub const SPOTIFY_UNAVAILABLE: &str = "Spotify plays this song itself, so Encore never hears its audio. \
     Download it, or play a SoundCloud or YouTube version, to measure BPM and key.";

/// A request the listener stopped; the row has already forgotten it, so the
/// wording is only ever logged, not shown.
pub const CANCELLED: &str = "The analysis stopped before it could answer.";

/// How far an analysis has got, for the row's progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Finding the song's audio: a file, or a stream link.
    Finding,
    /// The audio is in hand and being listened to.
    Listening,
}

/// Per-song readings and requests. Loading is deduplicated; a failed reading
/// can be asked for again.
#[derive(Default)]
pub struct State {
    next_request: u64,
    entries: HashMap<String, (u64, Loadable<Reading>)>,
    stages: HashMap<String, Stage>,
}

impl State {
    pub fn get(&self, uri: &str) -> Option<&Loadable<Reading>> {
        self.entries.get(uri).map(|(_, state)| state)
    }

    /// The reading when there is one; a failed or running request has none.
    pub fn reading(&self, uri: &str) -> Option<Reading> {
        self.get(uri).and_then(Loadable::get).copied()
    }

    /// The running request's number, for a late answer to be matched
    /// against and for cancelling it.
    pub fn request(&self, uri: &str) -> Option<u64> {
        match self.entries.get(uri) {
            Some((request, Loadable::Loading)) => Some(*request),
            _ => None,
        }
    }

    /// How far the running request has got.
    pub fn stage(&self, uri: &str) -> Option<Stage> {
        self.stages.get(uri).copied()
    }

    /// Starts a request for `uri`, unless one is running or already answered.
    pub fn begin(&mut self, uri: &str) -> Option<u64> {
        if matches!(self.get(uri), Some(Loadable::Loading | Loadable::Loaded(_))) {
            return None;
        }
        self.next_request = self.next_request.wrapping_add(1);
        self.stages.remove(uri);
        self.entries
            .insert(uri.to_owned(), (self.next_request, Loadable::Loading));
        Some(self.next_request)
    }

    pub fn progress(&mut self, uri: &str, request: u64, stage: Stage) {
        if let Some((held, Loadable::Loading)) = self.entries.get(uri)
            && *held == request
        {
            self.stages.insert(uri.to_owned(), stage);
        }
    }

    pub fn finish(&mut self, uri: &str, request: u64, result: Result<Reading, String>) {
        if let Some((held, state)) = self.entries.get_mut(uri)
            && *held == request
        {
            *state = match result {
                Ok(reading) => Loadable::Loaded(reading),
                Err(error) => Loadable::Failed(error),
            };
            self.stages.remove(uri);
        }
    }

    /// Stops a running request, returning its number so the backend can drop
    /// it. The row goes back to inviting a measurement.
    pub fn cancel(&mut self, uri: &str) -> Option<u64> {
        let request = self.request(uri)?;
        self.entries.remove(uri);
        self.stages.remove(uri);
        Some(request)
    }

    /// Keeps a reading the passive analyser made while the song played, or
    /// one from the cache, so a row shows it without asking again.
    pub fn remember(&mut self, uri: &str, reading: Reading) {
        if matches!(self.get(uri), Some(Loadable::Loading | Loadable::Loaded(_))) {
            return;
        }
        self.stages.remove(uri);
        self.entries
            .insert(uri.to_owned(), (0, Loadable::Loaded(reading)));
    }

    /// Seeds the state from the cache, so an answered song stays answered
    /// across launches.
    pub fn load(&mut self, readings: impl IntoIterator<Item = (String, Reading)>) {
        for (uri, reading) in readings {
            self.remember(&uri, reading);
        }
    }
}

/// This file's shape. Older copies read a file without it as version 0,
/// and a newer one is left untouched rather than guessed at.
const CACHE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct CacheFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    readings: BTreeMap<String, Reading>,
}

/// The readings remembered between launches, in a small JSON file. A
/// missing, corrupt, older or newer file simply means fewer remembered
/// readings; every one can be measured again.
#[derive(Default)]
pub struct Cache {
    path: Option<PathBuf>,
    readings: BTreeMap<String, Reading>,
}

impl Cache {
    /// Reads the cache at `path`. This is a small file read at startup, like
    /// the local files index; the writes go through the app's
    /// `fastframe_file::Writer`.
    pub fn load(path: PathBuf) -> Self {
        let Ok(bytes) = std::fs::read(&path) else {
            return Self {
                path: Some(path),
                ..Self::default()
            };
        };
        let Ok(file) = serde_json::from_slice::<CacheFile>(&bytes) else {
            return Self {
                path: Some(path),
                ..Self::default()
            };
        };
        if file.version > CACHE_VERSION {
            return Self {
                path: Some(path),
                ..Self::default()
            };
        }
        let readings = file
            .readings
            .into_iter()
            .filter(|(uri, reading)| !uri.is_empty() && reading.bpm > 0)
            .map(|(uri, reading)| (uri, named_key(reading)))
            .collect();
        Self {
            path: Some(path),
            readings,
        }
    }

    pub fn get(&self, uri: &str) -> Option<Reading> {
        self.readings.get(uri).copied()
    }

    /// Remembers one reading. Local files are not cached: their path stays
    /// the same when the file behind it changes, and reading a file again is
    /// cheap.
    pub fn insert(&mut self, uri: &str, reading: Reading) {
        if uri.is_empty() || reading.bpm == 0 || uri.starts_with("spotify:local:") {
            return;
        }
        self.readings.insert(uri.to_owned(), named_key(reading));
    }

    pub fn readings(&self) -> impl Iterator<Item = (&String, &Reading)> {
        self.readings.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.readings.is_empty()
    }

    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// The bytes to hand to the writer, or [`None`] when the readings cannot
    /// be written at all. Nothing is written then, so a file that is already
    /// there keeps what it holds.
    pub fn encode(&self) -> Option<Vec<u8>> {
        let file = CacheFile {
            version: CACHE_VERSION,
            readings: self.readings.clone(),
        };
        serde_json::to_vec(&file).ok()
    }
}

/// `reading` with a key that cannot be named dropped: a pitch class outside
/// the twelve, or a confidence that is not a number, can only come from a
/// damaged cache file, and naming one would index past the pitch names.
fn named_key(reading: Reading) -> Reading {
    let key = reading
        .key
        .filter(|(key, confidence)| key.tonic < 12 && confidence.is_finite());
    Reading { key, ..reading }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A click track: a burst of filtered noise, decaying like a drum
    /// sample, every beat at `bpm`, and nothing between.
    fn clicks(seconds: f64, bpm: f64) -> Vec<f32> {
        clicks_at(seconds, bpm, RATE)
    }

    /// As [`clicks`], at a file's own sample rate.
    fn clicks_at(seconds: f64, bpm: f64, rate: u32) -> Vec<f32> {
        let len = (seconds * f64::from(rate)).round() as usize;
        let mut out = vec![0.0_f32; len];
        let period = f64::from(rate) * 60.0 / bpm;
        let onset = 512_usize;
        let mut next = 0.0_f64;
        while next < len as f64 {
            let at = next.round() as usize;
            for i in 0..onset {
                if at + i < len {
                    let decay = (-(i as f32) / onset as f32 * 7.0).exp();
                    let noise = (((i * 2654435761_usize) % 1000) as f32 / 1000.0) - 0.5;
                    out[at + i] += 2.0 * noise * decay;
                }
            }
            next += period;
        }
        out
    }

    /// Clicks with a bright tick at every 1/`division` of the beat: the
    /// hi-hat subdivision that used to double a boom-bap tempo.
    fn clicks_with_hats(seconds: f64, bpm: f64, division: u32) -> Vec<f32> {
        let mut out = clicks(seconds, bpm);
        let len = out.len();
        let period = f64::from(RATE) * 60.0 / (bpm * f64::from(division));
        let mut next = 0.0_f64;
        while next < len as f64 {
            let at = next.round() as usize;
            for i in 0..128 {
                if at + i < len {
                    let decay = (-(i as f32) / 128.0 * 10.0).exp();
                    out[at + i] += 0.7 * decay * ((i as f32 * 0.9).sin());
                }
            }
            next += period;
        }
        out
    }

    /// A kick and snare alternating on the beat, with hats on `division`
    /// subdivisions: one drum pattern at `bpm`.
    fn drum_pattern(seconds: f64, bpm: f64, division: u32) -> Vec<f32> {
        let len = (seconds * f64::from(RATE)).round() as usize;
        let mut out = vec![0.0_f32; len];
        let beat = f64::from(RATE) * 60.0 / bpm;
        let mut at = 0.0_f64;
        let mut index = 0_usize;
        while at < len as f64 {
            let start = at.round() as usize;
            let is_snare = index % 2 == 1;
            let onset = if is_snare { 700 } else { 900 };
            for i in 0..onset {
                if start + i < len {
                    let decay = (-(i as f32) / onset as f32 * 6.0).exp();
                    if is_snare {
                        let noise =
                            (((i * 2654435761_usize + index * 97) % 1000) as f32 / 1000.0) - 0.5;
                        out[start + i] += 0.45 * noise * decay;
                    } else {
                        let t = i as f64 / f64::from(RATE);
                        out[start + i] +=
                            0.7 * (std::f64::consts::TAU * 55.0 * t).sin() as f32 * decay;
                    }
                }
            }
            for hat in 0..division {
                let hat_at = (at + beat * f64::from(hat) / f64::from(division)).round() as usize;
                for i in 0..100 {
                    if hat_at + i < len {
                        let decay = (-(i as f32) / 100.0 * 14.0).exp();
                        out[hat_at + i] += 0.18 * ((i as f32 * 2.1).sin()) * decay;
                    }
                }
            }
            at += beat;
            index += 1;
        }
        out
    }

    /// A sustained bass root plus its triad two octaves up, with drums.
    fn key_mix(root: u8, minor: bool, detune_cents: f64, seconds: f64) -> Vec<f32> {
        let len = (seconds * f64::from(RATE)).round() as usize;
        let mut out = vec![0.0_f32; len];
        let mul = 2.0_f64.powf(detune_cents / 1200.0);
        let bass = 440.0 * 2.0_f64.powf((f64::from(root) - 69.0) / 12.0) * mul;
        let third = if minor { 3 } else { 4 };
        let notes = [root + 24, root + 24 + third, root + 24 + 7, root + 36];
        for (i, sample) in out.iter_mut().enumerate() {
            let t = i as f64 / f64::from(RATE);
            let phase = (t * 2.0) % 1.0;
            let envelope = (-phase * 2.5).exp();
            *sample += envelope as f32
                * (0.55 * (std::f64::consts::TAU * bass * t).sin()
                    + 0.08 * (std::f64::consts::TAU * 2.0 * bass * t).sin())
                    as f32;
            for &midi in &notes {
                let frequency = 440.0 * 2.0_f64.powf((f64::from(midi) - 69.0) / 12.0) * mul;
                *sample += 0.10 * (std::f64::consts::TAU * frequency * t).sin() as f32
                    + 0.03 * (std::f64::consts::TAU * 2.0 * frequency * t).sin() as f32;
            }
        }
        for (sample, drum) in out.iter_mut().zip(clicks(seconds, 90.0)) {
            *sample += 0.25 * drum;
        }
        out
    }

    #[test]
    fn click_tracks_read_their_own_tempo() {
        for bpm in [70.0, 90.0, 120.0, 140.0, 174.0] {
            let reading = analyze(&clicks(16.0, bpm), RATE).expect("a click track has an answer");
            assert!(
                (f64::from(reading.bpm) - bpm).abs() <= 1.0,
                "clicked at {bpm} bpm, read {reading:?}"
            );
        }
    }

    #[test]
    fn a_hi_hat_subdivision_does_not_double_the_beat() {
        // 16th hats over a boom-bap pocket used to read as an exact
        // doubling; a fast 140 pattern with them used to read as 70.
        for (bpm, division) in [(70.0, 4), (87.0, 4), (140.0, 2), (140.0, 4)] {
            let reading = analyze(&clicks_with_hats(16.0, bpm, division), RATE)
                .expect("a click track has an answer");
            assert!(
                (f64::from(reading.bpm) - bpm).abs() <= 1.0,
                "clicked at {bpm} with 1/{division} hats, read {reading:?}"
            );
        }
    }

    #[test]
    fn a_drum_pattern_reads_its_beat() {
        for bpm in [70.0, 87.0, 140.0] {
            let reading =
                analyze(&drum_pattern(16.0, bpm, 2), RATE).expect("a drum pattern has an answer");
            assert!(
                (f64::from(reading.bpm) - bpm).abs() <= 1.0,
                "drums at {bpm}, read {reading:?}"
            );
        }
    }

    #[test]
    fn a_boom_bap_pocket_with_sixteenth_hats_is_not_doubled() {
        // The bench's worst case: a 85–90 BPM pocket under 16th hats used to
        // come back as an exact doubling.
        for bpm in [85.0, 87.0, 90.0] {
            let reading = analyze(&drum_pattern(16.0, bpm, 4), RATE)
                .expect("a drum pattern with hats has an answer");
            assert!(
                (f64::from(reading.bpm) - bpm).abs() <= 1.0,
                "drums at {bpm} with 1/16 hats, read {reading:?}"
            );
        }
    }

    #[test]
    fn audio_at_a_files_own_rate_is_resampled_before_reading() {
        // 8 kHz goes up and 48 kHz comes down; both must read the tempo the
        // file was made at, not a rate-skewed one.
        for rate in [8_000, 22_050, 44_100, 48_000] {
            let reading = analyze(&clicks_at(16.0, 120.0, rate), rate)
                .expect("a resampled click track has an answer");
            assert!(
                (f64::from(reading.bpm) - 120.0).abs() <= 1.0,
                "at {rate} Hz, read {reading:?}"
            );
        }
    }

    #[test]
    fn a_rate_of_zero_or_a_buffer_too_small_to_hold_sound_answers_nothing() {
        let clicks = clicks(4.0, 120.0);
        assert!(analyze(&clicks, 0).is_none());
        assert!(analyze(&[0.5], 44_100).is_none());
        assert!(analyze(&[0.5], 1).is_none());
        assert!(analyze(&[0.5; 100], 44_100).is_none());
        assert!(analyze(&vec![0.0; 100], 1).is_none());
    }

    #[test]
    fn every_major_and_minor_key_reads_back() {
        for root in 0..12_u8 {
            for minor in [false, true] {
                let samples = key_mix(24 + root, minor, 0.0, 6.0);
                let reading = analyze(&samples, RATE).expect("a chord has an answer");
                let (key, confidence) = reading.key.expect("a chord names a key");
                let expected = Key { tonic: root, minor };
                assert_eq!(key, expected, "confidence {confidence}");
                assert!(confidence >= KEY_CONFIDENT, "confidence {confidence}");
            }
        }
    }

    #[test]
    fn a_detuned_808_still_reads_its_key() {
        for (root, minor) in [(4, true), (8, false), (11, true)] {
            for detune in [-40.0, 40.0] {
                let samples = key_mix(24 + root, minor, detune, 6.0);
                let reading = analyze(&samples, RATE).expect("a chord has an answer");
                let (key, _) = reading.key.expect("a detuned chord names a key");
                assert_eq!(key, Key { tonic: root, minor }, "detune {detune}");
            }
        }
    }

    #[test]
    fn the_bass_root_breaks_a_relative_key_tie() {
        // An Am–F loop with the bass on A and F: the melody alone could be C
        // major, but a producer hears A minor.
        let len = (10.0 * f64::from(RATE)) as usize;
        let mut looped = vec![0.0_f32; len];
        for (i, sample) in looped.iter_mut().enumerate() {
            let t = i as f64 / f64::from(RATE);
            let a_minor = ((t / 2.0) as usize).is_multiple_of(2);
            let chord = if a_minor {
                [57_u8, 60, 64]
            } else {
                [53, 57, 60]
            };
            let root = if a_minor { 33.0 } else { 29.0 };
            *sample += 0.35 * (std::f64::consts::TAU * root * t).sin() as f32;
            for &midi in &chord {
                let frequency = 440.0 * 2.0_f64.powf((f64::from(midi) - 69.0) / 12.0);
                *sample += 0.12 * (std::f64::consts::TAU * frequency * t).sin() as f32;
            }
        }
        let reading = analyze(&looped, RATE).expect("a loop has an answer");
        assert_eq!(
            reading.key.map(|(key, _)| key),
            Some(Key {
                tonic: 9,
                minor: true
            })
        );
    }

    #[test]
    fn silence_has_no_answer() {
        assert!(analyze(&vec![0.0_f32; RATE as usize * 8], RATE).is_none());
        assert!(analyze(&[], RATE).is_none());
    }

    #[test]
    fn a_cancelled_analysis_answers_nothing() {
        let cancel = AtomicBool::new(true);
        assert!(analyze_cancellable(&clicks(8.0, 120.0), RATE, &cancel).is_none());
    }

    fn reading(bpm: u16) -> Reading {
        Reading {
            bpm,
            key: Some((
                Key {
                    tonic: 9,
                    minor: true,
                },
                0.8,
            )),
        }
    }

    #[test]
    fn one_request_runs_at_a_time_and_a_late_answer_is_dropped() {
        let mut state = State::default();
        let first = state.begin("soundcloud:track:1").unwrap();
        assert!(state.begin("soundcloud:track:1").is_none());
        state.progress("soundcloud:track:1", first, Stage::Listening);
        assert_eq!(state.stage("soundcloud:track:1"), Some(Stage::Listening));
        // A stale answer for an older request changes nothing.
        state.finish("soundcloud:track:1", first + 1, Ok(reading(120)));
        assert!(matches!(
            state.get("soundcloud:track:1"),
            Some(Loadable::Loading)
        ));
        state.finish("soundcloud:track:1", first, Ok(reading(140)));
        assert_eq!(
            state
                .reading("soundcloud:track:1")
                .map(|reading| reading.bpm),
            Some(140)
        );
        assert_eq!(state.stage("soundcloud:track:1"), None);
        // Answered songs are not asked for again.
        assert!(state.begin("soundcloud:track:1").is_none());
    }

    #[test]
    fn a_failed_reading_can_be_asked_for_again() {
        let mut state = State::default();
        let first = state.begin("spotify:track:1").unwrap();
        state.finish("spotify:track:1", first, Err("no audio".into()));
        assert!(matches!(
            state.get("spotify:track:1"),
            Some(Loadable::Failed(message)) if message == "no audio"
        ));
        let second = state.begin("spotify:track:1").unwrap();
        assert_ne!(first, second);
        assert!(matches!(
            state.get("spotify:track:1"),
            Some(Loadable::Loading)
        ));
    }

    #[test]
    fn a_running_request_can_be_cancelled() {
        let mut state = State::default();
        let request = state.begin("soundcloud:track:cancel").unwrap();
        assert_eq!(state.cancel("soundcloud:track:cancel"), Some(request));
        assert!(state.get("soundcloud:track:cancel").is_none());
        assert_eq!(state.stage("soundcloud:track:cancel"), None);
        assert_eq!(state.cancel("soundcloud:track:cancel"), None);
        // A late answer for the cancelled request changes nothing.
        state.finish("soundcloud:track:cancel", request, Ok(reading(120)));
        assert!(state.get("soundcloud:track:cancel").is_none());
    }

    #[test]
    fn a_reading_heard_while_playing_is_remembered() {
        let mut state = State::default();
        state.remember("soundcloud:track:2", reading(87));
        assert_eq!(state.reading("soundcloud:track:2").map(|r| r.bpm), Some(87));
        assert!(state.begin("soundcloud:track:2").is_none());
        // A request in flight is not overwritten by the passive one.
        let request = state.begin("soundcloud:track:3").unwrap();
        state.remember("soundcloud:track:3", reading(70));
        assert!(matches!(
            state.get("soundcloud:track:3"),
            Some(Loadable::Loading)
        ));
        state.finish("soundcloud:track:3", request, Ok(reading(140)));
        assert_eq!(
            state.reading("soundcloud:track:3").map(|r| r.bpm),
            Some(140)
        );
    }

    #[test]
    fn spotify_without_a_file_can_still_be_tried_for_a_saved_match() {
        assert_eq!(unavailable("spotify:track:4"), None);
        assert_eq!(unavailable("soundcloud:track:4"), None);
        assert_eq!(unavailable("youtube:video:dQw4w9WgXcQ"), None);
        assert!(
            unavailable("spotify:local:A:B:C:1")
                .unwrap()
                .contains("folder")
        );
        assert!(
            unavailable("encore:song:123e4567-e89b-12d3-a456-426614174000")
                .unwrap()
                .contains("downloaded here")
        );
        assert!(SPOTIFY_UNAVAILABLE.contains("never hears its audio"));
    }

    fn cache_root(name: &str) -> PathBuf {
        crate::test_dirs::temp_dir().join(format!(
            "siphon-bpm-key-cache-{name}-{}-{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ))
    }

    #[test]
    fn the_cache_round_trips_and_ignores_older_and_newer_files() {
        let root = cache_root("round-trip");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("bpm-key.json");
        let mut cache = Cache::load(path.clone());
        assert!(cache.is_empty());
        cache.insert(
            "soundcloud:track:1",
            Reading {
                bpm: 140,
                key: Some((
                    Key {
                        tonic: 1,
                        minor: true,
                    },
                    0.8,
                )),
            },
        );
        // Local files are not cached: their path outlives the file behind it.
        cache.insert("spotify:local:A:B:C:1", reading(120));
        std::fs::write(&path, cache.encode().expect("the cache encodes")).unwrap();
        let reopened = Cache::load(path.clone());
        assert_eq!(
            reopened
                .get("soundcloud:track:1")
                .map(|reading| reading.bpm),
            Some(140)
        );
        assert_eq!(reopened.get("spotify:local:A:B:C:1"), None);

        // The first files had no version at all.
        let older = root.join("older.json");
        std::fs::write(
            &older,
            br#"{"readings":{"soundcloud:track:2":{"bpm":87,"key":null}}}"#,
        )
        .unwrap();
        assert_eq!(
            Cache::load(older).get("soundcloud:track:2").map(|r| r.bpm),
            Some(87)
        );

        // A damaged file cannot name its key: the tempo is still worth
        // keeping, so the reading stays and only the key is dropped. Naming
        // pitch class 200 would read past the names.
        let damaged = root.join("damaged-key.json");
        std::fs::write(
            &damaged,
            br#"{"version":1,"readings":{"soundcloud:track:4":{"bpm":128,"key":[{"tonic":200,"minor":true},0.7]}}}"#,
        )
        .unwrap();
        let loaded = Cache::load(damaged);
        assert_eq!(loaded.get("soundcloud:track:4").map(|r| r.bpm), Some(128));
        assert_eq!(loaded.get("soundcloud:track:4").and_then(|r| r.key), None);

        // The same on the way in, so a reading the analysis could not name is
        // never written where naming it would panic.
        let mut unnamed = Cache::default();
        unnamed.insert(
            "soundcloud:track:5",
            Reading {
                bpm: 90,
                key: Some((
                    Key {
                        tonic: 12,
                        minor: false,
                    },
                    0.0,
                )),
            },
        );
        let written: CacheFile =
            serde_json::from_slice(&unnamed.encode().expect("the cache encodes")).unwrap();
        assert_eq!(written.readings["soundcloud:track:5"].key, None);
        assert_eq!(written.readings["soundcloud:track:5"].bpm, 90);

        // Corrupt and newer files read as empty and are left untouched.
        for (name, body) in [
            (
                "corrupt.json",
                &br#"{"readings":{"soundcloud:track:3":"#[..],
            ),
            (
                "newer.json",
                &br#"{"version":99,"readings":{"soundcloud:track:3":{"bpm":120}}}"#[..],
            ),
        ] {
            let path = root.join(name);
            std::fs::write(&path, body).unwrap();
            assert!(Cache::load(path.clone()).is_empty());
            assert_eq!(std::fs::read(&path).unwrap(), body);
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
