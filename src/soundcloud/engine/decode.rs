use std::collections::VecDeque;
use std::io::Cursor;
use std::sync::{Arc, Mutex, PoisonError};

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{Decoder as AudioDecoder, DecoderOptions};
use symphonia::core::errors::Error as DecodeError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Time, TimeBase};

use super::source::{Fetch, RangeSource, ReadError, Segment, playlist};
use super::{Error, Protocol, Stream};

/// One demuxer/decoder, with the exact start position expressed in seconds
/// of its own timestamps. fMP4 timestamps can begin at a nonzero offset.
struct Audio {
    format: Box<dyn FormatReader>,
    codec: Box<dyn AudioDecoder>,
    track: u32,
    time_base: TimeBase,
    skip_until: f64,
    origin: Option<f64>,
    relative: bool,
    resampler: Option<crate::resample::Resampler>,
    rate: u32,
    codec_type: symphonia::core::codecs::CodecType,
    error: ReadError,
}

impl Audio {
    fn open(
        source: Box<dyn MediaSource>,
        hint: &str,
        error: ReadError,
        skip: f64,
        seek: bool,
    ) -> Result<Self, Error> {
        let mut probed = symphonia::default::get_probe()
            .format(
                Hint::new().with_extension(hint),
                MediaSourceStream::new(source, Default::default()),
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(|e| translate(e, &error))?;
        let track = probed
            .format
            .default_track()
            .ok_or_else(|| Error::Decode("No audio track in stream".into()))?;
        let id = track.id;
        let rate = track
            .codec_params
            .sample_rate
            .ok_or_else(|| Error::Decode("Missing audio sample rate".into()))?;
        if rate == 0 || rate > 384_000 {
            return Err(Error::Unsupported("Unsupported audio sample rate".into()));
        }
        let time_base = track
            .codec_params
            .time_base
            .unwrap_or(TimeBase::new(1, rate));
        let codec_type = track.codec_params.codec;
        let codec = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|e| translate(e, &error))?;
        if seek && skip > 0.0 {
            probed
                .format
                .seek(
                    SeekMode::Accurate,
                    SeekTo::Time {
                        time: Time::from(skip),
                        track_id: Some(id),
                    },
                )
                .map_err(|e| translate(e, &error))?;
        }
        Ok(Self {
            format: probed.format,
            codec,
            track: id,
            time_base,
            skip_until: skip,
            origin: None,
            relative: !seek,
            resampler: crate::resample::Resampler::new(rate, 44_100, 2),
            rate,
            codec_type,
            error,
        })
    }

    fn next(&mut self) -> Result<Option<Vec<f64>>, Error> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(DecodeError::IoError(ref e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    if let Some(error) = self
                        .error
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take()
                    {
                        return Err(error);
                    }
                    return Ok(None);
                }
                Err(error) => return Err(translate(error, &self.error)),
            };
            if packet.track_id() != self.track {
                continue;
            }
            let time = self.time_base.calc_time(packet.ts);
            let time = time.seconds as f64 + time.frac;
            let origin = *self
                .origin
                .get_or_insert(if self.relative { time } else { 0.0 });
            let decoded = self
                .codec
                .decode(&packet)
                .map_err(|e| translate(e, &self.error))?;
            let spec = *decoded.spec();
            let channels = spec.channels.count();
            if spec.rate != self.rate || !(1..=2).contains(&channels) {
                return Err(Error::Unsupported(
                    "Only mono or stereo streams with a fixed sample rate are supported".into(),
                ));
            }
            let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            buffer.copy_interleaved_ref(decoded);
            let frames = buffer.len() / channels;
            let skip = ((self.skip_until - (time - origin)).max(0.0) * f64::from(self.rate)).round()
                as usize;
            if skip >= frames {
                continue;
            }
            let mut stereo = Vec::with_capacity((frames - skip) * 2);
            for frame in buffer.samples()[skip * channels..].chunks_exact(channels) {
                stereo.extend_from_slice(&[frame[0], frame[channels - 1]]);
            }
            let samples = match &mut self.resampler {
                Some(resampler) => resampler.process(&stereo),
                None => stereo,
            };
            if samples.is_empty() {
                continue;
            }
            return Ok(Some(samples.into_iter().map(f64::from).collect()));
        }
    }
}

fn translate(error: DecodeError, read: &ReadError) -> Error {
    read.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .unwrap_or_else(|| match error {
            DecodeError::Unsupported(reason) => Error::Unsupported(reason.to_string()),
            _ => Error::Decode(error.to_string()),
        })
}

/// Keeps one compressed HLS segment ahead. Dropping/pausing aborts its HTTP
/// task; no periodic timers, decoder polling or device callbacks run paused.
pub(super) struct Decoder {
    audio: Audio,
    fetch: Fetch,
    segments: Vec<Segment>,
    segment: usize,
    ahead: Option<tokio::task::JoinHandle<Result<Vec<u8>, Error>>>,
    hint: String,
    flushed: bool,
    held: VecDeque<Vec<f64>>,
    deferred_error: Option<Error>,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.suspend();
    }
}

impl Decoder {
    pub fn open(fetch: Fetch, stream: &Stream, position: u32) -> Result<Self, Error> {
        if stream.mime.to_ascii_lowercase().contains("opus") {
            return Err(Error::Unsupported(
                "Opus HLS is unsupported. Select an MP3 or AAC transcoding".into(),
            ));
        }
        let error = Arc::new(Mutex::new(None));
        let hint = if stream.mime.contains("mp4") {
            "m4a"
        } else if stream.mime.contains("aac") {
            "aac"
        } else {
            "mp3"
        }
        .to_string();
        if stream.protocol == Protocol::Progressive {
            let source = RangeSource::new(fetch.clone(), stream.url.clone(), Arc::clone(&error))?;
            let audio = Audio::open(
                Box::new(source),
                &hint,
                error,
                f64::from(position) / 1_000.0,
                true,
            )?;
            return Ok(Self {
                audio,
                fetch,
                segments: vec![],
                segment: 0,
                ahead: None,
                hint,
                flushed: false,
                held: VecDeque::new(),
                deferred_error: None,
            });
        }
        let segments = playlist(&fetch, stream)?;
        let seconds = f64::from(position) / 1_000.0;
        let segment = segments
            .iter()
            .position(|s| seconds < s.start + s.duration)
            .unwrap_or(segments.len() - 1);
        let bytes = fetch
            .runtime
            .block_on(segment_bytes(&fetch, &segments[segment]))?;
        let segment_hint = if segments[segment].init.is_some() {
            "m4a"
        } else {
            &hint
        };
        let audio = Audio::open(
            Box::new(Cursor::new(bytes)),
            segment_hint,
            error,
            (seconds - segments[segment].start).max(0.0),
            false,
        )?;
        Ok(Self {
            audio,
            fetch,
            segments,
            segment,
            ahead: None,
            hint,
            flushed: false,
            held: VecDeque::new(),
            deferred_error: None,
        })
    }

    pub fn resume(&mut self, revision: u64) {
        self.fetch
            .revision
            .store(revision, std::sync::atomic::Ordering::SeqCst);
        self.prefetch();
    }

    pub fn suspend(&mut self) {
        if let Some(ahead) = self.ahead.take() {
            ahead.abort();
        }
    }

    fn prefetch(&mut self) {
        if self.ahead.is_none()
            && let Some(segment) = self.segments.get(self.segment + 1).cloned()
        {
            let fetch = self.fetch.clone();
            self.ahead = Some(
                self.fetch
                    .runtime
                    .spawn(async move { segment_bytes(&fetch, &segment).await }),
            );
        }
    }

    pub fn next(&mut self) -> Result<Option<Vec<f64>>, Error> {
        if let Some(pcm) = self.held.pop_front() {
            self.fetch.check()?;
            return Ok(Some(pcm));
        }
        if let Some(error) = self.deferred_error.take() {
            return Err(error);
        }
        self.decode_next()
    }

    /// Decode ahead without changing the playback position. Limit memory to
    /// three seconds and stop after 150 ms between packets. An in-flight HTTP
    /// read retains the normal request deadline and transport cancellation.
    pub fn loudness_lookahead(&mut self) -> Result<Vec<f64>, Error> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(150);
        let mut samples = Vec::new();
        while samples.len() / 2 < crate::loudness::LOOKAHEAD_FRAMES
            && std::time::Instant::now() < deadline
        {
            match self.decode_next() {
                Ok(Some(pcm)) => {
                    samples.extend_from_slice(&pcm);
                    self.held.push_back(pcm);
                }
                Ok(None) => break,
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) => {
                    // Play successfully buffered audio before asking for a
                    // refreshed URL, as ordinary packet-by-packet decode does.
                    self.deferred_error = Some(error);
                    break;
                }
            }
        }
        Ok(samples)
    }

    fn decode_next(&mut self) -> Result<Option<Vec<f64>>, Error> {
        self.fetch.check()?;
        self.prefetch();
        loop {
            if let Some(samples) = self.audio.next()? {
                return Ok(Some(samples));
            }
            let Some(segment) = self.segments.get(self.segment + 1) else {
                if !self.flushed {
                    self.flushed = true;
                    if let Some(resampler) = &mut self.audio.resampler {
                        let tail = resampler.process(&[0.0; 64]);
                        if !tail.is_empty() {
                            return Ok(Some(tail.into_iter().map(f64::from).collect()));
                        }
                    }
                }
                return Ok(None);
            };
            let bytes = if let Some(ahead) = self.ahead.take() {
                self.fetch
                    .runtime
                    .block_on(ahead)
                    .map_err(|_| Error::Cancelled)??
            } else {
                self.fetch
                    .runtime
                    .block_on(segment_bytes(&self.fetch, segment))?
            };
            self.segment += 1;
            let mut next = Audio::open(
                Box::new(Cursor::new(bytes)),
                if segment.init.is_some() {
                    "m4a"
                } else {
                    &self.hint
                },
                Arc::new(Mutex::new(None)),
                0.0,
                false,
            )?;
            if next.rate == self.audio.rate && next.codec_type == self.audio.codec_type {
                // Preserve AAC overlap and the resampler's filter history
                // across segments, avoiding clicks and dropped tail samples.
                std::mem::swap(&mut next.codec, &mut self.audio.codec);
                next.resampler = self.audio.resampler.take();
            }
            self.audio = next;
            self.prefetch();
        }
    }
}

async fn segment_bytes(fetch: &Fetch, segment: &Segment) -> Result<Vec<u8>, Error> {
    if let Some(init) = &segment.init {
        let (init, audio) = tokio::join!(fetch.get(init, None), fetch.get(&segment.url, None));
        let mut bytes = init?.bytes;
        bytes.extend(audio?.bytes);
        Ok(bytes)
    } else {
        Ok(fetch.get(&segment.url, None).await?.bytes)
    }
}
