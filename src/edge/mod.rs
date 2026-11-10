//! The always-on edge at `edge.encore.senni.dev`, spoken to directly.
//!
//! A linked phone may be away from this computer, so this module keeps a
//! realtime socket in the phone's room, copies the library, queue and saved
//! matches to the edge's small per-listener store, offers playlist local files
//! for the phone to play, fetches YouTube audio when the phone asks, and can
//! keep the sign-ins in the edge vault.
//!
//! Everything here runs on the backend runtime and never on the UI thread.
//! The group service mints the five-minute tokens; no token, URL with a token,
//! or refresh token is ever logged. A failed edge is silent: the group
//! service's relay keeps working exactly as before.

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::Value;

use crate::credentials::Store as CredentialStore;
use crate::downloads::saved::Saved;
use crate::friends::{Client as GroupClient, Identity, saved_identity};
use crate::http::Http;

mod fetch;
mod live;
pub mod sync;
mod upload;
mod vault;

pub use sync::{MixedPlaylist, MixedTrack, PlaylistSummary};

/// The upload key a playlist local file is offered under, named by the sync
/// record before the file has finished uploading.
pub fn local_key(uri: &str) -> String {
    upload::key_for_uri(uri)
}

/// How long before a token's expiry it is replaced. Tokens live five minutes.
const REFRESH_BEFORE: i64 = 90;

/// The scopes the group service mints separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Live,
    Media,
    Sync,
}

/// One set of five-minute tokens, with the address they belong to.
///
/// Deliberately redacted: three bearers live here, and a log line may only
/// name the address and the expiry.
#[derive(Clone)]
pub struct Tokens {
    pub url: String,
    live: String,
    media: String,
    sync: String,
    /// Unix seconds.
    pub expires_at: i64,
}

impl Tokens {
    fn get(&self, scope: Scope) -> &str {
        match scope {
            Scope::Live => &self.live,
            Scope::Media => &self.media,
            Scope::Sync => &self.sync,
        }
    }
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("url", &self.url)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or_default()
}

/// The expiry a token names in its own payload. A test token need not be
/// signed; it then lives as long as a real one.
fn token_expiry(token: &str) -> i64 {
    let payload = token
        .split('.')
        .nth(1)
        .and_then(|segment| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(segment)
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get("exp").and_then(Value::as_i64));
    payload.unwrap_or_else(|| now_secs() + 240)
}

/// Where every edge request gets its token and address from. Minting is
/// serialized so a reconnect and a sync never spend two group requests.
pub struct Source {
    http: Http,
    group: GroupClient,
    credentials: CredentialStore,
    tokens: tokio::sync::Mutex<Option<Tokens>>,
    identity: std::sync::Mutex<Option<(Identity, Instant)>>,
}

impl Source {
    pub fn new(http: Http, group: GroupClient, credentials: CredentialStore) -> Self {
        Self {
            http,
            group,
            credentials,
            tokens: tokio::sync::Mutex::new(None),
            identity: std::sync::Mutex::new(None),
        }
    }

    pub fn http(&self) -> Http {
        self.http.clone()
    }

    pub fn credentials(&self) -> &CredentialStore {
        &self.credentials
    }

    async fn identity(&self) -> Option<Identity> {
        if let Ok(held) = self.identity.lock()
            && let Some((identity, at)) = held.as_ref()
            && at.elapsed() < Duration::from_secs(60)
        {
            return Some(identity.clone());
        }
        let identity = saved_identity(&self.credentials).await?;
        if let Ok(mut held) = self.identity.lock() {
            *held = Some((identity.clone(), Instant::now()));
        }
        Some(identity)
    }

    /// A token for `scope`, replaced shortly before it runs out. Failing to
    /// mint is not an error worth showing: the relay still works.
    pub async fn tokens(&self) -> Result<Tokens, String> {
        let mut held = self.tokens.lock().await;
        if let Some(tokens) = held.as_ref()
            && tokens.expires_at - now_secs() > REFRESH_BEFORE
        {
            return Ok(tokens.clone());
        }
        let identity = self
            .identity()
            .await
            .ok_or_else(|| "Connect to your group to use the edge.".to_owned())?;
        let grant = self.group.edge_tokens(&identity).await?;
        let expires_at = [&grant.live, &grant.media, &grant.sync]
            .into_iter()
            .map(|token| token_expiry(token))
            .min()
            .unwrap_or_else(|| now_secs() + 240);
        let tokens = Tokens {
            url: grant.url,
            live: grant.live,
            media: grant.media,
            sync: grant.sync,
            expires_at,
        };
        *held = Some(tokens.clone());
        Ok(tokens)
    }

    pub async fn token(&self, scope: Scope) -> Result<(Tokens, String), String> {
        let tokens = self.tokens().await?;
        let token = tokens.get(scope).to_owned();
        Ok((tokens, token))
    }
}

/// What the app asks of the edge connection.
#[derive(Debug)]
pub enum Request {
    /// Whether this computer talks to the edge at all. Turning it off stops
    /// the socket and everything waiting behind it; nothing is deleted there.
    Enabled(bool),
    /// What this computer plays, sent to the phone at once.
    State(Value),
    /// The answer to a phone command, over the same socket.
    Reply { id: String, value: Value },
    /// The playlist snapshot changed (Spotify rows and mixed playlists).
    SyncPlaylists(Value),
    /// The queue or its position changed.
    SyncQueue(Value),
    /// Push the saved matches and take the edge's back.
    Matches,
    /// Local files to offer, newest first. Skipped ones are not retried.
    Upload(Vec<UploadJob>),
    /// The listener turned playlist uploads on or off.
    UploadsEnabled(bool),
    /// YouTube sources the edge asked for, at most two.
    Fetch(Vec<String>),
    /// Keep the sign-ins in the vault, or remove them.
    Vault(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadJob {
    /// The stable key the phone resolves this file by.
    pub key: String,
    pub path: PathBuf,
}

/// What comes back from the edge socket.
#[derive(Debug)]
pub enum Event {
    /// A command from the phone, in the same shape the relay delivers.
    Commands(Vec<crate::remote::Incoming>),
    /// What the phone itself plays, or `None` when it stopped.
    Phone(Option<crate::friends::PhonePlaying>),
    /// The edge restarted or this computer is ahead of it: read `/v1/sync/*`
    /// again.
    Reset,
}

/// The live socket plus the work queue. State and replies bypass the queue so
/// a slow upload never delays what the phone sees.
pub struct Runtime {
    tx: tokio::sync::mpsc::UnboundedSender<Request>,
    live: live::Live,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Runtime {
    pub(crate) fn spawn(
        source: Arc<Source>,
        saved: Saved,
        temp: PathBuf,
        emit: Arc<dyn Fn(Event) + Send + Sync>,
    ) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let live = live::spawn(Arc::clone(&source), emit);
        let mut tasks = Vec::new();

        let (upload_tx, mut upload_rx) = tokio::sync::mpsc::unbounded_channel::<UploadJob>();
        let uploads_enabled = Arc::new(AtomicBool::new(true));
        let uploads = upload::Uploads::new(Arc::clone(&source));
        let enabled = Arc::clone(&uploads_enabled);
        tasks.push(tokio::spawn(async move {
            while let Some(job) = upload_rx.recv().await {
                if !enabled.load(Ordering::Relaxed) {
                    continue;
                }
                let _ = uploads.upload(job).await;
            }
        }));

        let writer = sync::Writer::spawn(Arc::clone(&source), saved);
        let vault = vault::Vault::new(Arc::clone(&source));
        // `None` until the app says: pushing before the listener's choice is
        // known would delete the sign-ins the phone is using.
        let vault_setting = Arc::new(std::sync::Mutex::new(None::<bool>));
        let vault_wake = Arc::new(tokio::sync::Notify::new());
        let vault_flag = Arc::clone(&vault_setting);
        let vault_woken = Arc::clone(&vault_wake);
        tasks.push(tokio::spawn(async move {
            let mut every = tokio::time::interval(Duration::from_secs(60));
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                let setting = *vault_flag
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(on) = setting {
                    let _ = vault.push(on).await;
                }
                tokio::select! {
                    _ = every.tick() => {}
                    () = vault_woken.notified() => {}
                }
            }
        }));

        let fetch_http = source.http();
        let fetcher = temp;
        tasks.push(tokio::spawn(async move {
            while let Some(request) = rx.recv().await {
                match request {
                    Request::Enabled(_) => {}
                    Request::SyncPlaylists(data) => writer.playlists(data).await,
                    Request::SyncQueue(data) => writer.queue(data).await,
                    Request::Matches => writer.matches().await,
                    Request::Upload(jobs) => {
                        for job in jobs {
                            let _ = upload_tx.send(job);
                        }
                    }
                    Request::UploadsEnabled(on) => uploads_enabled.store(on, Ordering::Relaxed),
                    Request::Vault(on) => {
                        *vault_setting
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(on);
                        vault_wake.notify_one();
                    }
                    Request::Fetch(sources) => {
                        let upload_tx = upload_tx.clone();
                        let http = fetch_http.clone();
                        let dir = fetcher.clone();
                        tokio::spawn(async move {
                            for source in sources.into_iter().take(2) {
                                let Ok((key, path)) = fetch::download(&http, &source, &dir).await
                                else {
                                    continue;
                                };
                                let _ = upload_tx.send(UploadJob { key, path });
                            }
                        });
                    }
                    Request::State(_) | Request::Reply { .. } => {}
                }
            }
        }));

        Self { tx, live, tasks }
    }

    pub fn send(&self, request: Request) {
        match request {
            Request::State(state) => self.live.state(state),
            Request::Reply { id, value } => self.live.reply(&id, value),
            other => {
                let _ = self.tx.send(other);
            }
        }
    }

    /// Stops everything this computer does for the edge. The listener's files
    /// and sign-ins stay where they are: only this computer's side stops.
    pub fn stop(self) {
        for task in self.tasks {
            task.abort();
        }
        self.live.stop();
    }
}

/// The address of the live room, over plain `ws` only for a local test server.
pub(crate) fn live_url(tokens: &Tokens, since: u64) -> String {
    let base = if let Some(rest) = tokens.url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = tokens.url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        tokens.url.clone()
    };
    let mut url = format!("{base}/v1/live?token={}", urlencode(&tokens.live));
    if since > 0 {
        url.push_str(&format!("&since={since}"));
    }
    url
}

fn urlencode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
