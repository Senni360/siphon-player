//! Async SoundCloud web API v2 reads. Use the backend Tokio runtime.
//!
//! Transcodings checked on 2026-10-28: SoundCloud announced AAC HLS at
//! 160/96 kbit/s and retirement of public API MP3/Opus by December 2025:
//! <https://developers.soundcloud.com/blog/api-streaming-urls/>
//! The web API is a separate surface: yt-dlp still handles legacy MP3,
//! AAC HLS and Opus, and skips CTR/CBC encrypted formats as DRM:
//! <https://github.com/yt-dlp/yt-dlp/blob/master/yt_dlp/extractor/soundcloud.py>
//! <https://github.com/yt-dlp/yt-dlp/issues/17335>
//! The 2026-10-28 anonymous fixtures include AAC 96k and an adaptive manifest.
//! Prefer progressive MP3 when actually offered, then AAC HLS (160k first),
//! then HLS MP3. Opus has no decoder in this build. Never resolve encrypted
//! or adaptive transcodings.
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
use std::sync::atomic::{AtomicBool, Ordering};
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
    #[error(
        "Your SoundCloud session expired. Reconnect SoundCloud with a current oauth_token cookie."
    )]
    SessionExpired,
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

impl super::account::Resolver for Client {
    async fn resolve_profile(
        &self,
        input: &super::account::ProfileInput,
    ) -> std::result::Result<super::account::Profile, super::account::ResolveError> {
        // Profile mode is public, even when another session was connected.
        let anonymous = Self {
            shared: Arc::clone(&self.shared),
            oauth_token: None,
            access: Arc::default(),
        };
        let url = match input {
            super::account::ProfileInput::Permalink(name) => {
                format!("https://soundcloud.com/{name}")
            }
            super::account::ProfileInput::ShortLink(url) => url.clone(),
        };
        match anonymous.resolve(&url).await.map_err(account_error)? {
            Resolved::User(user) => Ok(account_profile(user)),
            _ => Err(super::account::ResolveError::NotFound),
        }
    }

    async fn me(
        &self,
        token: &super::account::SessionToken,
    ) -> std::result::Result<super::account::Profile, super::account::ResolveError> {
        let authenticated = Self {
            shared: Arc::clone(&self.shared),
            oauth_token: Some(token.expose().to_owned()),
            access: Arc::default(),
        };
        authenticated
            .me()
            .await
            .map(account_profile)
            .map_err(account_error)
    }
}

fn account_profile(user: User) -> super::account::Profile {
    super::account::Profile {
        user_id: user.id,
        username: user.username,
        permalink: user
            .permalink_url
            .as_deref()
            .and_then(|url| url.trim_end_matches('/').rsplit('/').next())
            .unwrap_or_default()
            .to_owned(),
        avatar: user.avatar_url,
    }
}

fn account_error(error: Error) -> super::account::ResolveError {
    match error {
        Error::Status(401 | 403) | Error::NotSignedIn | Error::SessionExpired => {
            super::account::ResolveError::Unauthorized
        }
        Error::Status(404) => super::account::ResolveError::NotFound,
        _ => super::account::ResolveError::Unavailable,
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
    access: Arc<Access>,
}

#[derive(Default)]
struct Access {
    expired: AtomicBool,
    revoked: AtomicBool,
    // Private share tokens stay in memory, scoped to this connection.
    secrets: RwLock<HashMap<String, String>>,
    on_expired: Option<Box<dyn Fn() + Send + Sync>>,
}

impl Client {
    /// Stop old account-scoped clones before installing another connection.
    pub(crate) fn revoke(&self) {
        self.access.revoked.store(true, Ordering::SeqCst);
        self.access
            .secrets
            .write()
            .unwrap_or_else(|lock| lock.into_inner())
            .clear();
    }

    pub fn on_session_expired(mut self, notify: impl Fn() + Send + Sync + 'static) -> Self {
        Arc::get_mut(&mut self.access)
            .expect("new connection")
            .on_expired = Some(Box::new(notify));
        self
    }

    fn expire_session(&self) {
        if !self.access.expired.swap(true, Ordering::SeqCst) {
            self.access
                .secrets
                .write()
                .unwrap_or_else(|lock| lock.into_inner())
                .clear();
            if let Some(notify) = &self.access.on_expired {
                notify();
            }
        }
    }

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
            access: Arc::default(),
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

    /// Public browsing shares discovery and cooldown without account access.
    pub(crate) fn anonymous(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            oauth_token: None,
            access: Arc::default(),
        }
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
                "track_likes" | "playlist_likes" | "playlists" | "tracks" | "followings",
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
        let mut playlist = self.playlist_snapshot(id).await?;
        self.fill_playlist(&mut playlist).await?;
        Ok(playlist)
    }

    /// Initial set membership, including stubs, for optimistic local copies.
    pub async fn playlist_snapshot(&self, id: u64) -> Result<Playlist> {
        self.get(&format!("playlists/{id}"), &[]).await
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
        let mut batch = Vec::new();
        let mut filled = Vec::new();
        for id in ids {
            let has_secret = self
                .access
                .secrets
                .read()
                .unwrap_or_else(|lock| lock.into_inner())
                .contains_key(&super::track_uri(id));
            if has_secret {
                filled.push(self.track(id).await?);
            } else {
                batch.push(id);
            }
        }
        filled.extend(self.tracks(&batch).await?);
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
        match &mut resolved {
            Resolved::Track(track) => {
                // The API's canonical permalink can omit a private share
                // token. Preserve the accepted input for copy/share and saved
                // metadata, including short links that grant private access.
                track.permalink_url = Some(url.to_string());
            }
            Resolved::Playlist(playlist) => {
                playlist.permalink_url = Some(url.to_string());
                self.fill_playlist(playlist).await?;
            }
            Resolved::User(_) => {}
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

    pub async fn user_followings(&self, id: u64, limit: u32, offset: u32) -> Result<Page<User>> {
        self.user_page(id, "followings", limit, offset).await
    }

    /// The web app's Feed, not its activities/notification inbox.
    /// Cursor validation stays specific to this session-only endpoint.
    pub async fn following_feed(&self, next: Option<&str>) -> Result<Page<super::following::Item>> {
        if self.oauth_token.is_none() {
            return Err(Error::NotSignedIn);
        }
        let path = self.social_cursor(next.unwrap_or("stream"), "/stream")?;
        let query = if next.is_none() {
            vec![("limit", "50".into()), ("linked_partitioning", "1".into())]
        } else {
            Vec::new()
        };
        let page: Page<super::following::WireItem> = self.get(&path, &query).await?;
        let next_href = page
            .next_href
            .map(|cursor| self.social_cursor(&cursor, "/stream"))
            .transpose()?;
        if next.is_some() && next_href.as_deref() == Some(path.as_str()) {
            return Err(Error::Decode);
        }
        Ok(Page {
            collection: page
                .collection
                .into_iter()
                .filter_map(super::following::WireItem::into_item)
                .collect(),
            next_href,
            total_results: page.total_results,
        })
    }

    /// One explicit mutation. The caller supplies the configured no-redirect,
    /// no-retry collection client so a credential cannot follow a redirect.
    pub async fn set_liked(
        &self,
        http: &reqwest::Client,
        user: u64,
        track: u64,
        liked: bool,
    ) -> Result<()> {
        self.set_membership(http, &format!("users/{user}/track_likes/{track}"), liked)
            .await
    }

    pub async fn set_reposted(
        &self,
        http: &reqwest::Client,
        target: super::reposts::Target,
        reposted: bool,
    ) -> Result<()> {
        self.set_membership(
            http,
            &format!("{}/{}", target.path(), target.id()),
            reposted,
        )
        .await
    }

    pub async fn reposted_ids(&self) -> Result<super::reposts::Snapshot> {
        if self.oauth_token.is_none() {
            return Err(Error::NotSignedIn);
        }
        let tracks = self.repost_ids("me/track_reposts/ids").await?;
        let playlists = self.repost_ids("me/playlist_reposts/ids").await?;
        Ok(super::reposts::Snapshot {
            tracks,
            playlists,
            loaded: true,
        })
    }

    async fn repost_ids(&self, path: &str) -> Result<std::collections::HashSet<u64>> {
        let mut page: Page<u64> = self.get(path, &[("limit", "200".into())]).await?;
        let mut ids = std::collections::HashSet::new();
        let mut seen = std::collections::HashSet::new();
        loop {
            ids.extend(page.collection);
            let Some(next) = page.next_href else { break };
            let next = self.social_cursor(&next, &format!("/{path}"))?;
            if !seen.insert(next.clone()) {
                return Err(Error::InvalidUrl);
            }
            page = self.get(&next, &[("limit", "200".into())]).await?;
        }
        Ok(ids)
    }

    async fn set_membership(
        &self,
        http: &reqwest::Client,
        path: &str,
        enabled: bool,
    ) -> Result<()> {
        let token = self.oauth_token.as_ref().ok_or(Error::NotSignedIn)?;
        if self.access.expired.load(Ordering::SeqCst) || self.access.revoked.load(Ordering::SeqCst)
        {
            return Err(Error::SessionExpired);
        }
        let mut url = self.api_url(path)?;
        let client_id = self.ensure_client_id(None).await?;
        url.query_pairs_mut().append_pair("client_id", &client_id);
        let mut authorization = header::HeaderValue::from_str(&format!("OAuth {token}"))
            .map_err(|_| Error::NotSignedIn)?;
        authorization.set_sensitive(true);
        let permit = self
            .shared
            .limiter
            .acquire()
            .await
            .map_err(|_| Error::ClientIdUnavailable)?;
        let remaining = self
            .shared
            .cooldown
            .lock()
            .await
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_default();
        if !remaining.is_zero() {
            return Err(Error::RateLimited {
                retry_after: remaining,
            });
        }
        if self.access.expired.load(Ordering::SeqCst) || self.access.revoked.load(Ordering::SeqCst)
        {
            return Err(Error::SessionExpired);
        }
        let method = if enabled {
            reqwest::Method::PUT
        } else {
            reqwest::Method::DELETE
        };
        let response = http
            .request(method, url)
            .header(header::AUTHORIZATION, authorization)
            .send()
            .await?;
        // Session verification needs its own request slot. Release this one
        // first, even when other like writes occupy the remaining slots.
        drop(permit);
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let delay = retry_after(response.headers(), 0);
            *self.shared.cooldown.lock().await = Instant::now().checked_add(delay);
            Err(Error::RateLimited { retry_after: delay })
        } else if matches!(response.status().as_u16(), 401 | 403) {
            // A forbidden track need not mean an expired account.
            if matches!(self.me().await, Err(Error::SessionExpired)) {
                Err(Error::SessionExpired)
            } else {
                Err(Error::Status(response.status().as_u16()))
            }
        } else if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::Status(response.status().as_u16()))
        }
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
        log::debug!(
            "SoundCloud stream: track_id={} supported_transcodings={}",
            track.id,
            choices.len()
        );
        for transcoding in choices {
            if !self
                .api_url(&transcoding.url)?
                .path()
                .starts_with("/media/")
            {
                return Err(Error::InvalidUrl);
            }
            let mut query = track
                .track_authorization
                .as_ref()
                .map(|auth| vec![("track_authorization", auth.clone())])
                .unwrap_or_default();
            if let Some(secret) = self
                .access
                .secrets
                .read()
                .unwrap_or_else(|lock| lock.into_inner())
                .get(&super::track_uri(track.id))
                .cloned()
            {
                query.push(("secret_token", secret));
            }
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
        log::debug!(
            "SoundCloud client discovery: public_assets={}",
            assets.len()
        );
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
                log::info!("SoundCloud client discovery: public client ID refreshed");
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

    // Social cursors can be persisted in account-scoped caches. Keep only the
    // expected API endpoint and pagination data, never credentials returned in
    // a next_href. Resolving relative URLs also makes cursor-loop checks stable.
    fn social_cursor(&self, path: &str, endpoint: &str) -> Result<String> {
        let mut url = self.api_url(path)?;
        if url.path() != endpoint {
            return Err(Error::InvalidUrl);
        }
        let query = url
            .query_pairs()
            .filter(|(key, _)| {
                !matches!(
                    key.as_ref(),
                    "client_id"
                        | "oauth_token"
                        | "access_token"
                        | "secret_token"
                        | "track_authorization"
                )
            })
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        url.set_query(None);
        url.set_fragment(None);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url.to_string())
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        if self.access.revoked.load(Ordering::SeqCst) {
            return Err(Error::NotSignedIn);
        }
        let mut url = self.api_url(path)?;
        let requires_session = url.path() == "/me"
            || url.path().starts_with("/me/")
            || url.path() == "/stream"
            || url.path().ends_with("/liked_and_owned");
        if requires_session && self.oauth_token.is_none() {
            return Err(Error::NotSignedIn);
        }
        if requires_session && self.access.expired.load(Ordering::SeqCst) {
            return Err(Error::SessionExpired);
        }
        let retained = url
            .query_pairs()
            .filter(|(key, _)| {
                key != "client_id"
                    && key != "oauth_token"
                    && key != "access_token"
                    && !query.iter().any(|(name, _)| key.as_ref() == *name)
            })
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();
        url.set_query(None);
        url.query_pairs_mut()
            .extend_pairs(retained)
            .extend_pairs(query.iter().map(|(key, value)| (*key, value)));
        let parts = url
            .path()
            .trim_start_matches('/')
            .split('/')
            .collect::<Vec<_>>();
        if let [kind @ ("tracks" | "playlists"), id] = parts.as_slice() {
            let key = format!(
                "soundcloud:{}:{id}",
                if *kind == "tracks" {
                    "track"
                } else {
                    "playlist"
                }
            );
            if let Some(secret) = self
                .access
                .secrets
                .read()
                .unwrap_or_else(|lock| lock.into_inner())
                .get(&key)
            {
                url.query_pairs_mut().append_pair("secret_token", secret);
            }
        }
        let mut id = self.ensure_client_id(None).await?;
        for attempt in 0..2 {
            if requires_session && self.access.expired.load(Ordering::SeqCst) {
                return Err(Error::SessionExpired);
            }
            let mut authenticated = url.clone();
            authenticated
                .query_pairs_mut()
                .append_pair("client_id", &id);
            // A session-only request must never become anonymous if another
            // request expires the account between this check and dispatch.
            let oauth = self.oauth_token.is_some()
                && (requires_session || !self.access.expired.load(Ordering::SeqCst));
            let response = match self.send(authenticated, oauth).await {
                Err(error @ Error::Status(401 | 403)) if oauth => {
                    // A rejected public client ID can look like a rejected
                    // session. Preserve the ordinary one-time discovery retry.
                    if attempt == 0 {
                        if let Ok(fresh) = self.ensure_client_id(Some(&id)).await {
                            id = fresh;
                            continue;
                        }
                        // Discovery can itself be blocked. Keep the last ID
                        // while checking the session and falling back publicly.
                        self.shared
                            .client_id
                            .write()
                            .unwrap_or_else(|lock| lock.into_inner())
                            .get_or_insert_with(|| id.clone());
                    }
                    let mut me = self.api_url("me")?;
                    me.query_pairs_mut().append_pair("client_id", &id);
                    if url.path() == "/me"
                        || matches!(self.send(me, true).await, Err(Error::Status(401 | 403)))
                    {
                        self.expire_session();
                        if requires_session {
                            return Err(Error::SessionExpired);
                        }
                        // Public content remains available after a session expires.
                        let mut public = url.clone();
                        let pairs = public
                            .query_pairs()
                            .filter(|(key, _)| key != "secret_token")
                            .map(|(key, value)| (key.into_owned(), value.into_owned()))
                            .collect::<Vec<_>>();
                        public.set_query(None);
                        public
                            .query_pairs_mut()
                            .extend_pairs(pairs)
                            .append_pair("client_id", &id);
                        self.send(public, false).await
                    } else {
                        return Err(error);
                    }
                }
                Err(Error::Status(401 | 403)) if attempt == 0 => {
                    id = self.ensure_client_id(Some(&id)).await?;
                    continue;
                }
                response => response,
            }?;
            let value: serde_json::Value = response.json().await.map_err(|_| Error::Decode)?;
            {
                let mut secrets = self
                    .access
                    .secrets
                    .write()
                    .unwrap_or_else(|lock| lock.into_inner());
                let hint = if url.path().starts_with("/tracks") || url.path().ends_with("/tracks") {
                    Some("track")
                } else if url.path().starts_with("/playlists")
                    || url.path().ends_with("/playlists")
                    || url.path().ends_with("/liked_and_owned")
                {
                    Some("playlist")
                } else {
                    None
                };
                if !self.access.expired.load(Ordering::SeqCst)
                    && !self.access.revoked.load(Ordering::SeqCst)
                {
                    remember_secrets(&value, hint, &mut secrets);
                }
            }
            return serde_json::from_value(value).map_err(|_| Error::Decode);
        }
        unreachable!("the second attempt always returns")
    }

    async fn send(&self, url: Url, oauth: bool) -> Result<reqwest::Response> {
        for attempt in 0..=RATE_LIMIT_RETRIES {
            if self.access.revoked.load(Ordering::SeqCst) {
                return Err(Error::NotSignedIn);
            }
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
            if self.access.revoked.load(Ordering::SeqCst) {
                return Err(Error::NotSignedIn);
            }
            if oauth && self.access.expired.load(Ordering::SeqCst) {
                return Err(Error::SessionExpired);
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

fn remember_secrets(
    value: &serde_json::Value,
    hint: Option<&str>,
    secrets: &mut HashMap<String, String>,
) {
    match value {
        serde_json::Value::Object(object) => {
            let kind = object
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .or_else(|| object.contains_key("tracks").then_some("playlist"))
                .or(hint);
            if let (Some(kind @ ("track" | "playlist")), Some(id), Some(secret)) = (
                kind,
                object.get("id").and_then(serde_json::Value::as_u64),
                object
                    .get("secret_token")
                    .and_then(serde_json::Value::as_str),
            ) && secrets.len() < 20_000
            {
                secrets.insert(format!("soundcloud:{kind}:{id}"), secret.to_owned());
            }
            for (key, value) in object {
                let hint = match key.as_str() {
                    "track" | "tracks" => Some("track"),
                    "playlist" | "playlists" => Some("playlist"),
                    "collection" => kind,
                    _ => None,
                };
                remember_secrets(value, hint, secrets);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                remember_secrets(value, hint, secrets);
            }
        }
        _ => {}
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
