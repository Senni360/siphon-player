//! An optional Google sign-in, through the listener's own Google Cloud project,
//! that syncs subscriptions and likes with their real YouTube account. Browsing
//! and playing never use it. Runs on the backend runtime, never the UI thread.
//!
//! Nothing here has a `Debug` implementation over a secret, and no response body
//! reaches an error message, so credentials cannot end up in a log or a toast.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::browse::{self, valid_channel};
use super::likes::Channel;
use super::{Thumbnail, Video};
use crate::api::models::Track;

const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const API: &str = "https://www.googleapis.com/youtube/v3";
/// Read and change this account's subscriptions and ratings. YouTube has no
/// narrower scope that allows both.
const SCOPE: &str = "https://www.googleapis.com/auth/youtube";
/// A token is renewed this long before it runs out.
const REFRESH_MARGIN: Duration = Duration::from_secs(90);
/// How far a sync reads. Listing costs one quota unit a page, so these are cheap,
/// and they keep an account with a huge history from filling the library.
const MAX_SUBSCRIPTION_PAGES: u32 = 20;
const MAX_LIKE_PAGES: u32 = 10;
const PAGE_SIZE: &str = "50";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("Google no longer accepts this sign-in. Connect YouTube again.")]
    Revoked,
    #[error(
        "YouTube's daily limit for your Google project is used up. It resets at midnight, Pacific time."
    )]
    Quota,
    #[error("This Google account has no YouTube channel yet. Create one on YouTube first.")]
    NoChannel,
    #[error(
        "YouTube refused that. Check that the YouTube Data API is turned on in your Google project."
    )]
    Refused,
    #[error("That channel can't be subscribed to.")]
    BadChannel,
    #[error("YouTube could not be reached. Check your connection and try again.")]
    Network,
    #[error("YouTube sent an answer Encore couldn't read.")]
    Unreadable,
    #[error("Google did not accept this Client ID and secret. Check them and try again.")]
    BadClient,
}

/// The listener's own OAuth client, from their Google project.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientKeys {
    pub id: String,
    pub secret: String,
}

impl std::fmt::Debug for ClientKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientKeys").finish_non_exhaustive()
    }
}

impl ClientKeys {
    /// Keys as pasted, if they have the shape Google gives them. Whether they
    /// work is for Google to say.
    pub fn parse(id: &str, secret: &str) -> Option<Self> {
        let id = id.trim();
        let secret = secret.trim();
        let plain = |text: &str| !text.is_empty() && !text.chars().any(char::is_whitespace);
        (plain(id) && plain(secret) && id.ends_with(".apps.googleusercontent.com")).then(|| Self {
            id: id.to_owned(),
            secret: secret.to_owned(),
        })
    }
}

/// Who is connected. Not a secret.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub image: Option<String>,
}

/// What the credential store keeps.
#[derive(Serialize, Deserialize)]
pub struct Stored {
    pub keys: ClientKeys,
    pub refresh_token: String,
    pub profile: Profile,
}

impl Stored {
    pub fn encode(&self) -> Option<Vec<u8>> {
        serde_json::to_vec(self).ok()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}

/// A live connection: the stored grant and the current short-lived token.
pub struct Session {
    pub stored: Stored,
    access: Option<(String, Instant)>,
}

impl Session {
    pub fn new(stored: Stored) -> Self {
        Self {
            stored,
            access: None,
        }
    }

    /// A token good for now, renewing it if needed.
    pub async fn access_token(&mut self, http: &reqwest::Client) -> Result<String, Error> {
        if let Some((token, until)) = &self.access
            && Instant::now() + REFRESH_MARGIN < *until
        {
            return Ok(token.clone());
        }
        let tokens = refresh(http, &self.stored.keys, &self.stored.refresh_token).await?;
        let until = Instant::now() + Duration::from_secs(tokens.expires_in);
        self.access = Some((tokens.access_token.clone(), until));
        Ok(tokens.access_token)
    }
}

pub fn authorize_url(keys: &ClientKeys, redirect: &str, state: &str, challenge: &str) -> String {
    reqwest::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("client_id", keys.id.as_str()),
            ("redirect_uri", redirect),
            ("response_type", "code"),
            ("scope", SCOPE),
            ("state", state),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            // A refresh token comes only with these two.
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )
    .map(Into::into)
    .unwrap_or_default()
}

#[derive(Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default = "an_hour")]
    pub expires_in: u64,
}

fn an_hour() -> u64 {
    3600
}

pub async fn exchange_code(
    http: &reqwest::Client,
    keys: &ClientKeys,
    code: &str,
    verifier: &str,
    redirect: &str,
) -> Result<Tokens, Error> {
    let response = http
        .post(TOKEN_URL)
        .form(&[
            ("client_id", keys.id.as_str()),
            ("client_secret", keys.secret.as_str()),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|_| Error::Network)?;
    if !response.status().is_success() {
        return Err(Error::BadClient);
    }
    response.json().await.map_err(|_| Error::Unreadable)
}

async fn refresh(
    http: &reqwest::Client,
    keys: &ClientKeys,
    refresh_token: &str,
) -> Result<Tokens, Error> {
    let response = http
        .post(TOKEN_URL)
        .form(&[
            ("client_id", keys.id.as_str()),
            ("client_secret", keys.secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|_| Error::Network)?;
    if response.status().is_client_error() {
        return Err(Error::Revoked);
    }
    if !response.status().is_success() {
        return Err(Error::Network);
    }
    response.json().await.map_err(|_| Error::Unreadable)
}

/// Tells Google to forget the grant. Disconnecting goes ahead whatever it says.
pub async fn revoke(http: &reqwest::Client, refresh_token: &str) {
    let _ = http
        .post(REVOKE_URL)
        .form(&[("token", refresh_token)])
        .send()
        .await;
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: ErrorDetail,
}

#[derive(Default, Deserialize)]
struct ErrorDetail {
    #[serde(default)]
    errors: Vec<ErrorReason>,
}

#[derive(Deserialize)]
struct ErrorReason {
    #[serde(default)]
    reason: String,
}

/// What a refusal means. Only its fixed reason word is read, never echoed.
fn classify(status: u16, body: &[u8]) -> Error {
    let reasons: Vec<String> = serde_json::from_slice::<ErrorBody>(body)
        .map(|body| body.error.errors.into_iter().map(|e| e.reason).collect())
        .unwrap_or_default();
    let has = |reason: &str| reasons.iter().any(|each| each == reason);
    if has("quotaExceeded") || has("rateLimitExceeded") || has("dailyLimitExceeded") {
        Error::Quota
    } else if status == 401 {
        Error::Revoked
    } else if has("subscriptionForbidden") || has("publisherNotFound") {
        Error::BadChannel
    } else if status == 403 || status == 400 || status == 404 {
        Error::Refused
    } else {
        Error::Network
    }
}

async fn read(request: reqwest::RequestBuilder) -> Result<Vec<u8>, Error> {
    let response = request.send().await.map_err(|_| Error::Network)?;
    let status = response.status();
    let bytes = response.bytes().await.map_err(|_| Error::Network)?;
    if status.is_success() {
        Ok(bytes.to_vec())
    } else {
        Err(classify(status.as_u16(), &bytes))
    }
}

async fn get<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    token: &str,
    path: &str,
    query: &[(&str, &str)],
) -> Result<T, Error> {
    let bytes = read(
        http.get(format!("{API}/{path}"))
            .bearer_auth(token)
            .query(query),
    )
    .await?;
    serde_json::from_slice(&bytes).map_err(|_| Error::Unreadable)
}

#[derive(Default, Deserialize)]
struct Images {
    #[serde(default)]
    default: Option<Thumbnail>,
    #[serde(default)]
    medium: Option<Thumbnail>,
    #[serde(default)]
    high: Option<Thumbnail>,
    #[serde(default)]
    standard: Option<Thumbnail>,
    #[serde(default)]
    maxres: Option<Thumbnail>,
}

impl Images {
    fn all(self) -> Vec<Thumbnail> {
        [
            self.default,
            self.medium,
            self.high,
            self.standard,
            self.maxres,
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn avatar(self) -> Option<String> {
        self.high
            .or(self.medium)
            .or(self.default)
            .and_then(|image| browse::image_url(&image.url))
    }
}

#[derive(Default, Deserialize)]
struct Page<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
    #[serde(default, rename = "nextPageToken")]
    next: Option<String>,
}

#[derive(Deserialize)]
struct OwnChannel {
    id: String,
    snippet: OwnSnippet,
}

#[derive(Deserialize)]
struct OwnSnippet {
    #[serde(default)]
    title: String,
    #[serde(default)]
    thumbnails: Images,
}

pub async fn profile(http: &reqwest::Client, token: &str) -> Result<Profile, Error> {
    let page: Page<OwnChannel> = get(
        http,
        token,
        "channels",
        &[("part", "snippet"), ("mine", "true")],
    )
    .await?;
    let channel = page.items.into_iter().next().ok_or(Error::NoChannel)?;
    Ok(Profile {
        id: channel.id,
        name: channel.snippet.title,
        image: channel.snippet.thumbnails.avatar(),
    })
}

#[derive(Deserialize)]
struct Subscription {
    id: String,
    snippet: SubscriptionSnippet,
}

#[derive(Deserialize)]
struct SubscriptionSnippet {
    #[serde(default)]
    title: String,
    #[serde(default)]
    thumbnails: Images,
    #[serde(rename = "resourceId")]
    resource: Resource,
}

#[derive(Deserialize, Serialize)]
struct Resource {
    #[serde(default, rename = "channelId")]
    channel: String,
}

fn subscription_channel(item: Subscription) -> Option<Channel> {
    valid_channel(&item.snippet.resource.channel).then(|| Channel {
        id: item.snippet.resource.channel,
        name: item.snippet.title,
        image: item.snippet.thumbnails.avatar(),
        subscription: Some(item.id),
    })
}

pub async fn subscriptions(http: &reqwest::Client, token: &str) -> Result<Vec<Channel>, Error> {
    let mut channels = Vec::new();
    let mut next: Option<String> = None;
    for _ in 0..MAX_SUBSCRIPTION_PAGES {
        let mut query = vec![
            ("part", "snippet"),
            ("mine", "true"),
            ("maxResults", PAGE_SIZE),
            ("order", "alphabetical"),
        ];
        if let Some(next) = &next {
            query.push(("pageToken", next));
        }
        let page: Page<Subscription> = get(http, token, "subscriptions", &query).await?;
        channels.extend(page.items.into_iter().filter_map(subscription_channel));
        next = page.next;
        if next.is_none() {
            break;
        }
    }
    Ok(channels)
}

#[derive(Deserialize)]
struct RatedVideo {
    id: String,
    snippet: RatedSnippet,
    #[serde(default, rename = "contentDetails")]
    details: Details,
    #[serde(default)]
    statistics: Statistics,
}

#[derive(Deserialize)]
struct RatedSnippet {
    #[serde(default)]
    title: String,
    #[serde(default, rename = "channelTitle")]
    channel: Option<String>,
    #[serde(default, rename = "channelId")]
    channel_id: Option<String>,
    #[serde(default, rename = "publishedAt")]
    published: Option<String>,
    #[serde(default)]
    thumbnails: Images,
    #[serde(default, rename = "liveBroadcastContent")]
    live: Option<String>,
}

#[derive(Default, Deserialize)]
struct Details {
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Default, Deserialize)]
struct Statistics {
    #[serde(default, rename = "viewCount")]
    views: Option<String>,
}

/// A length as YouTube writes it, `PT1H2M3S`, in seconds.
fn iso_seconds(text: &str) -> Option<f64> {
    let rest = text.strip_prefix('P')?;
    let (days, time) = rest.split_once('T').unwrap_or((rest, ""));
    let mut total = 0.0;
    if let Some(days) = days.strip_suffix('D') {
        total += days.parse::<f64>().ok()? * 86_400.0;
    }
    let mut number = String::new();
    for character in time.chars() {
        match character {
            'H' | 'M' | 'S' => {
                let value = number.parse::<f64>().ok()?;
                number.clear();
                total += value
                    * match character {
                        'H' => 3600.0,
                        'M' => 60.0,
                        _ => 1.0,
                    };
            }
            _ => number.push(character),
        }
    }
    Some(total)
}

fn rated_track(video: RatedVideo) -> Option<Track> {
    if video
        .snippet
        .live
        .as_deref()
        .is_some_and(|live| live != "none")
    {
        return None;
    }
    Video {
        id: video.id,
        title: video.snippet.title,
        channel: video.snippet.channel,
        channel_id: video.snippet.channel_id,
        uploader: None,
        description: None,
        duration: video.details.duration.as_deref().and_then(iso_seconds),
        upload_date: video
            .snippet
            .published
            .and_then(|date| date.get(..10).map(|day| day.replace('-', ""))),
        view_count: video.statistics.views.and_then(|views| views.parse().ok()),
        thumbnails: video.snippet.thumbnails.all(),
        is_live: false,
        live_status: None,
        availability: None,
    }
    .to_track()
    .ok()
}

/// The account's liked videos, newest first.
pub async fn likes(http: &reqwest::Client, token: &str) -> Result<Vec<Track>, Error> {
    let mut tracks = Vec::new();
    let mut next: Option<String> = None;
    for _ in 0..MAX_LIKE_PAGES {
        let mut query = vec![
            ("part", "snippet,contentDetails,statistics"),
            ("myRating", "like"),
            ("maxResults", PAGE_SIZE),
        ];
        if let Some(next) = &next {
            query.push(("pageToken", next));
        }
        let page: Page<RatedVideo> = get(http, token, "videos", &query).await?;
        tracks.extend(page.items.into_iter().filter_map(rated_track));
        next = page.next;
        if next.is_none() {
            break;
        }
    }
    Ok(tracks)
}

#[derive(Deserialize)]
struct Created {
    id: String,
}

/// Subscribes, and returns the subscription's id, which unsubscribing needs.
pub async fn subscribe(
    http: &reqwest::Client,
    token: &str,
    channel: &str,
) -> Result<String, Error> {
    if !(channel.starts_with("UC") && valid_channel(channel)) {
        return Err(Error::BadChannel);
    }
    let body = serde_json::json!({
        "snippet": { "resourceId": { "kind": "youtube#channel", "channelId": channel } }
    });
    let request = http
        .post(format!("{API}/subscriptions"))
        .bearer_auth(token)
        .query(&[("part", "snippet")])
        .json(&body);
    match read(request).await {
        Ok(bytes) => {
            let created: Created = serde_json::from_slice(&bytes).map_err(|_| Error::Unreadable)?;
            Ok(created.id)
        }
        Err(error) => Err(error),
    }
}

pub async fn unsubscribe(
    http: &reqwest::Client,
    token: &str,
    subscription: &str,
) -> Result<(), Error> {
    let request = http
        .delete(format!("{API}/subscriptions"))
        .bearer_auth(token)
        .query(&[("id", subscription)]);
    read(request).await.map(|_| ())
}

pub async fn rate(
    http: &reqwest::Client,
    token: &str,
    video: &str,
    liked: bool,
) -> Result<(), Error> {
    let request = http
        .post(format!("{API}/videos/rate"))
        .bearer_auth(token)
        .query(&[
            ("id", video),
            ("rating", if liked { "like" } else { "none" }),
        ])
        .header(reqwest::header::CONTENT_LENGTH, 0);
    read(request).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_taken_only_in_the_shape_google_gives_them() {
        let keys =
            ClientKeys::parse(" 123-abc.apps.googleusercontent.com ", " GOCSPX-secret\n").unwrap();
        assert_eq!(keys.id, "123-abc.apps.googleusercontent.com");
        assert_eq!(keys.secret, "GOCSPX-secret");
        assert!(ClientKeys::parse("123-abc", "secret").is_none());
        assert!(ClientKeys::parse("123.apps.googleusercontent.com", "").is_none());
        assert!(ClientKeys::parse("a b.apps.googleusercontent.com", "secret").is_none());
    }

    #[test]
    fn secrets_never_show_in_debug_output() {
        let keys = ClientKeys {
            id: "the-client-id".into(),
            secret: "the-secret".into(),
        };
        let shown = format!("{keys:?}");
        assert!(!shown.contains("the-secret") && !shown.contains("the-client-id"));
    }

    #[test]
    fn signing_in_asks_for_a_refresh_token_and_uses_pkce() {
        let keys = ClientKeys {
            id: "id.apps.googleusercontent.com".into(),
            secret: "secret".into(),
        };
        let url = authorize_url(&keys, "http://127.0.0.1:5000/login", "state", "challenge");
        assert!(url.starts_with(AUTHORIZE_URL));
        for part in [
            "access_type=offline",
            "prompt=consent",
            "code_challenge=challenge",
            "code_challenge_method=S256",
            "state=state",
        ] {
            assert!(url.contains(part), "{part}");
        }
        assert!(!url.contains("secret"), "the secret never goes in a URL");
    }

    #[test]
    fn refusals_are_told_apart_without_echoing_the_answer() {
        let body = |reason: &str| {
            format!(r#"{{"error":{{"errors":[{{"reason":"{reason}"}}],"message":"token abc"}}}}"#)
                .into_bytes()
        };
        assert_eq!(classify(403, &body("quotaExceeded")), Error::Quota);
        assert_eq!(classify(401, &body("authError")), Error::Revoked);
        assert_eq!(
            classify(403, &body("subscriptionForbidden")),
            Error::BadChannel
        );
        assert_eq!(classify(403, &body("accessNotConfigured")), Error::Refused);
        assert_eq!(classify(503, b"not json"), Error::Network);
        for error in [Error::Quota, Error::Revoked, Error::Refused, Error::Network] {
            assert!(!error.to_string().contains("abc"));
        }
    }

    #[test]
    fn lengths_read_as_youtube_writes_them() {
        assert_eq!(iso_seconds("PT3M20S"), Some(200.0));
        assert_eq!(iso_seconds("PT1H2M3S"), Some(3723.0));
        assert_eq!(iso_seconds("P1DT1S"), Some(86_401.0));
        assert_eq!(iso_seconds("P0D"), Some(0.0));
        assert_eq!(iso_seconds("nonsense"), None);
    }

    #[test]
    fn a_subscription_becomes_a_channel_that_can_be_unsubscribed_from() {
        let page: Page<Subscription> = serde_json::from_str(
            r#"{"items":[{"id":"sub-1","snippet":{"title":"Sanche Beats",
              "resourceId":{"kind":"youtube#channel","channelId":"UCliaQW3E6I9cBJkgm2fdOnw"},
              "thumbnails":{"high":{"url":"https://yt3.ggpht.com/a=s800"}}}},
              {"id":"sub-2","snippet":{"title":"Bad","resourceId":{"channelId":"--exec"}}}],
              "nextPageToken":"next"}"#,
        )
        .unwrap();
        assert_eq!(page.next.as_deref(), Some("next"));
        let channels: Vec<Channel> = page
            .items
            .into_iter()
            .filter_map(subscription_channel)
            .collect();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].id, "UCliaQW3E6I9cBJkgm2fdOnw");
        assert_eq!(channels[0].subscription.as_deref(), Some("sub-1"));
        assert_eq!(
            channels[0].image.as_deref(),
            Some("https://yt3.ggpht.com/a=s800")
        );
    }

    #[test]
    fn a_liked_video_becomes_a_song_and_live_ones_are_left_out() {
        let page: Page<RatedVideo> = serde_json::from_str(
            r#"{"items":[{"id":"eHz0FSfSLP8",
              "snippet":{"title":"DARK THOUGHTS","channelTitle":"Sanche Beats",
                "channelId":"UCliaQW3E6I9cBJkgm2fdOnw","publishedAt":"2024-05-06T10:00:00Z",
                "liveBroadcastContent":"none",
                "thumbnails":{"high":{"url":"https://i.ytimg.com/vi/eHz0FSfSLP8/hqdefault.jpg","width":480,"height":360}}},
              "contentDetails":{"duration":"PT3M"},"statistics":{"viewCount":"2260048"}},
              {"id":"7XtFkYxMsRw","snippet":{"title":"Live","liveBroadcastContent":"live"}}]}"#,
        )
        .unwrap();
        let tracks: Vec<Track> = page.items.into_iter().filter_map(rated_track).collect();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].uri, "youtube:video:eHz0FSfSLP8");
        assert_eq!(tracks[0].duration_ms, 180_000);
        assert_eq!(tracks[0].play_count, Some(2_260_048));
        assert_eq!(tracks[0].release_date.as_deref(), Some("2024-05-06"));
        assert_eq!(
            tracks[0].artists[0].uri.as_deref(),
            Some("youtube:channel:UCliaQW3E6I9cBJkgm2fdOnw")
        );
    }

    #[test]
    fn what_is_stored_round_trips_and_a_damaged_entry_reads_as_nothing() {
        let stored = Stored {
            keys: ClientKeys {
                id: "id".into(),
                secret: "secret".into(),
            },
            refresh_token: "refresh".into(),
            profile: Profile {
                id: "UC".into(),
                name: "Me".into(),
                image: None,
            },
        };
        let back = Stored::decode(&stored.encode().unwrap()).unwrap();
        assert_eq!(back.refresh_token, "refresh");
        assert_eq!(back.profile.name, "Me");
        assert!(Stored::decode(b"not json").is_none());
    }
}
