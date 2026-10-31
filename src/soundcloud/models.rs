//! The web API's catalogue objects, kept alongside their app conversions.

use serde::{Deserialize, Serialize};

use super::{playlist_uri, track_uri, user_uri};
use crate::api::models as app;
pub use crate::api::models::PublisherMetadata;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Page<T> {
    pub collection: Vec<T>,
    #[serde(default)]
    pub next_href: Option<String>,
    #[serde(default)]
    pub total_results: Option<u32>,
}

impl<T> Page<T> {
    pub fn map<U>(self, convert: impl FnMut(T) -> U) -> Page<U> {
        Page {
            collection: self.collection.into_iter().map(convert).collect(),
            next_href: self.next_href,
            total_results: self.total_results,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct User {
    pub id: u64,
    #[serde(default)]
    pub username: String,
    pub avatar_url: Option<String>,
    pub permalink_url: Option<String>,
    pub followers_count: Option<u64>,
}

impl User {
    pub fn to_artist(&self) -> app::Artist {
        app::Artist {
            id: self.id.to_string(),
            name: self.username.clone(),
            uri: user_uri(self.id),
            images: artwork(self.avatar_url.as_deref()),
            followers: self.followers_count.map(|total| app::Followers { total }),
            external_urls: external_urls(self.permalink_url.as_deref()),
            ..Default::default()
        }
    }

    fn artist_ref(&self, name: Option<&str>) -> app::ArtistRef {
        app::ArtistRef {
            id: Some(self.id.to_string()),
            name: name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(&self.username)
                .to_owned(),
            uri: Some(user_uri(self.id)),
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Track {
    pub id: u64,
    pub title: Option<String>,
    #[serde(default)]
    pub duration: u32,
    pub full_duration: Option<u32>,
    pub artwork_url: Option<String>,
    pub created_at: Option<String>,
    pub release_date: Option<String>,
    pub display_date: Option<String>,
    pub playback_count: Option<u64>,
    pub label_name: Option<String>,
    pub permalink_url: Option<String>,
    pub user: Option<User>,
    pub publisher_metadata: Option<Box<PublisherMetadata>>,
    pub streamable: Option<bool>,
    pub policy: Option<String>,
    pub state: Option<String>,
    #[serde(default)]
    pub snipped: bool,
    pub media: Option<Media>,
    pub track_authorization: Option<String>,
}

impl std::fmt::Debug for Track {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Track")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("duration", &self.duration)
            .field("full_duration", &self.full_duration)
            .finish_non_exhaustive()
    }
}

impl Track {
    pub fn is_stub(&self) -> bool {
        self.title.is_none()
    }

    pub fn is_preview(&self) -> bool {
        self.policy.as_deref() == Some("SNIP")
            || self.snipped
            || self
                .full_duration
                .is_some_and(|full| shortened(self.duration, full))
    }

    pub fn is_playable(&self) -> bool {
        !self.is_stub()
            && self.streamable != Some(false)
            && self.policy.as_deref() != Some("BLOCK")
            && self
                .state
                .as_deref()
                .is_none_or(|state| state == "finished")
            && self.media.as_ref().is_some_and(|media| {
                media
                    .transcodings
                    .iter()
                    .any(|item| item.preference().is_some())
            })
    }

    /// Preserve the full catalogue duration even when playback is a preview.
    pub fn to_track(&self, playlist: Option<&Playlist>) -> app::Track {
        let artists = self
            .user
            .as_ref()
            .map(|user| {
                user.artist_ref(
                    self.publisher_metadata
                        .as_ref()
                        .and_then(|data| data.artist.as_deref()),
                )
            })
            .into_iter()
            .collect::<Vec<_>>();
        let images = artwork(
            self.artwork_url
                .as_deref()
                .filter(|url| !url.is_empty())
                .or_else(|| {
                    self.user
                        .as_ref()
                        .and_then(|user| user.avatar_url.as_deref())
                }),
        );
        let mut track = app::Track {
            id: Some(self.id.to_string()),
            name: self.title.clone().unwrap_or_default(),
            uri: track_uri(self.id),
            duration_ms: self.full_duration.unwrap_or(self.duration),
            release_date: self
                .release_date
                .as_ref()
                .filter(|date| !date.trim().is_empty())
                .or_else(|| {
                    self.display_date
                        .as_ref()
                        .filter(|date| !date.trim().is_empty())
                })
                .or_else(|| {
                    self.created_at
                        .as_ref()
                        .filter(|date| !date.trim().is_empty())
                })
                .cloned(),
            play_count: self.playback_count,
            publisher_metadata: self.publisher_metadata.clone(),
            label_name: self.label_name.clone(),
            external_ids: app::ExternalIds {
                isrc: self
                    .publisher_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.isrc.clone()),
            },
            artists: artists.clone(),
            album: Some(app::Album {
                id: playlist.map(|list| list.id.to_string()).unwrap_or_default(),
                uri: playlist
                    .map(|list| playlist_uri(list.id))
                    .unwrap_or_default(),
                name: playlist.map_or("SoundCloud", |list| &list.title).to_owned(),
                // Retain the original set share beside its identity. A fresh
                // client can use it to recover member access after restart.
                external_urls: external_urls(
                    playlist.and_then(|list| list.permalink_url.as_deref()),
                ),
                images,
                artists,
                ..Default::default()
            }),
            is_playable: Some(self.is_playable()),
            preview_only: self.is_preview(),
            external_urls: external_urls(self.permalink_url.as_deref()),
            ..Default::default()
        };
        crate::retitle::apply(&mut track);
        track
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Media {
    #[serde(default)]
    pub transcodings: Vec<Transcoding>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Transcoding {
    pub url: String,
    #[serde(default)]
    pub preset: String,
    #[serde(default)]
    pub duration: u32,
    #[serde(default)]
    pub snipped: bool,
    pub format: Format,
}

impl Transcoding {
    pub fn protocol(&self) -> Option<Protocol> {
        if self.url.contains("encrypted-hls") {
            return None;
        }
        match self.format.protocol.as_str() {
            "progressive" => Some(Protocol::Progressive),
            "hls" => Some(Protocol::Hls),
            _ => None,
        }
    }

    pub fn codec(&self) -> Option<&'static str> {
        let mime = self.format.mime_type.to_ascii_lowercase();
        let preset = self.preset.to_ascii_lowercase();
        // ABR manifests may contain encrypted variants. Only choose known,
        // individual encodings, never the web player's adaptive manifest.
        if preset.starts_with("abr") {
            None
        } else if mime.contains("opus") || preset.starts_with("opus") {
            Some("opus")
        } else if mime.contains("mp4a")
            || mime.starts_with("audio/mp4")
            || mime.starts_with("audio/aac")
            || preset.starts_with("aac")
        {
            Some("aac")
        } else if matches!(
            mime.split(';').next().map(str::trim),
            Some("audio/mpeg" | "audio/mp3")
        ) || preset.starts_with("mp3")
        {
            Some("mp3")
        } else {
            None
        }
    }

    pub(crate) fn preference(&self) -> Option<(u8, u8)> {
        let rank = match (self.protocol()?, self.codec()?) {
            (Protocol::Progressive, "mp3") => 0,
            (Protocol::Hls, "aac") => 1,
            (Protocol::Hls, "mp3") => 2,
            (Protocol::Progressive, "aac") => 3,
            // The shared Symphonia build decodes MP3 and AAC, not Opus.
            _ => return None,
        };
        Some((rank, u8::from(!self.preset.contains("160k"))))
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Format {
    pub protocol: String,
    pub mime_type: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Hls,
    Progressive,
}

#[derive(Clone)]
pub struct StreamInfo {
    pub url: String,
    pub protocol: Protocol,
    pub mime: String,
    pub codec: String,
    /// The selected stream's duration, which may be shorter than the track.
    pub duration_ms: u32,
    pub is_preview: bool,
}

impl std::fmt::Debug for StreamInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamInfo")
            .field("protocol", &self.protocol)
            .field("mime", &self.mime)
            .field("codec", &self.codec)
            .field("duration_ms", &self.duration_ms)
            .field("is_preview", &self.is_preview)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Playlist {
    pub id: u64,
    #[serde(default)]
    pub title: String,
    pub description: Option<String>,
    pub artwork_url: Option<String>,
    pub permalink_url: Option<String>,
    pub user: Option<User>,
    pub public: Option<bool>,
    pub is_album: Option<bool>,
    pub track_count: Option<u32>,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

impl Playlist {
    pub fn to_playlist(&self) -> app::Playlist {
        app::Playlist {
            id: self.id.to_string(),
            name: self.title.clone(),
            uri: playlist_uri(self.id),
            description: self.description.clone(),
            images: artwork(
                self.artwork_url
                    .as_deref()
                    .or_else(|| {
                        self.tracks
                            .iter()
                            .find_map(|track| track.artwork_url.as_deref())
                    })
                    .or_else(|| {
                        self.user
                            .as_ref()
                            .and_then(|user| user.avatar_url.as_deref())
                    }),
            ),
            owner: self
                .user
                .as_ref()
                .map(|user| app::Owner {
                    id: Some(user.id.to_string()),
                    display_name: Some(user.username.clone()),
                    uri: Some(user_uri(user.id)),
                })
                .unwrap_or_default(),
            public: self.public,
            tracks: Some(app::TrackCount {
                total: self.track_count.unwrap_or(self.tracks.len() as u32),
            }),
            external_urls: external_urls(self.permalink_url.as_deref()),
            ..Default::default()
        }
    }

    pub fn to_tracks(&self) -> Vec<app::Track> {
        self.tracks
            .iter()
            .map(|track| track.to_track(Some(self)))
            .collect()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind")]
pub enum Resolved {
    #[serde(rename = "track")]
    Track(Track),
    #[serde(rename = "playlist")]
    Playlist(Playlist),
    #[serde(rename = "user")]
    User(User),
}

#[derive(Deserialize)]
pub(crate) struct TrackLike {
    pub track: Track,
}

#[derive(Deserialize)]
pub(crate) struct PlaylistLike {
    pub playlist: Playlist,
}

/// Encoded stream durations can differ from the catalogue by a few samples.
/// A one-second tolerance avoids labeling that padding as a Go+ snippet.
pub(crate) fn shortened(duration: u32, full: u32) -> bool {
    duration.saturating_add(1_000) < full
}

fn artwork(url: Option<&str>) -> Vec<app::Image> {
    url.filter(|url| !url.is_empty())
        .map(|url| {
            let mut url = url.to_owned();
            let mut resized = false;
            if let Some((stem, ext)) = url.rsplit_once('.')
                && let Some((base, size)) = stem.rsplit_once('-')
                && matches!(
                    size,
                    "large"
                        | "mini"
                        | "tiny"
                        | "small"
                        | "badge"
                        | "crop"
                        | "original"
                        | "t67x67"
                        | "t300x300"
                        | "t500x500"
                )
            {
                url = format!("{base}-t500x500.{ext}");
                resized = true;
            }
            app::Image {
                url,
                width: resized.then_some(500),
                height: resized.then_some(500),
            }
        })
        .into_iter()
        .collect()
}

fn external_urls(url: Option<&str>) -> app::ExternalUrls {
    // The shared model has one external-link slot, named for its original
    // service. Its consumers open that URL without interpreting the name.
    app::ExternalUrls {
        spotify: url.map(str::to_owned),
    }
}
