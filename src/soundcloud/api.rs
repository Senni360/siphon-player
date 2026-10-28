//! Async SoundCloud web API v2 reads. Use the backend Tokio runtime.
//!
//! Transcodings checked on 2026-10-28: SoundCloud announced AAC HLS at
//! 160/96 kbit/s and retirement of public API MP3/Opus by December 2025:
//! https://developers.soundcloud.com/blog/api-streaming-urls/
//! The web API is a separate surface: yt-dlp still handles legacy MP3,
//! AAC HLS and Opus, and skips CTR/CBC encrypted formats as DRM:
//! https://github.com/yt-dlp/yt-dlp/blob/master/yt_dlp/extractor/soundcloud.py
//! https://github.com/yt-dlp/yt-dlp/issues/17335
//! The 2026-10-28 anonymous fixtures include AAC 96k and an adaptive manifest.
//! Prefer progressive MP3 when actually offered, then AAC HLS (160k first),
//! HLS MP3 and HLS Opus. Never resolve encrypted or adaptive transcodings.
//! Discovery follows yt-dlp's reverse asset scan. A WAF challenge can make
//! discovery unavailable; a previously persisted client ID may still work.
//!
//! This module never requests ads, writes files or logs credentials. Pass
//! the app's shared Http handle so proxy changes apply to subsequent calls.
//!
//! Construct [`Client`] with an optional saved public client ID and OAuth
//! token. After requests, read [`Client::client_id`] and persist changes off
//! the UI thread. Recreate the client when the signed-in account changes.
//! Pages retain SoundCloud's opaque `next_href`: use [`Client::next_page`],
//! or [`Client::next_likes`] / [`Client::next_liked_playlists`] for the
//! wrapped like feeds. [`Client::playlist`] and [`Client::resolve`] hydrate
//! playlist stubs in order; search and user playlist pages stay summaries.
//! A missing batch result is an error, so deleted tracks never shift rows.
//! [`StreamInfo::duration_ms`] describes playback; the converted app track
//! keeps the full catalogue duration. Respect [`StreamInfo::is_preview`].
//! Long rate limits return [`Error::RateLimited`] and hold a shared cooldown
//! across clones instead of keeping a runtime task asleep for hours.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use reqwest::{StatusCode, Url, header};
use serde::{Deserialize, de::DeserializeOwned};
use tokio::sync::{Mutex, Semaphore};
use tokio::time::Instant;

use super::models::{Page, Playlist, PlaylistLike, Resolved, Track, TrackLike, Transcoding, User};
pub use super::models::{Protocol, StreamInfo};
use crate::http::Http;

const API: &str = "https://api-v2.soundcloud.com/";
const HOME: &str = "https://soundcloud.com/";
const RATE_LIMIT_RETRIES: u32 = 3;
const MAX_INLINE_WAIT: Duration = Duration::from_secs(30);

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("SoundCloud needs an OAuth token for this request")]
    NotSignedIn,
    #[error("SoundCloud request failed (HTTP {0})")]
    Status(u16),
    #[error("SoundCloud is rate limiting requests; try again after {retry_after:?}")]
    RateLimited { retry_after: Duration },
    #[error("SoundCloud network error: {0}")]
    Network(String),
    #[error("SoundCloud returned an unexpected response")]
    Decode,
    #[error("Unable to discover SoundCloud's public client ID")]
    ClientIdUnavailable,
    #[error("Invalid SoundCloud URL")]
    InvalidUrl,
    #[error("SoundCloud did not return track {0}")]
    MissingTrack(u64),
    #[error("SoundCloud has no supported, unencrypted stream for this track")]
    NoStream,
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Network(error.without_url().to_string())
    }
}

struct Shared {
    http: Http,
    client_id: RwLock<Option<String>>,
    discovery: Mutex<()>,
    cooldown: Mutex<Option<Instant>>,
    limiter: Semaphore,
    api: Url,
    home: Url,
}

/// Clones share discovery, cached client ID, concurrency and rate cooldown.
/// The caller owns token storage and atomic persistence of `client_id()`.
#[derive(Clone)]
pub struct Client {
    shared: Arc<Shared>,
    oauth_token: Option<String>,
}

impl Client {
    pub fn new(
        http: impl Into<Http>,
        client_id: Option<String>,
        oauth_token: Option<String>,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                http: http.into(),
                client_id: RwLock::new(client_id.filter(|id| valid_client_id(id))),
                discovery: Mutex::new(()),
                cooldown: Mutex::new(None),
                limiter: Semaphore::new(4),
                api: Url::parse(API).expect("constant API URL"),
                home: Url::parse(HOME).expect("constant homepage URL"),
            }),
            oauth_token: oauth_token.filter(|token| !token.is_empty()),
        }
    }

    /// Read after any request, including a failed one, to persist a new ID.
    pub fn client_id(&self) -> Option<String> {
        self.shared
            .client_id
            .read()
            .unwrap_or_else(|lock| lock.into_inner())
            .clone()
    }

    pub async fn discover_client_id(&self) -> Result<String> {
        self.ensure_client_id(None).await
    }

    pub async fn search_tracks(&self, query: &str, limit: u32, offset: u32) -> Result<Page<Track>> {
        self.search("search/tracks", query, limit, offset).await
    }

    /// Includes both sets and albums, as returned by SoundCloud.
    pub async fn search_playlists(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Page<Playlist>> {
        self.search("search/playlists", query, limit, offset).await
    }

    pub async fn search_users(&self, query: &str, limit: u32, offset: u32) -> Result<Page<User>> {
        self.search("search/users", query, limit, offset).await
    }

    async fn search<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Page<T>> {
        self.get(
            path,
            &[
                ("q", query.to_owned()),
                ("limit", limit.clamp(1, 200).to_string()),
                ("offset", offset.to_string()),
                ("linked_partitioning", "1".into()),
            ],
        )
        .await
    }

    /// Pass the previous page's next_href unchanged. Its old client_id is
    /// replaced. Only catalogue feed paths on the API origin are accepted,
    /// so a cursor cannot send OAuth elsewhere or request an ad endpoint.
    pub async fn next_page<T: DeserializeOwned>(&self, next_href: &str) -> Result<Page<T>> {
        let url = self.api_url(next_href)?;
        let segments = url
            .path()
            .trim_start_matches('/')
            .split('/')
            .collect::<Vec<_>>();
        let allowed = match segments.as_slice() {
            ["search", "tracks" | "playlists" | "users"] => true,
            [
                "users",
                id,
                "track_likes" | "playlist_likes" | "playlists" | "tracks",
            ]
            | ["users", id, "playlists", "liked_and_owned"] => id.parse::<u64>().is_ok(),
            _ => false,
        };
        if !allowed {
            return Err(Error::InvalidUrl);
        }
        self.get(next_href, &[]).await
    }

    pub async fn track(&self, id: u64) -> Result<Track> {
        self.get(&format!("tracks/{id}"), &[]).await
    }

    /// Ordered like ids, including duplicates. Missing tracks are errors,
    /// never silently dropped, so playlist positions remain trustworthy.
    pub async fn tracks(&self, ids: &[u64]) -> Result<Vec<Track>> {
        let mut found = HashMap::new();
        for chunk in ids.chunks(50) {
            let ids = chunk
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",");
            let tracks: Vec<Track> = self.get("tracks", &[("ids", ids)]).await?;
            found.extend(tracks.into_iter().map(|track| (track.id, track)));
        }
        ids.iter()
            .map(|id| found.get(id).cloned().ok_or(Error::MissingTrack(*id)))
            .collect()
    }

    pub async fn playlist(&self, id: u64) -> Result<Playlist> {
        let mut playlist = self.get(&format!("playlists/{id}"), &[]).await?;
        self.fill_playlist(&mut playlist).await?;
        Ok(playlist)
    }

    /// Also accepts a playlist returned by search or resolve. Already complete
    /// tracks retain their metadata and duplicate entries retain their places.
    pub async fn fill_playlist(&self, playlist: &mut Playlist) -> Result<()> {
        let ids = playlist
            .tracks
            .iter()
            .filter(|track| track.is_stub())
            .map(|track| track.id)
            .collect::<Vec<_>>();
        let filled = self.tracks(&ids).await?;
        fill_stubs(playlist, filled)
    }

    pub async fn user(&self, id: u64) -> Result<User> {
        self.get(&format!("users/{id}"), &[]).await
    }

    pub async fn me(&self) -> Result<User> {
        if self.oauth_token.is_none() {
            return Err(Error::NotSignedIn);
        }
        self.get("me", &[]).await
    }

    pub async fn resolve(&self, url: &str) -> Result<Resolved> {
        let url = Url::parse(url).map_err(|_| Error::InvalidUrl)?;
        if !matches!(url.scheme(), "https" | "http")
            || !matches!(
                url.host_str(),
                Some(
                    "soundcloud.com"
                        | "www.soundcloud.com"
                        | "m.soundcloud.com"
                        | "on.soundcloud.com"
                )
            )
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            return Err(Error::InvalidUrl);
        }
        let mut resolved = self.get("resolve", &[("url", url.to_string())]).await?;
        if let Resolved::Playlist(playlist) = &mut resolved {
            self.fill_playlist(playlist).await?;
        }
        Ok(resolved)
    }

    pub async fn user_likes(&self, id: u64, limit: u32, offset: u32) -> Result<Page<Track>> {
        let page: Page<TrackLike> = self.user_page(id, "track_likes", limit, offset).await?;
        Ok(page.map(|like| like.track))
    }

    pub async fn next_likes(&self, next_href: &str) -> Result<Page<Track>> {
        let page: Page<TrackLike> = self.next_page(next_href).await?;
        Ok(page.map(|like| like.track))
    }

    /// Own playlists and albums. Liked playlists have an independent cursor,
    /// exposed by user_liked_playlists, so neither list loses pagination.
    pub async fn user_playlists(&self, id: u64, limit: u32, offset: u32) -> Result<Page<Playlist>> {
        self.user_page(id, "playlists", limit, offset).await
    }

    pub async fn user_liked_playlists(
        &self,
        id: u64,
        limit: u32,
        offset: u32,
    ) -> Result<Page<Playlist>> {
        let page: Page<PlaylistLike> = self.user_page(id, "playlist_likes", limit, offset).await?;
        Ok(page.map(|like| like.playlist))
    }

    pub async fn next_liked_playlists(&self, next_href: &str) -> Result<Page<Playlist>> {
        let page: Page<PlaylistLike> = self.next_page(next_href).await?;
        Ok(page.map(|like| like.playlist))
    }

    /// SoundCloud's combined own + liked collection requires OAuth, even for
    /// public users. Anonymous callers can page the two public lists above.
    pub async fn user_collection_playlists(
        &self,
        id: u64,
        limit: u32,
        offset: u32,
    ) -> Result<Page<Playlist>> {
        if self.oauth_token.is_none() {
            return Err(Error::NotSignedIn);
        }
        self.user_page(id, "playlists/liked_and_owned", limit, offset)
            .await
    }

    pub async fn user_tracks(&self, id: u64, limit: u32, offset: u32) -> Result<Page<Track>> {
        self.user_page(id, "tracks", limit, offset).await
    }

    async fn user_page<T: DeserializeOwned>(
        &self,
        id: u64,
        kind: &str,
        limit: u32,
        offset: u32,
    ) -> Result<Page<T>> {
        self.get(
            &format!("users/{id}/{kind}"),
            &[
                ("limit", limit.clamp(1, 200).to_string()),
                ("offset", offset.to_string()),
                ("linked_partitioning", "1".into()),
            ],
        )
        .await
    }

    pub async fn resolve_stream(&self, track: &Track) -> Result<StreamInfo> {
        if !track.is_playable() {
            return Err(Error::NoStream);
        }
        let mut choices = track
            .media
            .as_ref()
            .into_iter()
            .flat_map(|media| &media.transcodings)
            .filter(|item| item.preference().is_some())
            .collect::<Vec<_>>();
        choices.sort_by_key(|item| item.preference());
        for transcoding in choices {
            if !self
                .api_url(&transcoding.url)?
                .path()
                .starts_with("/media/")
            {
                return Err(Error::InvalidUrl);
            }
            let query = track
                .track_authorization
                .as_ref()
                .map(|auth| vec![("track_authorization", auth.clone())])
                .unwrap_or_default();
            let response: StreamResponse = match self.get(&transcoding.url, &query).await {
                // Old MP3 endpoints may still be advertised after removal.
                Err(Error::Status(404 | 410)) => continue,
                other => other?,
            };
            let url = Url::parse(&response.url).map_err(|_| Error::Decode)?;
            if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
                return Err(Error::InvalidUrl);
            }
            return Ok(stream_info(track, transcoding, response.url));
        }
        Err(Error::NoStream)
    }

    async fn ensure_client_id(&self, rejected: Option<&str>) -> Result<String> {
        let _discovery = self.shared.discovery.lock().await;
        if let Some(id) = self.client_id()
            && Some(id.as_str()) != rejected
        {
            return Ok(id);
        }
        *self
            .shared
            .client_id
            .write()
            .unwrap_or_else(|lock| lock.into_inner()) = None;
        let html = self
            .send(self.shared.home.clone(), false)
            .await?
            .text()
            .await?;
        let assets = script_assets(&html, &self.shared.home);
        for asset in assets.into_iter().rev() {
            let script = match self.send(asset, false).await {
                Ok(response) => match response.text().await {
                    Ok(script) => script,
                    Err(_) => continue,
                },
                Err(Error::Status(_)) | Err(Error::Network(_)) => continue,
                Err(error) => return Err(error),
            };
            if let Some(id) = extract_client_id(&script) {
                *self
                    .shared
                    .client_id
                    .write()
                    .unwrap_or_else(|lock| lock.into_inner()) = Some(id.clone());
                return Ok(id);
            }
        }
        Err(Error::ClientIdUnavailable)
    }

    fn api_url(&self, path: &str) -> Result<Url> {
        let url = self.shared.api.join(path).map_err(|_| Error::InvalidUrl)?;
        if url.origin() != self.shared.api.origin()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::InvalidUrl);
        }
        Ok(url)
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let mut url = self.api_url(path)?;
        let retained = url
            .query_pairs()
            .filter(|(key, _)| key != "client_id" && key != "oauth_token" && key != "access_token")
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        url.set_query(None);
        url.query_pairs_mut()
            .extend_pairs(retained)
            .extend_pairs(query.iter().map(|(key, value)| (*key, value)));
        let mut id = self.ensure_client_id(None).await?;
        for attempt in 0..2 {
            let mut authenticated = url.clone();
            authenticated
                .query_pairs_mut()
                .append_pair("client_id", &id);
            match self.send(authenticated, true).await {
                Err(Error::Status(401 | 403)) if attempt == 0 => {
                    id = self.ensure_client_id(Some(&id)).await?;
                }
                response => return response?.json().await.map_err(|_| Error::Decode),
            }
        }
        unreachable!("the second attempt always returns")
    }

    async fn send(&self, url: Url, oauth: bool) -> Result<reqwest::Response> {
        for attempt in 0..=RATE_LIMIT_RETRIES {
            let permit = self
                .shared
                .limiter
                .acquire()
                .await
                .map_err(|_| Error::ClientIdUnavailable)?;
            loop {
                let deadline = *self.shared.cooldown.lock().await;
                let remaining = deadline
                    .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                    .unwrap_or_default();
                if remaining > MAX_INLINE_WAIT {
                    return Err(Error::RateLimited {
                        retry_after: remaining,
                    });
                }
                if remaining.is_zero() {
                    break;
                }
                tokio::time::sleep(remaining).await;
            }
            let mut request = self
                .shared
                .http
                .client()
                .map_err(Error::Network)?
                .get(url.clone())
                .timeout(Duration::from_secs(30));
            if oauth && let Some(token) = &self.oauth_token {
                let mut value = header::HeaderValue::from_str(&format!("OAuth {token}"))
                    .map_err(|_| Error::NotSignedIn)?;
                value.set_sensitive(true);
                request = request.header(header::AUTHORIZATION, value);
            }
            let response = request.send().await?;
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                let delay = retry_after(response.headers(), attempt);
                let deadline = Instant::now()
                    .checked_add(delay)
                    .ok_or(Error::RateLimited { retry_after: delay })?;
                let mut cooldown = self.shared.cooldown.lock().await;
                *cooldown = Some(cooldown.map_or(deadline, |old| old.max(deadline)));
                drop(cooldown);
                drop(permit);
                if attempt == RATE_LIMIT_RETRIES || delay > MAX_INLINE_WAIT {
                    return Err(Error::RateLimited { retry_after: delay });
                }
                continue;
            }
            if !response.status().is_success() {
                return Err(Error::Status(response.status().as_u16()));
            }
            return Ok(response);
        }
        unreachable!("the last attempt always returns")
    }
}

#[derive(Deserialize)]
struct StreamResponse {
    url: String,
}

fn fill_stubs(playlist: &mut Playlist, tracks: Vec<Track>) -> Result<()> {
    let found = tracks
        .into_iter()
        .map(|track| (track.id, track))
        .collect::<HashMap<_, _>>();
    // Validate first, so a missing/deleted track never leaves a partially
    // filled playlist in the caller's state.
    for track in playlist.tracks.iter().filter(|track| track.is_stub()) {
        if found.get(&track.id).is_none_or(Track::is_stub) {
            return Err(Error::MissingTrack(track.id));
        }
    }
    for track in &mut playlist.tracks {
        if track.is_stub()
            && let Some(full) = found.get(&track.id)
        {
            *track = full.clone();
        }
    }
    Ok(())
}

fn stream_info(track: &Track, item: &Transcoding, url: String) -> StreamInfo {
    let mut duration = if item.duration == 0 {
        track.duration
    } else {
        item.duration
    };
    let parsed = Url::parse(&url).ok();
    let path = parsed.as_ref().map_or("", Url::path);
    let thirty_second_preview = path.contains("/preview/0/30/") || path.contains("/playlist/0/30/");
    if thirty_second_preview && (duration == 0 || duration > 30_000) {
        duration = 30_000;
    }
    StreamInfo {
        is_preview: track.is_preview()
            || item.snipped
            || item.url.contains("/preview/")
            || path.contains("/preview/")
            || thirty_second_preview
            || super::models::shortened(duration, track.full_duration.unwrap_or(track.duration)),
        url,
        protocol: item.protocol().expect("selected supported protocol"),
        mime: item.format.mime_type.clone(),
        codec: item.codec().expect("selected supported codec").to_owned(),
        duration_ms: duration,
    }
}

fn valid_client_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn extract_client_id(script: &str) -> Option<String> {
    for (position, _) in script.match_indices("client_id") {
        if position > 0
            && (script.as_bytes()[position - 1].is_ascii_alphanumeric()
                || matches!(script.as_bytes()[position - 1], b'_' | b'$'))
        {
            continue;
        }
        let rest = script[position + "client_id".len()..].trim_start();
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('"') else {
            continue;
        };
        let Some((id, _)) = rest.split_once('"') else {
            continue;
        };
        if valid_client_id(id) {
            return Some(id.to_owned());
        }
    }
    None
}

fn script_assets(html: &str, home: &Url) -> Vec<Url> {
    html.split("<script")
        .skip(1)
        .filter_map(|tail| {
            let tag = tail.split_once('>')?.0;
            let attrs = html_attributes(tag);
            let src = &attrs
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case("src"))?
                .1;
            let url = home.join(&src.replace("&amp;", "&")).ok()?;
            let host = url.host_str()?;
            (url.scheme() == home.scheme()
                && url.username().is_empty()
                && url.password().is_none()
                && (url.origin() == home.origin()
                    || host == "soundcloud.com"
                    || host.ends_with(".soundcloud.com")
                    || host == "sndcdn.com"
                    || host.ends_with(".sndcdn.com")))
            .then_some(url)
        })
        .collect()
}

fn html_attributes(mut tag: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    while !tag.trim_start().is_empty() {
        tag = tag.trim_start();
        let end = tag
            .find(|ch: char| ch.is_ascii_whitespace() || ch == '=')
            .unwrap_or(tag.len());
        let key = tag[..end].to_owned();
        tag = tag[end..].trim_start();
        let mut value = String::new();
        if let Some(rest) = tag.strip_prefix('=') {
            tag = rest.trim_start();
            if let Some(quote) = tag.chars().next().filter(|ch| matches!(ch, '\'' | '"')) {
                tag = &tag[1..];
                let end = tag.find(quote).unwrap_or(tag.len());
                value = tag[..end].to_owned();
                tag = tag.get(end + 1..).unwrap_or_default();
            } else {
                let end = tag.find(char::is_whitespace).unwrap_or(tag.len());
                value = tag[..end].to_owned();
                tag = &tag[end..];
            }
        }
        attrs.push((key, value));
    }
    attrs
}

fn retry_after(headers: &header::HeaderMap, attempt: u32) -> Duration {
    let value = headers
        .get(header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok());
    if let Some(seconds) = value.and_then(|value| value.parse::<u64>().ok()) {
        return Duration::from_secs(seconds);
    }
    if let Some(date) = value
        .and_then(|value| jiff::fmt::strtime::parse("%a, %d %b %Y %H:%M:%S GMT", value).ok())
        .and_then(|parsed| parsed.to_datetime().ok())
        .and_then(|date| date.to_zoned(jiff::tz::TimeZone::UTC).ok())
    {
        let seconds = date
            .timestamp()
            .as_second()
            .saturating_sub(jiff::Timestamp::now().as_second());
        return Duration::from_secs(seconds.max(0) as u64);
    }
    Duration::from_secs(1 << attempt)
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
