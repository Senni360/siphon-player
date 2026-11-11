//! Reading PCM out of a local audio file for the BPM and key analyser.
//!
//! Downloaded songs and songs in the local files folder are ordinary files,
//! so they can be measured whether or not they ever play. This runs on a
//! worker thread; it never touches the interface or the audio device.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as DecodeError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Why a file gave no audio, in words the row can show.
const UNREADABLE: &str = "That file couldn't be read as audio.";

/// `path` mixed down to mono, at its own sample rate, for at most
/// `max_seconds`. Returns the samples and the rate they are at. A set
/// `cancel` stops the reading between packets.
pub fn mono(path: &Path, max_seconds: f64, cancel: &AtomicBool) -> Result<(Vec<f32>, u32), String> {
    let file =
        std::fs::File::open(path).map_err(|_| "That file couldn't be opened.".to_string())?;
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut probed = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .map_err(|_| UNREADABLE.to_string())?;
    let track = probed
        .format
        .default_track()
        .ok_or_else(|| UNREADABLE.to_string())?;
    let rate = track
        .codec_params
        .sample_rate
        .filter(|rate| *rate > 0 && *rate <= 384_000)
        .ok_or_else(|| UNREADABLE.to_string())?;
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| UNREADABLE.to_string())?;
    let wanted = (max_seconds.max(0.0) * f64::from(rate)) as usize;
    let mut mono = Vec::with_capacity(wanted);
    let mut packets = 0_usize;
    while mono.len() < wanted {
        if packets.is_multiple_of(32) && cancel.load(Ordering::Relaxed) {
            return Err(crate::bpm_key::CANCELLED.to_string());
        }
        packets += 1;
        let packet = match probed.format.next_packet() {
            Ok(packet) => packet,
            // The end of the file is the end of the reading; a broken tail
            // still leaves everything decoded before it usable.
            Err(DecodeError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(_) => break,
        };
        if packet.track_id() != id {
            continue;
        }
        let Ok(decoded) = decoder.decode(&packet) else {
            break;
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count();
        if spec.rate != rate || channels == 0 {
            continue;
        }
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        for frame in buffer.samples().chunks_exact(channels) {
            let sum: f32 = frame.iter().sum();
            mono.push(sum / channels as f32);
            if mono.len() == wanted {
                break;
            }
        }
    }
    if mono.is_empty() {
        return Err(UNREADABLE.to_string());
    }
    Ok((mono, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny 16-bit mono WAV, built by hand so the test needs no fixture.
    fn wav(rate: u32, seconds: f64, frequency: f64) -> Vec<u8> {
        let frames = (rate as f64 * seconds) as u32;
        let mut data = Vec::with_capacity(frames as usize * 2);
        for frame in 0..frames {
            let t = f64::from(frame) / f64::from(rate);
            let sample = (std::f64::consts::TAU * frequency * t).sin() * 0.5;
            data.extend_from_slice(&((sample * f64::from(i16::MAX)) as i16).to_le_bytes());
        }
        let mut bytes = Vec::with_capacity(44 + data.len());
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes
    }

    /// A tiny 16-bit stereo WAV of a tone, with the right channel either the
    /// same tone or its inverse. Averaging the two channels reads silence in
    /// the inverted one and the tone in the other; taking one channel alone
    /// cannot do both.
    fn stereo_wav(rate: u32, seconds: f64, invert: bool) -> Vec<u8> {
        let frames = (rate as f64 * seconds) as u32;
        let mut data = Vec::with_capacity(frames as usize * 4);
        for frame in 0..frames {
            let t = f64::from(frame) / f64::from(rate);
            let sample = (std::f64::consts::TAU * 220.0 * t).sin() * 0.5;
            let left = (sample * f64::from(i16::MAX)) as i16;
            let right = (if invert { -sample } else { sample } * f64::from(i16::MAX)) as i16;
            data.extend_from_slice(&left.to_le_bytes());
            data.extend_from_slice(&right.to_le_bytes());
        }
        let mut bytes = Vec::with_capacity(44 + data.len());
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 4).to_le_bytes());
        bytes.extend_from_slice(&4u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes
    }

    #[test]
    fn a_wav_file_is_read_as_mono_at_its_own_rate() {
        let dir = crate::test_dirs::temp_dir().join(format!("audio-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tone.wav");
        std::fs::write(&path, wav(8_000, 1.5, 220.0)).unwrap();
        let never = AtomicBool::new(false);
        let (samples, rate) = mono(&path, 30.0, &never).expect("a plain WAV reads");
        assert_eq!(rate, 8_000);
        assert_eq!(samples.len(), 12_000);
        assert!(samples.iter().any(|sample| sample.abs() > 0.1));
        // The limit stops the reading early.
        let (short, _) = mono(&path, 0.5, &never).expect("a WAV reads");
        assert_eq!(short.len(), 4_000);
        // A cancel stops it before the end.
        let cancelled = AtomicBool::new(true);
        assert!(mono(&path, 30.0, &cancelled).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stereo_file_is_averaged_into_mono() {
        let dir =
            crate::test_dirs::temp_dir().join(format!("audio-probe-stereo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let never = AtomicBool::new(false);
        // A tone in both channels: the mix keeps it, at one sample per frame.
        let same = dir.join("same.wav");
        std::fs::write(&same, stereo_wav(11_025, 1.0, false)).unwrap();
        let (samples, rate) = mono(&same, 30.0, &never).expect("a stereo WAV reads");
        assert_eq!(rate, 11_025);
        assert_eq!(samples.len(), 11_025);
        assert!(samples.iter().any(|sample| sample.abs() > 0.1));
        // The same tone inverted on the right: averaging cancels it, while
        // taking either channel would not.
        let inverted = dir.join("inverted.wav");
        std::fs::write(&inverted, stereo_wav(11_025, 1.0, true)).unwrap();
        let (silent, _) = mono(&inverted, 30.0, &never).expect("a stereo WAV reads");
        assert_eq!(silent.len(), 11_025);
        assert!(
            silent.iter().all(|sample| sample.abs() < 1.0e-3),
            "the channels did not cancel"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn something_that_is_not_audio_says_so() {
        let dir =
            crate::test_dirs::temp_dir().join(format!("audio-probe-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notes.txt");
        std::fs::write(&path, b"not a song").unwrap();
        assert!(mono(&path, 30.0, &AtomicBool::new(false)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
