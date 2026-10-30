//! Bounded HTTP range reads and VOD HLS. All HTTP runs on the app runtime;
//! Symphonia calls these adapters only from the engine's decoding thread.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use symphonia::core::io::MediaSource;
use tokio::runtime::Handle;
use tokio::sync::Notify;

use super::{Error, Protocol, Stream};

const RANGE_BYTES: u64 = 32 * 1024;
const MAX_OBJECT: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Interrupt {
    pub revision: AtomicU64,
    pub wake: Notify,
}

#[derive(Clone)]
pub(super) struct Fetch {
    pub http: crate::http::Http,
    pub runtime: Handle,
    pub interrupt: Arc<Interrupt>,
    pub revision: Arc<AtomicU64>,
    /// Per-preload cancellation also stops an already running blocking task.
    pub cancel: Arc<Interrupt>,
}

pub(super) struct Object {
    pub bytes: Vec<u8>,
    pub total: Option<u64>,
}

impl Fetch {
    pub fn check(&self) -> Result<(), Error> {
        if self.interrupt.revision.load(Ordering::SeqCst) == self.revision.load(Ordering::SeqCst)
            && self.cancel.revision.load(Ordering::SeqCst) == 0
        {
            Ok(())
        } else {
            Err(Error::Cancelled)
        }
    }

    pub async fn get(&self, url: &str, range: Option<(u64, u64)>) -> Result<Object, Error> {
        let wake = self.interrupt.wake.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        let cancel = self.cancel.wake.notified();
        tokio::pin!(cancel);
        cancel.as_mut().enable();
        self.check()?;
        let request = async {
            let client = self.http.client().map_err(Error::Network)?;
            let mut request = client.get(url).timeout(Duration::from_secs(15));
            if let Some((start, end)) = range {
                request = request.header(reqwest::header::RANGE, format!("bytes={start}-{end}"));
            }
            let mut response = request.send().await.map_err(network)?;
            let status = response.status();
            if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::NOT_FOUND
            {
                return Err(Error::Expired);
            }
            if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                return Ok(Object {
                    bytes: vec![],
                    total: None,
                });
            }
            response.error_for_status_ref().map_err(network)?;
            let content_range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|s| s.to_str().ok());
            let total = content_range
                .and_then(|s| s.rsplit_once('/')?.1.parse().ok())
                .or_else(|| {
                    (status == reqwest::StatusCode::OK)
                        .then(|| response.content_length())
                        .flatten()
                });
            if let Some((start, _)) = range {
                if status == reqwest::StatusCode::PARTIAL_CONTENT {
                    let received = content_range.and_then(|s| {
                        s.strip_prefix("bytes ")?
                            .split_once('-')?
                            .0
                            .parse::<u64>()
                            .ok()
                    });
                    if received != Some(start) {
                        return Err(Error::Network("Invalid HTTP range response".into()));
                    }
                } else if start != 0 {
                    return Err(Error::Unsupported(
                        "The stream server does not support byte ranges".into(),
                    ));
                }
                if status != reqwest::StatusCode::PARTIAL_CONTENT
                    && response
                        .content_length()
                        .is_some_and(|size| size > RANGE_BYTES)
                {
                    return Err(Error::Unsupported(
                        "The stream server ignored the byte range".into(),
                    ));
                }
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(network)? {
                let limit = if range.is_some() {
                    RANGE_BYTES as usize
                } else {
                    MAX_OBJECT
                };
                if bytes.len().saturating_add(chunk.len()) > limit {
                    return Err(Error::Unsupported(
                        "The stream response exceeds its size limit".into(),
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(Object { bytes, total })
        };
        tokio::select! {
            result = request => result,
            _ = &mut wake => Err(Error::Cancelled),
            _ = &mut cancel => Err(Error::Cancelled),
        }
    }

    pub fn read(&self, url: &str, range: Option<(u64, u64)>) -> Result<Object, Error> {
        self.runtime.block_on(self.get(url, range))
    }
}

fn network(error: reqwest::Error) -> Error {
    // Signed stream URLs and query tokens must never reach logs or the UI.
    Error::Network(error.without_url().to_string())
}

pub(super) type ReadError = Arc<Mutex<Option<Error>>>;

pub(super) struct RangeSource {
    fetch: Fetch,
    url: String,
    at: u64,
    start: u64,
    bytes: Vec<u8>,
    total: Option<u64>,
    error: ReadError,
}

impl RangeSource {
    pub fn new(fetch: Fetch, url: String, error: ReadError) -> Result<Self, Error> {
        let object = fetch.read(&url, Some((0, RANGE_BYTES - 1)))?;
        Ok(Self {
            fetch,
            url,
            at: 0,
            start: 0,
            bytes: object.bytes,
            total: object.total,
            error,
        })
    }
}

impl Read for RangeSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.total.is_some_and(|total| self.at >= total) {
            return Ok(0);
        }
        if self.at < self.start || self.at >= self.start + self.bytes.len() as u64 {
            match self.fetch.read(
                &self.url,
                Some((self.at, self.at.saturating_add(RANGE_BYTES - 1))),
            ) {
                Ok(object) => {
                    self.start = self.at;
                    self.bytes = object.bytes;
                    self.total = object.total.or(self.total);
                }
                Err(error) => {
                    *self.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
                    return Err(io::Error::other("Audio stream read failed"));
                }
            }
        }
        let offset = (self.at - self.start) as usize;
        let count = out.len().min(self.bytes.len().saturating_sub(offset));
        out[..count].copy_from_slice(&self.bytes[offset..offset + count]);
        self.at += count as u64;
        Ok(count)
    }
}

impl Seek for RangeSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(at) => i128::from(at),
            SeekFrom::Current(offset) => i128::from(self.at) + i128::from(offset),
            SeekFrom::End(offset) => {
                i128::from(
                    self.total
                        .ok_or_else(|| io::Error::other("Unknown stream length"))?,
                ) + i128::from(offset)
            }
        };
        self.at = u64::try_from(position).map_err(|_| io::Error::other("Invalid stream seek"))?;
        Ok(self.at)
    }
}

impl MediaSource for RangeSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        self.total
    }
}

#[derive(Clone, Debug)]
pub(super) struct Segment {
    pub url: String,
    pub start: f64,
    pub duration: f64,
    pub init: Option<String>,
}

/// SoundCloud's finite media playlists. Encryption, TS and live playlists
/// need different demuxers/contracts and are explicitly rejected.
pub(super) fn playlist(fetch: &Fetch, stream: &Stream) -> Result<Vec<Segment>, Error> {
    if stream.protocol != Protocol::Hls {
        return Ok(vec![]);
    }
    let mut url = stream.url.clone();
    for _ in 0..4 {
        let bytes = fetch.read(&url, None)?.bytes;
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| Error::Decode("Invalid HLS playlist".into()))?;
        let base =
            reqwest::Url::parse(&url).map_err(|_| Error::Decode("Invalid HLS URL".into()))?;
        let mut variant = false;
        let mut media = vec![];
        let mut duration = None;
        let mut start = 0.0;
        let mut init = None;
        let mut next = None;
        for line in text.lines().map(str::trim) {
            if line.starts_with("#EXT-X-STREAM-INF:") {
                variant = !line.to_ascii_lowercase().contains("opus");
            } else if let Some(value) = line.strip_prefix("#EXTINF:") {
                let value: f64 = value
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .parse()
                    .map_err(|_| Error::Decode("Invalid HLS duration".into()))?;
                if !value.is_finite() || value <= 0.0 {
                    return Err(Error::Decode("Invalid HLS duration".into()));
                }
                duration = Some(value);
            } else if line.starts_with("#EXT-X-MAP:") {
                if line.contains("BYTERANGE") {
                    return Err(Error::Unsupported(
                        "HLS byte-range segments are unsupported".into(),
                    ));
                }
                let value = line
                    .split("URI=\"")
                    .nth(1)
                    .and_then(|s| s.split('"').next())
                    .ok_or_else(|| Error::Decode("Invalid HLS initialization segment".into()))?;
                init = Some(join(&base, value)?);
            } else if line.starts_with("#EXT-X-KEY:") && !line.contains("METHOD=NONE") {
                return Err(Error::Unsupported("Encrypted HLS is unsupported".into()));
            } else if line.starts_with("#EXT-X-BYTERANGE:") {
                return Err(Error::Unsupported(
                    "HLS byte-range segments are unsupported".into(),
                ));
            } else if !line.is_empty() && !line.starts_with('#') {
                if variant {
                    next = Some(join(&base, line)?);
                    break;
                }
                if let Some(duration) = duration.take() {
                    media.push(Segment {
                        url: join(&base, line)?,
                        start,
                        duration,
                        init: init.clone(),
                    });
                    start += duration;
                }
            }
        }
        if let Some(next) = next {
            url = next;
            continue;
        }
        if media.is_empty() {
            return Err(Error::Unsupported("No supported HLS audio variant".into()));
        }
        if !text.lines().any(|line| line.trim() == "#EXT-X-ENDLIST") {
            return Err(Error::Unsupported("Live HLS is unsupported".into()));
        }
        return Ok(media);
    }
    Err(Error::Decode("Too many nested HLS playlists".into()))
}

fn join(base: &reqwest::Url, path: &str) -> Result<String, Error> {
    base.join(path)
        .map(|url| url.into())
        .map_err(|_| Error::Decode("Invalid HLS segment URL".into()))
}
