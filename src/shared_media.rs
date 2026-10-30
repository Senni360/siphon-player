//! Bounded preparation of one explicitly selected local recording.
//!
//! Call on a blocking worker, never the UI thread. This module does not scan
//! folders, upload, persist audio, read accounts or start playback. Its immutable
//! byte snapshot is the exact data described by the returned digest.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata},
    io::{Cursor, Read},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};
use symphonia::core::{
    codecs::{CODEC_TYPE_MP3, DecoderOptions},
    formats::FormatOptions,
    io::MediaSourceStream,
    meta::{Limit, MetadataOptions},
    probe::Hint,
};

/// Technical-spike limit, matching the isolated upload ledger.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;
/// The first validated native preparation profile, not a Cast support claim.
pub const PROFILE: &str = "mp3-44100-stereo";
const SAMPLE_RATE: u64 = 44_100;
const CHUNK_BYTES: usize = 64 * 1024;
const WORK_BUDGET: Duration = Duration::from_secs(30);

/// Cooperative cancellation, checked between bounded reads and decoder packets.
/// It cannot interrupt an operating-system read that has not returned.
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Reading,
    Validating,
}

/// Counts only. A progress consumer must return promptly and must not retain an
/// unbounded history of updates. No title, path, tag or provider data is emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    pub bytes_read: u64,
    pub total_bytes: u64,
    pub decoded_frames: u64,
}

/// Private preparation metadata. A digest is an identity check, never access
/// authorization. Duration comes from decoded frames, not file tags.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingMetadata {
    pub size: u64,
    pub sha256: String,
    pub profile: String,
    pub duration_ms: u64,
}

/// Holds at most 16 MiB of original encoded audio. No file path is retained.
/// There is deliberately no Debug implementation that could print the bytes.
pub struct PreparedRecording {
    metadata: RecordingMetadata,
    bytes: Arc<[u8]>,
}

impl PreparedRecording {
    pub fn metadata(&self) -> &RecordingMetadata {
        &self.metadata
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// Re-preparation rehashes the selected file. Stored duration is not trusted;
    /// the new snapshot's decoded duration remains authoritative.
    pub fn require_identity(&self, expected: &RecordingMetadata) -> Result<(), PreparationError> {
        if self.metadata.size != expected.size
            || self.metadata.sha256 != expected.sha256
            || self.metadata.profile != expected.profile
        {
            return Err(PreparationError::Changed);
        }
        Ok(())
    }
}

/// Safe errors contain no local path, tag value, media bytes or library context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreparationError {
    Cancelled,
    TimedOut,
    ReadFailed,
    NotRegularFile,
    Empty,
    TooLarge,
    Changed,
    UnsupportedProfile,
    DecodeFailed,
    DurationOutsidePilot,
    AllocationFailed,
}

impl std::fmt::Display for PreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "Recording preparation cancelled",
            Self::TimedOut => "Recording preparation exceeded its 30-second work budget",
            Self::ReadFailed => "Cannot read the selected recording",
            Self::NotRegularFile => "Select one regular recording file",
            Self::Empty => "The selected recording is empty",
            Self::TooLarge => "The selected recording exceeds the 16 MiB pilot limit",
            Self::Changed => "The selected recording changed; select and prepare it again",
            Self::UnsupportedProfile => "The pilot requires 44.1 kHz stereo MP3",
            Self::DecodeFailed => "The selected recording could not be decoded",
            Self::DurationOutsidePilot => {
                "The pilot requires a recording between 10 seconds and one hour"
            }
            Self::AllocationFailed => "Not enough memory to prepare the selected recording",
        })
    }
}
impl std::error::Error for PreparationError {}

struct Work<'a> {
    cancellation: &'a Cancellation,
    deadline: Instant,
}
impl Work<'_> {
    fn check(&self) -> Result<(), PreparationError> {
        if self.cancellation.is_cancelled() {
            Err(PreparationError::Cancelled)
        } else if Instant::now() >= self.deadline {
            Err(PreparationError::TimedOut)
        } else {
            Ok(())
        }
    }
}

#[derive(PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: SystemTime,
}
impl Stamp {
    fn from_metadata(metadata: Metadata) -> Result<Self, PreparationError> {
        if !metadata.is_file() {
            return Err(PreparationError::NotRegularFile);
        }
        Ok(Self {
            size: metadata.len(),
            modified: metadata
                .modified()
                .map_err(|_| PreparationError::ReadFailed)?,
        })
    }
}

/// Reads exactly one selected file into a bounded, immutable snapshot and
/// decodes its MP3 packets without an output device. No upload consent is
/// inferred from preparation: a later party-add action must authorize upload.
/// A caller must discard results for a cancelled or superseded selection.
pub fn prepare_selected(
    path: &Path,
    cancellation: &Cancellation,
    mut progress: impl FnMut(Progress),
) -> Result<PreparedRecording, PreparationError> {
    let work = Work {
        cancellation,
        deadline: Instant::now() + WORK_BUDGET,
    };
    work.check()?;
    // Reject normal directory/device selections before opening them. Reads on
    // unavailable filesystems may still block in the OS; cancellation is soft.
    let selected =
        Stamp::from_metadata(std::fs::metadata(path).map_err(|_| PreparationError::ReadFailed)?)?;
    check_size(selected.size)?;
    let mut file = File::open(path).map_err(|_| PreparationError::ReadFailed)?;
    let opened = Stamp::from_metadata(file.metadata().map_err(|_| PreparationError::ReadFailed)?)?;
    if selected != opened {
        return Err(PreparationError::Changed);
    }
    let (bytes, sha256) = read_snapshot(&mut file, opened.size, &work, &mut progress)?;
    check_unchanged(path, &file, &opened)?;
    let bytes: Arc<[u8]> = bytes.into();
    let frames = validate_audio(Arc::clone(&bytes), &work, &mut progress)?;
    check_unchanged(path, &file, &opened)?;
    work.check()?;
    Ok(PreparedRecording {
        metadata: RecordingMetadata {
            size: bytes.len() as u64,
            sha256,
            profile: PROFILE.to_owned(),
            duration_ms: frames * 1000 / SAMPLE_RATE,
        },
        bytes,
    })
}

fn check_unchanged(path: &Path, file: &File, opened: &Stamp) -> Result<(), PreparationError> {
    let final_handle =
        Stamp::from_metadata(file.metadata().map_err(|_| PreparationError::ReadFailed)?)?;
    let final_path =
        Stamp::from_metadata(std::fs::metadata(path).map_err(|_| PreparationError::ReadFailed)?)?;
    if *opened != final_handle || *opened != final_path {
        return Err(PreparationError::Changed);
    }
    Ok(())
}

fn check_size(size: u64) -> Result<(), PreparationError> {
    if size == 0 {
        Err(PreparationError::Empty)
    } else if size > MAX_BYTES {
        Err(PreparationError::TooLarge)
    } else {
        Ok(())
    }
}

fn read_snapshot(
    reader: &mut impl Read,
    size: u64,
    work: &Work<'_>,
    progress: &mut impl FnMut(Progress),
) -> Result<(Vec<u8>, String), PreparationError> {
    check_size(size)?;
    work.check()?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size as usize)
        .map_err(|_| PreparationError::AllocationFailed)?;
    let mut digest = Sha256::new();
    let mut chunk = [0; CHUNK_BYTES];
    progress(Progress {
        stage: Stage::Reading,
        bytes_read: 0,
        total_bytes: size,
        decoded_frames: 0,
    });
    loop {
        work.check()?;
        let maximum = chunk.len().min((size + 1 - bytes.len() as u64) as usize);
        let count = reader
            .read(&mut chunk[..maximum])
            .map_err(|_| PreparationError::ReadFailed)?;
        work.check()?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > size {
            return Err(PreparationError::Changed);
        }
        digest.update(&chunk[..count]);
        bytes.extend_from_slice(&chunk[..count]);
        progress(Progress {
            stage: Stage::Reading,
            bytes_read: bytes.len() as u64,
            total_bytes: size,
            decoded_frames: 0,
        });
    }
    if bytes.len() as u64 != size {
        return Err(PreparationError::Changed);
    }
    Ok((bytes, format!("{:x}", digest.finalize())))
}

fn validate_audio(
    bytes: Arc<[u8]>,
    work: &Work<'_>,
    progress: &mut impl FnMut(Progress),
) -> Result<u64, PreparationError> {
    let size = bytes.len() as u64;
    progress(Progress {
        stage: Stage::Validating,
        bytes_read: size,
        total_bytes: size,
        decoded_frames: 0,
    });
    work.check()?;
    check_mp3_prefix(&bytes)?;
    let source = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut probed = symphonia::default::get_probe()
        .format(
            Hint::new().with_extension("mp3"),
            source,
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions {
                limit_metadata_bytes: Limit::Maximum(0),
                limit_visual_bytes: Limit::Maximum(0),
            },
        )
        .map_err(|_| PreparationError::DecodeFailed)?;
    let track = probed
        .format
        .default_track()
        .ok_or(PreparationError::UnsupportedProfile)?;
    if probed.format.tracks().len() != 1
        || track.codec_params.codec != CODEC_TYPE_MP3
        || track.codec_params.sample_rate != Some(SAMPLE_RATE as u32)
        || track
            .codec_params
            .channels
            .is_none_or(|channels| channels.count() != 2)
    {
        return Err(PreparationError::UnsupportedProfile);
    }
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| PreparationError::DecodeFailed)?;
    let mut frames = 0;
    loop {
        work.check()?;
        let packet = match probed.format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(_) => return Err(PreparationError::DecodeFailed),
        };
        if packet.track_id() != id {
            return Err(PreparationError::UnsupportedProfile);
        }
        let audio = decoder
            .decode(&packet)
            .map_err(|_| PreparationError::DecodeFailed)?;
        if audio.spec().rate != SAMPLE_RATE as u32 || audio.spec().channels.count() != 2 {
            return Err(PreparationError::UnsupportedProfile);
        }
        frames += audio.frames() as u64;
        if frames > SAMPLE_RATE * 3600 {
            return Err(PreparationError::DurationOutsidePilot);
        }
        progress(Progress {
            stage: Stage::Validating,
            bytes_read: size,
            total_bytes: size,
            decoded_frames: frames,
        });
    }
    work.check()?;
    if frames < SAMPLE_RATE * 10 {
        return Err(PreparationError::DurationOutsidePilot);
    }
    Ok(frames)
}

// A codec match alone also accepts MP3 inside MP4. Restrict this pilot to an
// MPEG-1 Layer III frame stream, optionally preceded by one ID3v2 tag. Packet
// decoding remains Symphonia's job; this is not a replacement MP3 demuxer.
fn check_mp3_prefix(bytes: &[u8]) -> Result<(), PreparationError> {
    let mut offset = 0;
    if bytes.starts_with(b"ID3") {
        let tag = bytes.get(..10).ok_or(PreparationError::DecodeFailed)?;
        if !(2..=4).contains(&tag[3]) || tag[4] == 0xff || (tag[3] == 4 && tag[5] & 0x10 != 0) {
            return Err(PreparationError::UnsupportedProfile);
        }
        if tag[6..10].iter().any(|byte| byte & 0x80 != 0) {
            return Err(PreparationError::DecodeFailed);
        }
        let size = tag[6..10]
            .iter()
            .fold(0usize, |size, byte| (size << 7) | usize::from(*byte));
        offset = 10 + size;
    }
    let header = bytes
        .get(offset..offset + 4)
        .ok_or(PreparationError::DecodeFailed)?;
    if header[0] != 0xff || header[1] & 0xe0 != 0xe0 {
        return Err(PreparationError::DecodeFailed);
    }
    if header[1] & 0x1e != 0x1a {
        return Err(PreparationError::UnsupportedProfile);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn work(cancellation: &Cancellation) -> Work<'_> {
        Work {
            cancellation,
            deadline: Instant::now() + WORK_BUDGET,
        }
    }

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join(".cache");
            std::fs::create_dir_all(&parent).unwrap();
            let root = parent.join(format!(
                "shared-media-test-{}-{:016x}",
                std::process::id(),
                rand::random::<u64>()
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn file(&self, bytes: &[u8]) -> std::path::PathBuf {
            let path = self.0.join("private-selected-recording.mp3");
            std::fs::write(&path, bytes).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn cancellation_prevents_opening_a_file_or_reading_any_bytes() {
        struct Unreadable;
        impl Read for Unreadable {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                panic!("read after cancellation");
            }
        }
        let cancellation = Cancellation::default();
        cancellation.cancel();
        assert!(matches!(
            prepare_selected(
                Path::new("PRIVATE/no-such-recording.mp3"),
                &cancellation,
                |_| {}
            ),
            Err(PreparationError::Cancelled)
        ));
        assert_eq!(
            read_snapshot(&mut Unreadable, 3, &work(&cancellation), &mut |_| {}).unwrap_err(),
            PreparationError::Cancelled
        );
    }

    #[test]
    fn size_limits_reject_before_reading_and_length_changes_never_return_an_identity() {
        let cancellation = Cancellation::default();
        let mut reader = Cursor::new(b"abc");
        for (size, expected) in [
            (0, PreparationError::Empty),
            (MAX_BYTES + 1, PreparationError::TooLarge),
        ] {
            assert_eq!(
                read_snapshot(&mut reader, size, &work(&cancellation), &mut |_| {}).unwrap_err(),
                expected
            );
            assert_eq!(reader.position(), 0);
        }
        for size in [2, 4] {
            reader.set_position(0);
            assert_eq!(
                read_snapshot(&mut reader, size, &work(&cancellation), &mut |_| {}).unwrap_err(),
                PreparationError::Changed
            );
        }
    }

    #[test]
    fn snapshot_hash_covers_the_original_bytes_and_progress_is_monotonic() {
        let cancellation = Cancellation::default();
        let mut updates = Vec::new();
        let (bytes, digest) = read_snapshot(
            &mut Cursor::new(b"abc"),
            3,
            &work(&cancellation),
            &mut |progress| updates.push(progress),
        )
        .unwrap();
        assert_eq!(bytes, b"abc");
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(updates.first().unwrap().bytes_read, 0);
        assert_eq!(updates.last().unwrap().bytes_read, 3);
        assert!(updates.iter().all(|progress| progress.total_bytes == 3
            && progress.stage == Stage::Reading
            && progress.decoded_frames == 0));
        assert!(
            updates
                .windows(2)
                .all(|pair| pair[0].bytes_read <= pair[1].bytes_read)
        );
    }

    #[test]
    fn cancellation_between_chunks_stops_before_the_remaining_file_is_read() {
        let cancellation = Cancellation::default();
        let mut reader = Cursor::new(vec![1; CHUNK_BYTES * 3]);
        let result = read_snapshot(
            &mut reader,
            (CHUNK_BYTES * 3) as u64,
            &work(&cancellation),
            &mut |progress| {
                if progress.bytes_read > 0 {
                    cancellation.cancel();
                }
            },
        );
        assert_eq!(result.unwrap_err(), PreparationError::Cancelled);
        assert_eq!(reader.position(), CHUNK_BYTES as u64);
    }

    #[test]
    fn a_work_deadline_fails_without_sleep_or_an_audio_device() {
        let cancellation = Cancellation::default();
        let expired = Work {
            cancellation: &cancellation,
            deadline: Instant::now(),
        };
        let mut reader = Cursor::new(b"abc");
        assert_eq!(
            read_snapshot(&mut reader, 3, &expired, &mut |_| {}).unwrap_err(),
            PreparationError::TimedOut
        );
        assert_eq!(reader.position(), 0);
    }

    #[test]
    fn read_errors_and_progress_never_include_private_input_context() {
        struct FailedRead;
        impl Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("PRIVATE/library/title/token"))
            }
        }
        let cancellation = Cancellation::default();
        let error =
            read_snapshot(&mut FailedRead, 3, &work(&cancellation), &mut |_| {}).unwrap_err();
        assert_eq!(error, PreparationError::ReadFailed);
        assert!(!format!("{error:?}: {error}").contains("PRIVATE"));
        let error = prepare_selected(
            Path::new("PRIVATE/no-such-recording.mp3"),
            &cancellation,
            |_| {},
        )
        .err()
        .unwrap();
        assert_eq!(error, PreparationError::ReadFailed);
        assert!(!error.to_string().contains("PRIVATE"));
    }

    #[test]
    fn directory_and_growing_file_selections_are_rejected_without_a_scan() {
        let fixture = Fixture::new();
        let cancellation = Cancellation::default();
        assert!(matches!(
            prepare_selected(&fixture.0, &cancellation, |_| {}),
            Err(PreparationError::NotRegularFile)
        ));
        let path = fixture.file(&vec![1; CHUNK_BYTES * 2]);
        let mut changed = false;
        let result = prepare_selected(&path, &cancellation, |progress| {
            if progress.stage == Stage::Reading && progress.bytes_read > 0 && !changed {
                changed = true;
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(b"changed")
                    .unwrap();
            }
        });
        assert!(matches!(result, Err(PreparationError::Changed)));
    }

    #[test]
    fn unsupported_audio_and_cancellation_before_decoding_fail_closed() {
        let cancellation = Cancellation::default();
        let mono: Arc<[u8]> = Arc::from(include_bytes!("testdata/soundcloud/tone.mp3").as_slice());
        assert_eq!(
            validate_audio(mono, &work(&cancellation), &mut |_| {}).unwrap_err(),
            PreparationError::UnsupportedProfile
        );
        assert_eq!(
            validate_audio(
                Arc::from(b"not audio".as_slice()),
                &work(&cancellation),
                &mut |_| {}
            )
            .unwrap_err(),
            PreparationError::DecodeFailed
        );
        assert_eq!(
            validate_audio(
                Arc::from(b"not audio".as_slice()),
                &work(&cancellation),
                &mut |_| cancellation.cancel()
            )
            .unwrap_err(),
            PreparationError::Cancelled
        );
    }

    #[test]
    fn raw_mp3_profile_does_not_accept_embedded_frames_in_another_container() {
        let frame = [0xff, 0xfb, 0x90, 0];
        check_mp3_prefix(&frame).unwrap();
        let mut tagged = b"ID3\x04\0\0\0\0\0\x03abc".to_vec();
        tagged.extend(frame);
        check_mp3_prefix(&tagged).unwrap();
        let mut mp4 = b"\0\0\0\x18ftypmp42\0\0\0\0".to_vec();
        mp4.extend(frame);
        assert_eq!(check_mp3_prefix(&mp4), Err(PreparationError::DecodeFailed));
        tagged[6] = 0x80;
        assert_eq!(
            check_mp3_prefix(&tagged),
            Err(PreparationError::DecodeFailed)
        );
        tagged[6] = 0;
        tagged[5] = 0x10;
        assert_eq!(
            check_mp3_prefix(&tagged),
            Err(PreparationError::UnsupportedProfile)
        );
        assert_eq!(
            check_mp3_prefix(b"ID3\x04"),
            Err(PreparationError::DecodeFailed)
        );
    }

    #[test]
    fn resume_requires_size_digest_and_profile_and_output_has_no_path_or_tags() {
        let prepared = PreparedRecording {
            metadata: RecordingMetadata {
                size: 3,
                sha256: "a".repeat(64),
                profile: PROFILE.to_owned(),
                duration_ms: 12_000,
            },
            bytes: Arc::from(b"abc".as_slice()),
        };
        prepared.require_identity(prepared.metadata()).unwrap();
        let mut expected = prepared.metadata().clone();
        expected.duration_ms = 1; // Recomputed from the new snapshot, not trusted.
        prepared.require_identity(&expected).unwrap();
        for changed in [
            RecordingMetadata {
                size: 4,
                ..prepared.metadata().clone()
            },
            RecordingMetadata {
                sha256: "b".repeat(64),
                ..prepared.metadata().clone()
            },
            RecordingMetadata {
                profile: "wav".to_owned(),
                ..prepared.metadata().clone()
            },
        ] {
            assert_eq!(
                prepared.require_identity(&changed),
                Err(PreparationError::Changed)
            );
        }
        let json = serde_json::to_value(prepared.metadata()).unwrap();
        let mut keys = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(keys, ["duration_ms", "profile", "sha256", "size"]);
        let mut extra = json;
        extra["local_path"] = "PRIVATE".into();
        assert!(serde_json::from_value::<RecordingMetadata>(extra).is_err());
    }

    #[test]
    #[ignore = "requires ffmpeg to generate approved synthetic MP3 recordings"]
    fn synthetic_recording_prepares_rechecks_identity_and_cancels_during_decode() {
        fn synthetic(rate: u32, channels: u8, seconds: u8, frequency: u16) -> Vec<u8> {
            let generated = std::process::Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    &format!("sine=frequency={frequency}:sample_rate={rate}:duration={seconds}"),
                    "-ac",
                    &channels.to_string(),
                    "-c:a",
                    "libmp3lame",
                    "-b:a",
                    "128k",
                    "-metadata",
                    "title=PRIVATE testing title",
                    "-f",
                    "mp3",
                    "pipe:1",
                ])
                .output()
                .unwrap();
            assert!(generated.status.success());
            generated.stdout
        }
        let fixture = Fixture::new();
        let bytes = synthetic(44_100, 2, 12, 440);
        let path = fixture.file(&bytes);
        let cancellation = Cancellation::default();
        let prepared = prepare_selected(&path, &cancellation, |_| {}).unwrap();
        assert_eq!(prepared.bytes(), bytes);
        assert_eq!(
            prepared.metadata().sha256,
            format!("{:x}", Sha256::digest(&bytes))
        );
        assert!((12_000..12_100).contains(&prepared.metadata().duration_ms));
        assert!(
            !serde_json::to_string(prepared.metadata())
                .unwrap()
                .contains("PRIVATE")
        );
        let same = prepare_selected(&path, &cancellation, |_| {}).unwrap();
        same.require_identity(prepared.metadata()).unwrap();
        let replacement = synthetic(44_100, 2, 12, 880);
        std::fs::write(&path, &replacement).unwrap();
        let changed = prepare_selected(&path, &cancellation, |_| {}).unwrap();
        assert_eq!(
            changed.require_identity(prepared.metadata()),
            Err(PreparationError::Changed)
        );
        assert_eq!(prepared.bytes(), bytes); // Original snapshot stayed immutable.
        assert!(matches!(
            prepare_selected(&path, &cancellation, |progress| {
                if progress.decoded_frames > 0 {
                    cancellation.cancel();
                }
            }),
            Err(PreparationError::Cancelled)
        ));
        let mut edited = false;
        let changing = prepare_selected(&path, &Cancellation::default(), |progress| {
            if progress.decoded_frames > 0 && !edited {
                edited = true;
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap()
                    .write_all(b"changed during decode")
                    .unwrap();
            }
        });
        assert!(matches!(changing, Err(PreparationError::Changed)));
        for (rate, channels, seconds, expected) in [
            (48_000, 2, 12, PreparationError::UnsupportedProfile),
            (44_100, 1, 12, PreparationError::UnsupportedProfile),
            (44_100, 2, 2, PreparationError::DurationOutsidePilot),
        ] {
            fixture.file(&synthetic(rate, channels, seconds, 440));
            assert_eq!(
                prepare_selected(&path, &Cancellation::default(), |_| {})
                    .err()
                    .unwrap(),
                expected
            );
        }
    }
}
