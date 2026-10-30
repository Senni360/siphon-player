use std::time::Duration;

use reqwest::{Method, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::api::models::PlayableItem;
use crate::http::Http;

/// Deployment owner replaces this, or supplies SIPHON_FRIENDS_URL including /api.
pub const DEFAULT_BASE_URL: &str = "https://siphon-friends.example.invalid/api";

// No Debug: this is the one-time bearer credential, bound to its origin.
#[derive(Clone, Serialize, Deserialize)]
pub struct Identity {
    pub user_id: String,
    pub secret: String,
    pub base_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub user_id: String,
    pub display_name: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Spotify,
    Soundcloud,
    Youtube,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NewPost {
    pub url: String,
    pub source: Source,
    pub title: String,
    pub artists: Vec<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: u32,
    #[serde(default)]
    pub note: String,
}

impl NewPost {
    /// Preserve SoundCloud's complete external URL, including private s- paths
    /// and query tokens. A numeric SoundCloud URI alone is not a share link.
    pub fn from_item(item: &PlayableItem) -> Result<Self, String> {
        let PlayableItem::Track(track) = item else {
            return Err("Only tracks, albums and playlists can be shared.".into());
        };
        let uri = item.uri();
        let (source, url) = if let Some(id) = uri.strip_prefix("youtube:video:") {
            if id.len() != 11
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            {
                return Err("Invalid YouTube video.".into());
            }
            (
                Source::Youtube,
                format!("https://www.youtube.com/watch?v={id}"),
            )
        } else if uri.starts_with("soundcloud:") {
            let url = track
                .external_urls
                .spotify
                .as_deref()
                .and_then(crate::link::soundcloud_url)
                .ok_or("This SoundCloud item has no share link.")?;
            (Source::Soundcloud, url)
        } else if let Some(rest) = uri.strip_prefix("spotify:") {
            let (kind, id) = rest.split_once(':').ok_or("Invalid Spotify item.")?;
            if !matches!(kind, "track" | "album" | "playlist")
                || id.is_empty()
                || !id.bytes().all(|b| b.is_ascii_alphanumeric())
            {
                return Err("This Spotify item cannot be shared.".into());
            }
            (
                Source::Spotify,
                format!("https://open.spotify.com/{kind}/{id}"),
            )
        } else {
            return Err("This item has no supported share link.".into());
        };
        Ok(Self {
            url,
            source,
            title: track.name.clone(),
            artists: track.artists.iter().map(|a| a.name.clone()).collect(),
            artwork_url: item.image(300).map(str::to_owned),
            duration_ms: item.duration_ms(),
            note: String::new(),
        })
    }

    /// Albums, sets and playlists can be shared by their existing share URL.
    pub fn from_url(url: String, source: Source, title: String) -> Self {
        Self {
            url,
            source,
            title,
            artists: Vec::new(),
            artwork_url: None,
            duration_ms: 0,
            note: String::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Post {
    pub id: String,
    pub author_id: String,
    pub author_name: String,
    pub created_at: u64,
    pub like_count: u32,
    pub liked: bool,
    #[serde(flatten)]
    pub item: NewPost,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Feed {
    pub posts: Vec<Post>,
    pub next_cursor: Option<String>,
}

#[derive(Clone)]
pub struct Client {
    http: Http,
    base: Url,
}

impl Client {
    pub fn configured(http: Http) -> Result<Self, String> {
        Self::new(
            http,
            &std::env::var("SIPHON_FRIENDS_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.into()),
        )
    }

    pub fn new(http: Http, base: &str) -> Result<Self, String> {
        let base =
            Url::parse(base.trim_end_matches('/')).map_err(|_| "Invalid Friends service URL.")?;
        let local = matches!(base.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if !(base.scheme() == "https" || local && base.scheme() == "http")
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err("Friends requires HTTPS, or a local development server.".into());
        }
        Ok(Self { http, base })
    }

    pub fn base_url(&self) -> &str {
        self.base.as_str()
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        identity: Option<&Identity>,
        body: Option<serde_json::Value>,
        cursor: Option<&str>,
    ) -> Result<T, String> {
        let mut url = self.base.clone();
        url.set_path(&format!("{}{path}", self.base.path()));
        if let Some(cursor) = cursor {
            url.query_pairs_mut().append_pair("cursor", cursor);
        }
        let mut request = self
            .http
            .client()?
            .request(method, url)
            .timeout(Duration::from_secs(15));
        if let Some(identity) = identity {
            if identity.base_url != self.base_url() {
                return Err("This Friends identity belongs to another service.".into());
            }
            request = request.bearer_auth(&identity.secret);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| "Friends could not be reached. Try again.".to_owned())?;
        let status = response.status();
        if !status.is_success() {
            // Never display arbitrary server errors, URLs, or authorization responses.
            return Err(match status.as_u16() {
                401 => "Friends sign-in is no longer valid. Leave and join again.",
                403 => "The invite code is incorrect, or this post belongs to someone else.",
                429 => "Friends is receiving too many requests. Try again later.",
                400 | 413 | 415 => {
                    "Check the name, link and note. The service rejected this request."
                }
                _ => "Friends is unavailable. Try again later.",
            }
            .into());
        }
        if response
            .content_length()
            .is_some_and(|size| size > 2 * 1024 * 1024)
        {
            return Err("Friends returned too much data.".into());
        }
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Friends response was interrupted.")?
        {
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err("Friends returned too much data.".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| "Friends returned an invalid response.".into())
    }

    pub async fn join(&self, invite_code: &str, display_name: &str) -> Result<Identity, String> {
        #[derive(Deserialize)]
        struct Joined {
            user_id: String,
            secret: String,
        }
        let joined: Joined = self
            .request(
                Method::POST,
                "/join",
                None,
                Some(serde_json::json!({"invite_code":invite_code,"display_name":display_name})),
                None,
            )
            .await?;
        if joined.user_id.is_empty()
            || joined.secret.len() != 64
            || !joined.secret.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("Friends returned an invalid identity.".into());
        }
        Ok(Identity {
            user_id: joined.user_id,
            secret: joined.secret,
            base_url: self.base_url().into(),
        })
    }
    pub async fn profile(&self, id: &Identity) -> Result<Profile, String> {
        self.request(Method::GET, "/me", Some(id), None, None).await
    }
    pub async fn rename(&self, id: &Identity, name: &str) -> Result<Profile, String> {
        self.request(
            Method::PATCH,
            "/me",
            Some(id),
            Some(serde_json::json!({"display_name": name})),
            None,
        )
        .await
    }
    pub async fn feed(&self, id: &Identity, cursor: Option<&str>) -> Result<Feed, String> {
        self.request(Method::GET, "/posts", Some(id), None, cursor)
            .await
    }
    pub async fn share(&self, id: &Identity, post: &NewPost) -> Result<Post, String> {
        self.request(
            Method::POST,
            "/posts",
            Some(id),
            Some(serde_json::to_value(post).map_err(|_| "Invalid share.")?),
            None,
        )
        .await
    }
    pub async fn like(&self, identity: &Identity, id: &str, liked: bool) -> Result<(), String> {
        self.mutate_post(
            identity,
            id,
            "/like",
            if liked { Method::PUT } else { Method::DELETE },
        )
        .await
    }
    pub async fn delete(&self, identity: &Identity, id: &str) -> Result<(), String> {
        self.mutate_post(identity, id, "", Method::DELETE).await
    }
    async fn mutate_post(
        &self,
        identity: &Identity,
        id: &str,
        suffix: &str,
        method: Method,
    ) -> Result<(), String> {
        if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            return Err("Invalid post id.".into());
        }
        let _: serde_json::Value = self
            .request(
                method,
                &format!("/posts/{id}{suffix}"),
                Some(identity),
                None,
                None,
            )
            .await?;
        Ok(())
    }
}
