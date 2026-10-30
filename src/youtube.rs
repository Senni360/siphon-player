//! Audio-only YouTube catalogue and helper-managed yt-dlp adapter.
//!
//! No Google credentials or video downloads. Metadata is durable; signed audio
//! URLs stay in memory and are refreshed by the shared external PCM engine.

pub mod likes;
mod resolver;
#[cfg(test)]
mod tests;

use std::io::Read;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::api::models::{Album, ArtistRef, ExternalUrls, Image, Track};
pub use resolver::{Error, Resolver};

pub fn video_id(uri: &str) -> Option<&str> {
    let id = uri.strip_prefix("youtube:video:")?;
    valid_id(id).then_some(id)
}

fn valid_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn is_track_uri(uri: &str) -> bool {
    video_id(uri).is_some()
}

/// Canonicalise public and unlisted video links, never arbitrary subprocess input.
pub fn parse(text: &str) -> Option<String> {
    let text = text.trim();
    if is_track_uri(text) {
        return Some(text.into());
    }
    if text.len() > 4096 || text.chars().any(char::is_control) {
        return None;
    }
    let url = reqwest::Url::parse(text).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let id = match url.host_str()? {
        "youtu.be" | "www.youtu.be" => url.path().strip_prefix('/')?.to_owned(),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            if url.path() == "/watch" {
                url.query_pairs()
                    .find(|(key, _)| key == "v")?
                    .1
                    .into_owned()
            } else {
                url.path()
                    .strip_prefix("/shorts/")?
                    .trim_end_matches('/')
                    .into()
            }
        }
        _ => return None,
    };
    valid_id(&id).then(|| format!("youtube:video:{id}"))
}

pub fn permalink(uri: &str) -> Option<String> {
    Some(format!(
        "https://www.youtube.com/watch?v={}",
        video_id(uri)?
    ))
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Thumbnail {
    pub url: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Video {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub uploader: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub upload_date: Option<String>,
    #[serde(default)]
    pub view_count: Option<u64>,
    #[serde(default)]
    pub thumbnails: Vec<Thumbnail>,
    #[serde(default)]
    pub is_live: bool,
    #[serde(default)]
    pub live_status: Option<String>,
    #[serde(default)]
    pub availability: Option<String>,
}

impl Video {
    pub fn to_track(&self) -> Result<Track, Error> {
        if !valid_id(&self.id)
            || self.is_live
            || matches!(
                self.live_status.as_deref(),
                Some("is_live" | "is_upcoming" | "post_live")
            )
        {
            return Err(Error::Unavailable);
        }
        if matches!(
            self.availability.as_deref(),
            Some("private" | "premium_only" | "subscriber_only" | "needs_auth")
        ) {
            return Err(Error::SignIn);
        }
        let release_date = self.upload_date.as_deref().and_then(upload_date);
        // YouTube covers remain 16:9. Choose the greatest usable square crop;
        // the redesign can crop this image without changing the row's shape.
        let images = self
            .thumbnails
            .iter()
            .filter(|image| {
                reqwest::Url::parse(&image.url).is_ok_and(|url| {
                    url.scheme() == "https"
                        && url.username().is_empty()
                        && url.password().is_none()
                        && url.host_str().is_some_and(|host| {
                            host == "i.ytimg.com" || host.ends_with(".ytimg.com")
                        })
                })
            })
            .max_by_key(|image| {
                (
                    image.width.unwrap_or(0).min(image.height.unwrap_or(0)),
                    image.width.unwrap_or(0),
                )
            })
            .map(|image| {
                vec![Image {
                    url: image.url.clone(),
                    width: image.width,
                    height: image.height,
                }]
            })
            .unwrap_or_default();
        Ok(Track {
            id: Some(self.id.clone()),
            uri: format!("youtube:video:{}", self.id),
            name: self.title.clone(),
            artists: vec![ArtistRef {
                name: self
                    .channel
                    .as_ref()
                    .or(self.uploader.as_ref())
                    .cloned()
                    .unwrap_or_else(|| "YouTube".into()),
                ..Default::default()
            }],
            duration_ms: self
                .duration
                .filter(|d| d.is_finite() && *d > 0.0)
                .map_or(0, |d| (d * 1000.0).min(u32::MAX as f64) as u32),
            release_date: release_date.clone(),
            play_count: self.view_count,
            album: Some(Album {
                name: "YouTube".into(),
                release_date,
                images,
                ..Default::default()
            }),
            external_urls: ExternalUrls {
                spotify: permalink(&format!("youtube:video:{}", self.id)),
            },
            is_playable: Some(true),
            ..Default::default()
        })
    }
}

/// One file per video avoids concurrent metadata requests losing each other's
/// cache writes. This type is used only on the backend, never during drawing.
#[derive(Clone)]
pub struct Catalogue {
    pub resolver: Resolver,
    pub cache: PathBuf,
}

const MAX_METADATA_BYTES: u64 = 4 * 1024 * 1024;

fn read_cached_video(reader: impl Read) -> Option<Video> {
    // Metadata is only an early rejection: a file can grow after stat/open.
    // Read at most one excess byte so a growing cache cannot allocate freely.
    let mut bytes = Vec::new();
    reader
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

impl Catalogue {
    pub async fn track(&self, uri: &str) -> Result<Track, Error> {
        let id = video_id(uri).ok_or(Error::Unavailable)?.to_owned();
        let path = self.cache.join(format!("{id}.json"));
        let read_path = path.clone();
        if let Ok(Some(video)) = tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(read_path).ok()?;
            if file.metadata().ok()?.len() > MAX_METADATA_BYTES {
                return None;
            }
            read_cached_video(file)
        })
        .await
            && video.id == id
        {
            return video.to_track();
        }
        let video = self.resolver.metadata(&id).await?;
        let track = video.to_track()?;
        let bytes = serde_json::to_vec(&video).map_err(|_| Error::Metadata)?;
        tokio::task::spawn_blocking(move || {
            if fastframe_file::write_private(&path, &bytes).is_err() {
                log::warn!("Could not cache YouTube metadata");
            }
        })
        .await
        .map_err(|_| Error::Metadata)?;
        Ok(track)
    }
}

/// Shared decoder dispatch predicate. Spotify always keeps its own audio source.
pub fn is_external(uri: &str) -> bool {
    is_track_uri(uri) || crate::soundcloud::is_track_uri(uri)
}

/// Expiring URLs are never persisted or printed. A small margin covers handoff.
pub fn url_expired(url: &str) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.query_pairs()
                .find(|(key, _)| key == "expire")
                .and_then(|(_, value)| value.parse::<u64>().ok())
        })
        .is_some_and(|expires| expires <= now.saturating_add(30))
}

fn upload_date(date: &str) -> Option<String> {
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let formatted = format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..]);
    formatted
        .parse::<jiff::civil::Date>()
        .ok()
        .map(|date| date.to_string())
}
