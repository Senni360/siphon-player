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

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NewPost {
    pub url: String,
    pub source: Source,
    pub title: String,
    pub artists: Vec<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: u32,
    #[serde(default)]
    pub note: String,
    /// The members a share was sent to. Empty: the whole group.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to: Vec<String>,
}

// Private SoundCloud URLs and their artwork can carry bearer-like tokens.
impl std::fmt::Debug for NewPost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewPost")
            .field("source", &self.source)
            .field("duration_ms", &self.duration_ms)
            .finish_non_exhaustive()
    }
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
            to: Vec::new(),
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
            to: Vec::new(),
        }
    }
}

/// A message in a share's thread.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reply {
    pub id: String,
    pub author_id: String,
    pub author_name: String,
    pub created_at: u64,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Post {
    pub id: String,
    pub author_id: String,
    pub author_name: String,
    pub created_at: u64,
    pub like_count: u32,
    pub liked: bool,
    #[serde(default)]
    pub replies: Vec<Reply>,
    /// How likely, in percent, the reader is to enjoy this share, as the
    /// service's Jev estimated from their own shares and hearts. None when
    /// nothing was estimated.
    #[serde(default, with = "percent")]
    pub affinity: Option<u8>,
    #[serde(flatten)]
    pub item: NewPost,
}

/// The service sends a probability from 0 to 1; the player keeps a percent
/// so a post stays comparable for equality.
mod percent {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<u8>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(percent) => serializer.serialize_f32(f32::from(*percent) / 100.0),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u8>, D::Error> {
        Ok(Option::<f64>::deserialize(deserializer)?
            .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
            .map(|p| (p * 100.0).round() as u8))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Feed {
    pub posts: Vec<Post>,
    pub next_cursor: Option<String>,
    /// The group's members, when this read asked for them too.
    #[serde(skip)]
    pub members: Option<Vec<Profile>>,
}

#[derive(Clone)]
pub struct Client {
    http: Http,
    base: Url,
}

impl Client {
    pub fn configured(http: Http) -> Result<Self, String> {
        let base = std::env::var("SIPHON_FRIENDS_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.into());
        if base == DEFAULT_BASE_URL {
            return Err("The Friends service has not been configured.".into());
        }
        Self::new(http, &base)
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
            || !joined
                .secret
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
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
    pub async fn leave(&self, id: &Identity) -> Result<(), String> {
        let _: serde_json::Value = self
            .request(Method::DELETE, "/me", Some(id), None, None)
            .await?;
        Ok(())
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
    /// Jev's probabilities over `candidates` as what follows `recent`.
    /// None when the service had no opinion.
    pub async fn next(
        &self,
        id: &Identity,
        recent: &[super::pick::Song],
        candidates: &[super::pick::Candidate],
    ) -> Result<Option<std::collections::HashMap<String, f64>>, String> {
        #[derive(Deserialize)]
        struct Ranked {
            probabilities: Option<std::collections::HashMap<String, f64>>,
        }
        let ranked: Ranked = self
            .request(
                Method::POST,
                "/jev/next",
                Some(id),
                Some(serde_json::json!({ "recent": recent, "candidates": candidates })),
                None,
            )
            .await?;
        Ok(ranked.probabilities)
    }
    /// Jev's readings of upload titles as artist and song, in order; None
    /// where no reading was confident.
    pub async fn titles(
        &self,
        id: &Identity,
        uploads: &[crate::retitle::Upload],
    ) -> Result<Vec<Option<crate::retitle::Reading>>, String> {
        #[derive(Deserialize)]
        struct Read {
            readings: Vec<Option<crate::retitle::Reading>>,
        }
        let read: Read = self
            .request(
                Method::POST,
                "/jev/titles",
                Some(id),
                Some(serde_json::json!({ "uploads": uploads })),
                None,
            )
            .await?;
        Ok(read.readings)
    }
    pub async fn members(&self, id: &Identity) -> Result<Vec<Profile>, String> {
        #[derive(Deserialize)]
        struct Members {
            members: Vec<Profile>,
        }
        let members: Members = self
            .request(Method::GET, "/members", Some(id), None, None)
            .await?;
        Ok(members.members)
    }
    pub async fn reply(
        &self,
        identity: &Identity,
        post: &str,
        text: &str,
    ) -> Result<Reply, String> {
        valid_id(post)?;
        self.request(
            Method::POST,
            &format!("/posts/{post}/replies"),
            Some(identity),
            Some(serde_json::json!({ "text": text })),
            None,
        )
        .await
    }
    pub async fn delete_reply(
        &self,
        identity: &Identity,
        post: &str,
        reply: &str,
    ) -> Result<(), String> {
        valid_id(reply)?;
        self.mutate_post(identity, post, &format!("/replies/{reply}"), Method::DELETE)
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
        valid_id(id)?;
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

/// Post and reply ids are the service's UUIDs; anything else never reaches
/// a request path.
fn valid_id(id: &str) -> Result<(), String> {
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err("Invalid post id.".into());
    }
    Ok(())
}
