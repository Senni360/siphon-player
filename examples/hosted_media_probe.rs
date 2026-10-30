//! Explicitly approved, small MP3 recording diagnostic. See local-sharing.md.
//! No uploads, account restoration, settings reads or persisted media.

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use librespot_playback::{
    audio_backend::Sink, convert::Converter, decoder::AudioPacket, mixer::VolumeGetter,
};
use reqwest::{blocking::Client, header};
use serde::Deserialize;
use spotifast::{
    cast::media::Receiver,
    sink::{AudioControl, DEFAULT_BUFFER_MS, RodioSink},
};
use std::{
    io::{Cursor, Read},
    net::IpAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::DecoderOptions,
    formats::{SeekMode, SeekTo},
    io::MediaSourceStream,
    probe::Hint,
    units::Time,
};

const MAX_BYTES: u64 = 16 * 1024 * 1024;
const RANGE_BYTES: u64 = 32 * 1024;

#[derive(Parser)]
#[command(about = "Opt-in hosted MP3 diagnostic; launches a receiver only with --cast")]
struct Args {
    /// Private JSON file containing url, expires_at_unix and duration_seconds.
    manifest: PathBuf,
    /// Confirms the recording and any receiver interruption are approved.
    #[arg(long)]
    approved_test_media: bool,
    /// Play on this computer's default audio output at 10% gain.
    #[arg(long, conflicts_with = "cast")]
    desktop: bool,
    /// Device IP address. Replaces its current receiver app.
    #[arg(long)]
    cast: Option<IpAddr>,
    #[arg(long, default_value_t = 8009)]
    port: u16,
    /// Initial desktop position, or target of a Cast seek after two seconds.
    #[arg(long, default_value_t = 5.0)]
    seek_seconds: f64,
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u64).range(3..=60))]
    seconds: u64,
    /// Check that an expired or revoked URL refuses a new range request.
    #[arg(long, conflicts_with_all = ["desktop", "cast"])]
    expect_denied: bool,
    /// Trust this additional PEM certificate for a local HTTPS test gateway.
    /// Does not change system trust or the receiver's own trust store.
    #[arg(long, conflicts_with = "cast")]
    ca_certificate: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    url: String,
    expires_at_unix: u64,
    duration_seconds: u64,
}

#[derive(Clone, Debug)]
struct RecordingVersion {
    size: u64,
    etag: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if !args.approved_test_media {
        bail!("explicit approval requires --approved-test-media");
    }
    let file = std::fs::File::open(&args.manifest).context("cannot open private manifest")?;
    let mut data = Vec::new();
    file.take(16 * 1024 + 1).read_to_end(&mut data)?;
    if data.len() > 16 * 1024 {
        bail!("manifest exceeds 16 KiB");
    }
    let manifest: Manifest =
        serde_json::from_slice(&data).map_err(|_| anyhow!("invalid private manifest"))?;
    validate_manifest(&manifest, now()?, args.expect_denied)?;
    if !args.seek_seconds.is_finite()
        || args.seek_seconds < 0.0
        || args.seek_seconds >= manifest.duration_seconds as f64
    {
        bail!("seek target must be inside the declared recording duration");
    }
    let mut client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .no_gzip();
    if let Some(path) = &args.ca_certificate {
        let file = std::fs::File::open(path).context("cannot open test certificate")?;
        let mut pem = Vec::new();
        file.take(64 * 1024 + 1).read_to_end(&mut pem)?;
        if pem.len() > 64 * 1024 {
            bail!("test certificate exceeds 64 KiB");
        }
        let certificate = reqwest::Certificate::from_pem(&pem)
            .map_err(|_| anyhow!("invalid PEM test certificate"))?;
        client = client.add_root_certificate(certificate);
    }
    let client = client.build().map_err(redact)?;
    if args.expect_denied {
        let response = request(&client, &manifest.url, 0, 0, None)?;
        if !matches!(response.status().as_u16(), 401 | 403 | 404 | 410) {
            bail!("temporary access is still usable or returned an unexpected status");
        }
        println!("Temporary access denied on a new range request.");
        return Ok(());
    }
    let (first, version) = range(&client, &manifest.url, 0, None)?;
    range(
        &client,
        &manifest.url,
        version.size.saturating_sub(RANGE_BYTES),
        Some(&version),
    )?;
    println!(
        "HTTPS MP3 range checks passed: {} bytes, first and final ranges.",
        version.size
    );
    if args.desktop {
        let mut bytes = first;
        while (bytes.len() as u64) < version.size {
            validate_manifest(&manifest, now()?, false)?;
            bytes.extend(range(&client, &manifest.url, bytes.len() as u64, Some(&version))?.0);
        }
        play_desktop(bytes, args.seek_seconds, args.seconds)?;
    } else if let Some(address) = args.cast {
        let mut receiver = Receiver::connect(address, args.port)?;
        let result = (|| -> Result<()> {
            let loaded = receiver.load(&manifest.url, 0.0)?;
            println!("Cast loaded: {loaded:?}");
            let started = Instant::now();
            let mut sought = false;
            while started.elapsed() < Duration::from_secs(args.seconds) {
                std::thread::sleep(Duration::from_millis(500));
                if !sought && started.elapsed() >= Duration::from_secs(2) {
                    println!(
                        "Cast seek response: {:?}",
                        receiver.seek(args.seek_seconds)?
                    );
                    sought = true;
                }
                println!(
                    "elapsed_ms={} status={:?}",
                    started.elapsed().as_millis(),
                    receiver.status()?
                );
            }
            Ok(())
        })();
        let stopped = receiver.stop();
        if stopped.is_err() {
            eprintln!(
                "Receiver stop was not confirmed; use its normal controls if audio continues."
            );
        }
        result?;
        stopped.context("could not confirm receiver stop; stop it using its normal controls")?;
    }
    Ok(())
}

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
fn redact(error: reqwest::Error) -> anyhow::Error {
    anyhow!("{}", error.without_url())
}

fn validate_manifest(manifest: &Manifest, now: u64, allow_expired: bool) -> Result<()> {
    let url = reqwest::Url::parse(&manifest.url).map_err(|_| anyhow!("invalid media URL"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        bail!("HTTPS without user information or fragment required");
    }
    if !(10..=3600).contains(&manifest.duration_seconds) {
        bail!("test recording must be 10 to 3600 seconds");
    }
    if !allow_expired
        && manifest.expires_at_unix.saturating_sub(now) < manifest.duration_seconds + 600
    {
        bail!("refresh playback access: it must outlive the recording by ten minutes");
    }
    Ok(())
}

fn request(
    client: &Client,
    url: &str,
    start: u64,
    end: u64,
    etag: Option<&str>,
) -> Result<reqwest::blocking::Response> {
    let mut request = client
        .get(url)
        .header(header::RANGE, format!("bytes={start}-{end}"))
        .header(header::ACCEPT_ENCODING, "identity");
    if let Some(etag) = etag {
        request = request.header(header::IF_MATCH, etag);
    }
    request.send().map_err(redact)
}

fn range(
    client: &Client,
    url: &str,
    start: u64,
    version: Option<&RecordingVersion>,
) -> Result<(Vec<u8>, RecordingVersion)> {
    let total = version.map(|version| version.size);
    let end = total.map_or(start + RANGE_BYTES - 1, |total| {
        (start + RANGE_BYTES).min(total) - 1
    });
    let response = request(
        client,
        url,
        start,
        end,
        version.map(|version| version.etag.as_str()),
    )?;
    if response.status().as_u16() != 206 {
        bail!("media server must return 206 for ranges (access may have expired)");
    }
    let mime = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if mime.split(';').next() != Some("audio/mpeg") {
        bail!("spike accepts audio/mpeg only");
    }
    if response.headers().contains_key(header::CONTENT_ENCODING) {
        bail!("compressed HTTP responses are not seekable by this probe");
    }
    let etag = strong_etag(response.headers())?;
    if version.is_some_and(|version| version.etag != etag) {
        bail!("recording changed between range reads; obtain fresh playback access");
    }
    let header = response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .context("missing Content-Range")?;
    let size = validate_range(header, start, end, total)?;
    let expected = end.min(size - 1) - start + 1;
    let mut bytes = Vec::new();
    response
        .take(expected + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("failed reading media range"))?;
    if bytes.len() as u64 != expected {
        bail!("range body length mismatch");
    }
    Ok((bytes, RecordingVersion { size, etag }))
}

fn strong_etag(headers: &header::HeaderMap) -> Result<String> {
    let mut values = headers.get_all(header::ETAG).iter();
    let etag = values
        .next()
        .and_then(|value| value.to_str().ok())
        .context("media requires a strong ETag for consistent range reads")?;
    let opaque = etag
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'));
    if values.next().is_some()
        || etag.len() > 1024
        || opaque.is_none_or(|value| {
            !value
                .bytes()
                .all(|byte| byte == 0x21 || (0x23..=0x7e).contains(&byte))
        })
    {
        bail!("media requires one valid strong ETag");
    }
    Ok(etag.to_owned())
}

fn validate_range(header: &str, start: u64, end: u64, total: Option<u64>) -> Result<u64> {
    let parsed = (|| {
        let (interval, size) = header.strip_prefix("bytes ")?.split_once('/')?;
        let (first, last) = interval.split_once('-')?;
        Some((
            first.parse::<u64>().ok()?,
            last.parse::<u64>().ok()?,
            size.parse::<u64>().ok()?,
        ))
    })();
    let (first, last, size) = parsed.context("invalid Content-Range")?;
    if size == 0
        || size > MAX_BYTES
        || first != start
        || first >= size
        || last != end.min(size - 1)
        || total.is_some_and(|total| total != size)
    {
        bail!("range mismatch, changed object, or recording exceeds 16 MiB spike limit");
    }
    Ok(size)
}

// This small-file acoustic check uses the application's existing native sink.
// Production sharing must reuse the streaming engine, not this in-memory probe.
fn play_desktop(bytes: Vec<u8>, seek: f64, seconds: u64) -> Result<()> {
    let mut audio = DesktopAudio::new(bytes, seek)?;
    struct QuietVolume;
    impl VolumeGetter for QuietVolume {
        fn attenuation_factor(&self) -> f64 {
            0.1
        }
    }
    let mut sink = RodioSink::new(
        None,
        Arc::new(|_| eprintln!("Native audio output failed")),
        Box::new(QuietVolume),
        DEFAULT_BUFFER_MS,
        AudioControl::new(DEFAULT_BUFFER_MS),
    );
    let mut converter = Converter::new(None);
    sink.start()?;
    let result = (|| -> Result<()> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(seconds) {
            let Some(samples) = audio.next()? else {
                break;
            };
            sink.write(AudioPacket::Samples(samples), &mut converter)?;
        }
        println!(
            "Desktop probe completed from requested position {seek}s; confirm sound and seek by ear."
        );
        Ok(())
    })();
    let stopped = sink.stop();
    result?;
    stopped?;
    Ok(())
}

struct DesktopAudio {
    format: Box<dyn symphonia::core::formats::FormatReader>,
    codec: Box<dyn symphonia::core::codecs::Decoder>,
    track: u32,
    time_base: symphonia::core::units::TimeBase,
    seek: f64,
}

impl DesktopAudio {
    fn new(bytes: Vec<u8>, seek: f64) -> Result<Self> {
        let source = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
        let mut probed = symphonia::default::get_probe().format(
            Hint::new().with_extension("mp3"),
            source,
            &Default::default(),
            &Default::default(),
        )?;
        let track = probed.format.default_track().context("no audio track")?;
        if track.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_MP3
            || track.codec_params.sample_rate != Some(44_100)
            || track
                .codec_params
                .channels
                .is_none_or(|channels| channels.count() != 2)
        {
            bail!("desktop spike requires 44.1 kHz stereo MP3");
        }
        let id = track.id;
        let time_base = track
            .codec_params
            .time_base
            .context("missing MP3 time base")?;
        let codec = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())?;
        probed.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from(seek),
                track_id: Some(id),
            },
        )?;
        Ok(Self {
            format: probed.format,
            codec,
            track: id,
            time_base,
            seek,
        })
    }

    fn next(&mut self) -> Result<Option<Vec<f64>>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(symphonia::core::errors::Error::IoError(error))
                    if error.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            };
            if packet.track_id() != self.track {
                continue;
            }
            let time = self.time_base.calc_time(packet.ts());
            let before = (self.seek - (time.seconds as f64 + time.frac)).max(0.0);
            let decoded = self.codec.decode(&packet)?;
            if decoded.spec().rate != 44_100 || decoded.spec().channels.count() != 2 {
                bail!("audio format changed");
            }
            let mut buffer = SampleBuffer::<f64>::new(decoded.capacity() as u64, *decoded.spec());
            buffer.copy_interleaved_ref(decoded);
            let skip = ((before * 44_100.0).round() as usize)
                .saturating_mul(2)
                .min(buffer.len());
            if skip < buffer.len() {
                return Ok(Some(buffer.samples()[skip..].to_vec()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expiry_uses_a_deterministic_clock_and_requires_seek_margin() {
        let mut manifest = Manifest {
            url: "https://media.example/test?secret=private".into(),
            expires_at_unix: 1700,
            duration_seconds: 100,
        };
        assert!(validate_manifest(&manifest, 1000, false).is_ok());
        assert!(validate_manifest(&manifest, 1001, false).is_err());
        manifest.expires_at_unix = 999;
        assert!(validate_manifest(&manifest, 1000, false).is_err());
        assert!(validate_manifest(&manifest, 1000, true).is_ok());
    }
    #[test]
    fn ranges_are_bounded_and_cannot_change_object_size() {
        assert_eq!(
            validate_range("bytes 32-63/64", 32, 63, Some(64)).unwrap(),
            64
        );
        for header in [
            "bytes 0-63/64",
            "bytes 32-62/64",
            "bytes 32-63/65",
            "bytes 32-63/999999999",
            "bytes 32-63/*",
        ] {
            assert!(validate_range(header, 32, 63, Some(64)).is_err());
        }
    }

    #[test]
    #[ignore = "requires ffmpeg to generate approved synthetic media in memory"]
    fn synthetic_mp3_decodes_seeks_and_finishes_without_an_audio_device() {
        let generated = std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=44100:duration=1.2",
                "-ac",
                "2",
                "-c:a",
                "libmp3lame",
                "-b:a",
                "128k",
                "-f",
                "mp3",
                "pipe:1",
            ])
            .output()
            .unwrap();
        assert!(generated.status.success());
        let bytes = generated.stdout;
        let samples = |seek| {
            let mut audio = DesktopAudio::new(bytes.clone(), seek).unwrap();
            let mut samples = Vec::new();
            while let Some(next) = audio.next().unwrap() {
                samples.extend(next);
            }
            samples
        };
        let full = samples(0.0);
        let sought = samples(0.2);
        assert!(sought.iter().any(|sample| sample.abs() > 0.001));
        assert_eq!(full.len() - sought.len(), 2 * 8820);
        assert!(DesktopAudio::new(b"not an MP3".to_vec(), 0.0).is_err());
    }

    #[test]
    fn desktop_probe_rejects_other_sample_layouts() {
        let mono = include_bytes!("../src/testdata/soundcloud/tone.mp3").to_vec();
        assert!(DesktopAudio::new(mono, 0.0).is_err());
    }

    fn serve(reply: &'static str) -> (String, std::thread::JoinHandle<String>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "http://{}/test?grant=private",
            listener.local_addr().unwrap()
        );
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            socket.write_all(reply.as_bytes()).unwrap();
            String::from_utf8(request).unwrap()
        });
        (url, thread)
    }

    #[test]
    fn real_range_reader_checks_headers_body_and_sends_no_provider_grant() {
        let (url, server) = serve(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mpeg\r\nETag: \"original\"\r\nContent-Range: bytes 0-3/4\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntest",
        );
        let client = Client::builder().no_proxy().build().unwrap();
        let (body, version) = range(&client, &url, 0, None).unwrap();
        assert_eq!(body, b"test");
        assert_eq!(version.size, 4);
        assert_eq!(version.etag, "\"original\"");
        let sent = server.join().unwrap().to_ascii_lowercase();
        assert!(sent.contains("range: bytes=0-32767"));
        assert!(sent.contains("accept-encoding: identity"));
        assert!(!sent.contains("authorization:"));
        assert!(!sent.contains("cookie:"));
        for response in [
            "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntest",
            "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mpeg\r\nETag: \"original\"\r\nContent-Range: bytes 0-3/4\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad",
            "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mpeg\r\nETag: \"original\"\r\nContent-Range: bytes 0-3/4\r\nContent-Length: 5\r\nConnection: close\r\n\r\nextra",
            "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mpeg\r\nETag: \"original\"\r\nContent-Range: bytes 0-3/4\r\nContent-Encoding: br\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntest",
        ] {
            let (url, server) = serve(response);
            let error = range(&client, &url, 0, None).unwrap_err();
            assert!(!format!("{error:?}").contains("private"));
            server.join().unwrap();
        }
    }

    #[test]
    fn subsequent_ranges_pin_the_original_etag_even_when_size_is_unchanged() {
        let client = Client::builder().no_proxy().build().unwrap();
        let original = RecordingVersion {
            size: 4,
            etag: "\"original\"".into(),
        };
        let (url, server) = serve(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mpeg\r\nETag: \"original\"\r\nContent-Range: bytes 2-3/4\r\nContent-Length: 2\r\nConnection: close\r\n\r\nst",
        );
        assert_eq!(range(&client, &url, 2, Some(&original)).unwrap().0, b"st");
        let sent = server.join().unwrap().to_ascii_lowercase();
        assert!(sent.contains("range: bytes=2-3"));
        assert!(sent.contains("if-match: \"original\""));
        for response in [
            "HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mpeg\r\nETag: \"replacement\"\r\nContent-Range: bytes 2-3/4\r\nContent-Length: 2\r\nConnection: close\r\n\r\nXX",
            "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mpeg\r\nContent-Range: bytes 2-3/4\r\nContent-Length: 2\r\nConnection: close\r\n\r\nXX",
        ] {
            let (url, server) = serve(response);
            assert!(range(&client, &url, 2, Some(&original)).is_err());
            server.join().unwrap();
        }
    }

    #[test]
    fn weak_malformed_or_ambiguous_etags_cannot_pin_a_recording() {
        for value in [
            "W/\"version\"",
            "version",
            "\"one\", \"two\"",
            "\"has space\"",
            "\"unterminated",
        ] {
            let mut headers = header::HeaderMap::new();
            headers.insert(header::ETAG, value.parse().unwrap());
            assert!(strong_etag(&headers).is_err());
        }
        let mut headers = header::HeaderMap::new();
        assert!(strong_etag(&headers).is_err());
        headers.append(header::ETAG, "\"first\"".parse().unwrap());
        headers.append(header::ETAG, "\"second\"".parse().unwrap());
        assert!(strong_etag(&headers).is_err());
        headers.clear();
        headers.insert(header::ETAG, "\"hash-1,opaque\"".parse().unwrap());
        assert_eq!(strong_etag(&headers).unwrap(), "\"hash-1,opaque\"");
    }

    #[test]
    fn network_errors_do_not_expose_temporary_access() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let error = request(
            &client,
            &format!("http://{address}/test?grant=private"),
            0,
            1,
            None,
        )
        .unwrap_err();
        assert!(!format!("{error:?}").contains("private"));
        assert!(!format!("{error:?}").contains("grant="));
    }
}
