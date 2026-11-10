//! The connection a linked phone reaches this computer through. It only runs for
//! someone who has linked one, and only looks often while the phone is watching.
use super::*;
use crate::remote::{IDLE_POLL, Incoming, WATCHED_POLL};

/// What the app asks of the phone's connection.
#[derive(Debug)]
pub enum RemoteRequest {
    /// Start looking for commands, if nothing is already.
    Start,
    State(serde_json::Value),
    /// Works the phone's own player: `toggle`, `pause`, `next`, `previous` or `seek`.
    Control {
        kind: &'static str,
        ms: Option<u32>,
    },
    Reply {
        id: String,
        value: serde_json::Value,
    },
    Search {
        id: String,
        query: String,
    },
    /// The audio of a song from SoundCloud, YouTube or the group, for the phone itself.
    Stream {
        id: String,
        uri: String,
        share_url: Option<String>,
        /// For a Spotify song: what to look for on YouTube and SoundCloud.
        wanted: Option<Box<Track>>,
    },
    /// A code for a phone to type, and this listener's own Spotify app for it to use.
    LinkCode {
        client_id: Option<String>,
    },
}

#[derive(Debug)]
pub enum RemoteEvent {
    Commands(Vec<Incoming>),
    Watched(bool),
    Code(Result<String, String>),
    /// What the phone plays itself, and how old that word is, in milliseconds.
    Phone(Option<(crate::friends::PhonePlaying, i64)>),
}

/// While the phone itself plays, how often to look, so a pause or a new song there
/// shows here soon even when the phone's page is closed and can't say so at once.
const PHONE_PLAYING_POLL: std::time::Duration = std::time::Duration::from_secs(4);
/// With the event stream open, a look is only a safety net.
const STREAMED_POLL: std::time::Duration = std::time::Duration::from_secs(5);
/// The stream pings every 15 seconds; this long without a word means it's gone.
const STREAM_SILENCE: std::time::Duration = std::time::Duration::from_secs(40);

fn phone_age(
    phone: Option<crate::friends::PhonePlaying>,
    now: Option<i64>,
) -> Option<(crate::friends::PhonePlaying, i64)> {
    phone.map(|phone| {
        let age = now.map_or(0, |now| (now - phone.at).max(0));
        (phone, age)
    })
}

/// Listens on the computer's event stream until it ends: wakes the looks for a
/// waiting command, and passes on what the phone plays.
async fn listen(
    client: crate::friends::Client,
    identity: crate::friends::Identity,
    wake: Arc<tokio::sync::Notify>,
    emit: impl Fn(RemoteEvent),
) {
    let Ok(mut response) = client.remote_events(&identity).await else {
        return;
    };
    let mut lines = crate::friends::EventLines::default();
    loop {
        let chunk = match tokio::time::timeout(STREAM_SILENCE, response.chunk()).await {
            Ok(Ok(Some(chunk))) => chunk,
            _ => return,
        };
        let Some(found) = lines.push(&chunk) else {
            return;
        };
        for data in found {
            match crate::friends::RemoteWord::read(&data) {
                Some(crate::friends::RemoteWord::Command) => wake.notify_one(),
                Some(crate::friends::RemoteWord::Phone { phone, now }) => {
                    emit(RemoteEvent::Phone(phone_age(phone, now)));
                }
                None => {}
            }
        }
    }
}

/// The group sign-in, read from the system's credential store at most once a minute
/// while a phone keeps this busy. Any failure forgets it, so a new sign-in is read.
#[derive(Clone, Default)]
pub(super) struct Signin(
    Arc<std::sync::Mutex<Option<(crate::friends::Identity, std::time::Instant)>>>,
);

impl Signin {
    const KEEP: std::time::Duration = std::time::Duration::from_secs(60);

    async fn get(&self, credentials: &CredentialStore) -> Option<crate::friends::Identity> {
        if let Some((identity, at)) = self.0.lock().ok()?.clone()
            && at.elapsed() < Self::KEEP
        {
            return Some(identity);
        }
        let identity = crate::friends::saved_identity(credentials).await?;
        if let Ok(mut held) = self.0.lock() {
            *held = Some((identity.clone(), std::time::Instant::now()));
        }
        Some(identity)
    }

    fn forget(&self) {
        if let Ok(mut held) = self.0.lock() {
            *held = None;
        }
    }
}

fn songs_json(tracks: &[Track]) -> serde_json::Value {
    serde_json::to_value(crate::remote::songs(tracks)).unwrap_or_default()
}

impl Worker {
    fn remote_client(&self) -> Result<crate::friends::Client, String> {
        let http = crate::http::build_collection_client(&self.engine_config.proxy)
            .map(Http::new)
            .unwrap_or_else(Http::unavailable);
        crate::friends::Client::configured(http)
    }

    pub(super) fn remote(&mut self, request: RemoteRequest) {
        let Ok(client) = self.remote_client() else {
            return;
        };
        let credentials = self.credentials.clone();
        let signin = self.hybrid.remote_signin.clone();
        let events = self.events.clone();
        let waker = self.waker.clone();
        let emit = move |event: RemoteEvent| {
            let _ = events.send(Event::Remote(event));
            waker.wake();
        };
        match request {
            RemoteRequest::Start => {
                if self.hybrid.remote_running {
                    return;
                }
                self.hybrid.remote_running = true;
                self.edge_start();
                let emit = Arc::new(emit);
                tokio::spawn(async move {
                    let mut watched = false;
                    let wake = Arc::new(tokio::sync::Notify::new());
                    // The event stream, open only while a phone is watching.
                    let mut stream: Option<tokio::task::JoinHandle<()>> = None;
                    loop {
                        let wait = match signin.get(&credentials).await {
                            None => IDLE_POLL,
                            Some(identity) => match client.remote_poll(&identity).await {
                                Ok(polled) => {
                                    if polled.wanted != watched {
                                        watched = polled.wanted;
                                        emit(RemoteEvent::Watched(watched));
                                    }
                                    let commands: Vec<Incoming> = polled
                                        .commands
                                        .into_iter()
                                        .filter_map(|value| serde_json::from_value(value).ok())
                                        .collect();
                                    if !commands.is_empty() {
                                        emit(RemoteEvent::Commands(commands));
                                    }
                                    let phone_playing =
                                        polled.phone.as_ref().is_some_and(|phone| phone.playing);
                                    emit(RemoteEvent::Phone(phone_age(polled.phone, polled.now)));
                                    if !watched {
                                        if let Some(open) = stream.take() {
                                            open.abort();
                                        }
                                    } else if stream.as_ref().is_none_or(|open| open.is_finished())
                                    {
                                        let (client, emit, wake) =
                                            (client.clone(), Arc::clone(&emit), Arc::clone(&wake));
                                        stream = Some(tokio::spawn(listen(
                                            client,
                                            identity,
                                            wake,
                                            move |event| emit(event),
                                        )));
                                    }
                                    let streaming =
                                        stream.as_ref().is_some_and(|open| !open.is_finished());
                                    match (watched, streaming, phone_playing) {
                                        (true, true, _) => STREAMED_POLL,
                                        (true, false, _) => WATCHED_POLL,
                                        (false, _, true) => PHONE_PLAYING_POLL,
                                        (false, _, false) => IDLE_POLL,
                                    }
                                }
                                Err(_) => {
                                    signin.forget();
                                    if watched {
                                        watched = false;
                                        emit(RemoteEvent::Watched(false));
                                    }
                                    if let Some(open) = stream.take() {
                                        open.abort();
                                    }
                                    IDLE_POLL
                                }
                            },
                        };
                        // A word on the stream that a command is waiting looks at once.
                        tokio::select! {
                            () = tokio::time::sleep(wait) => {}
                            () = wake.notified() => {}
                        }
                    }
                });
            }
            RemoteRequest::Control { kind, ms } => {
                tokio::spawn(async move {
                    if let Some(identity) = signin.get(&credentials).await {
                        let _ = client.remote_control(&identity, kind, ms).await;
                    }
                });
            }
            RemoteRequest::State(state) => {
                self.edge(crate::edge::Request::State(state.clone()));
                tokio::spawn(async move {
                    if let Some(identity) = signin.get(&credentials).await {
                        let _ = client.remote_state(&identity, &state).await;
                    }
                });
            }
            RemoteRequest::Reply { id, value } => {
                self.edge(crate::edge::Request::Reply {
                    id: id.clone(),
                    value: value.clone(),
                });
                tokio::spawn(async move {
                    if let Some(identity) = signin.get(&credentials).await {
                        let _ = client.remote_reply(&identity, &id, &value).await;
                    }
                });
            }
            RemoteRequest::Stream {
                id,
                uri,
                share_url,
                wanted,
            } => {
                let find = self.remote_stream(uri, share_url, wanted);
                let commands = self.commands.clone();
                tokio::spawn(async move {
                    let value = match find.await {
                        Ok(value) => value,
                        Err(error) => serde_json::json!({ "error": error }),
                    };
                    // The phone asks this over the edge socket when a computer
                    // is in its room, so the answer goes back the same way, and
                    // through the relay as before.
                    let _ = commands.send(Command::Edge(crate::edge::Request::Reply {
                        id: id.clone(),
                        value: value.clone(),
                    }));
                    if let Some(identity) = signin.get(&credentials).await {
                        let _ = client.remote_reply(&identity, &id, &value).await;
                    }
                });
            }
            RemoteRequest::Search { id, query } => {
                let search = self.remote_search(query);
                let commands = self.commands.clone();
                tokio::spawn(async move {
                    let value = search.await;
                    let _ = commands.send(Command::Edge(crate::edge::Request::Reply {
                        id: id.clone(),
                        value: value.clone(),
                    }));
                    if let Some(identity) = signin.get(&credentials).await {
                        let _ = client.remote_reply(&identity, &id, &value).await;
                    }
                });
            }
            RemoteRequest::LinkCode { client_id } => {
                tokio::spawn(async move {
                    let answer = match signin.get(&credentials).await {
                        None => Err("Connect to your group first.".to_owned()),
                        Some(identity) => client.link_start(&identity, client_id.as_deref()).await,
                    };
                    emit(RemoteEvent::Code(answer));
                });
            }
        }
    }

    /// The address of a song's audio for the phone. A Spotify song can't play in a
    /// phone's browser, so by the owner's choice (2026-11-05) the phone plays the same
    /// song from YouTube or SoundCloud, found with the download matcher's strict rules
    /// and remembered on disk. The computer itself still plays Spotify.
    fn remote_stream(
        &self,
        uri: String,
        share_url: Option<String>,
        wanted: Option<Box<Track>>,
    ) -> impl std::future::Future<Output = Result<serde_json::Value, String>> + Send + 'static {
        let catalogue = self.youtube_catalogue();
        let soundcloud = self.soundcloud_client();
        let saved = self.saved_matches.clone();
        let commands = self.commands.clone();
        let credentials = self.credentials.clone();
        let http = crate::http::build_collection_client(&self.engine_config.proxy)
            .map(Http::new)
            .unwrap_or_else(Http::unavailable);
        async move {
            if let Some(wanted) = wanted.filter(|_| uri.starts_with("spotify:track:")) {
                let spotify_uri = uri;
                let mut next = saved
                    .get(&spotify_uri)
                    .await
                    .and_then(|found| (!found.uri.trim().is_empty()).then_some(found.uri));
                let mut from_saved = next.is_some();
                loop {
                    let found = match next.take() {
                        Some(found) => found,
                        None => {
                            let found = crate::downloads::matching::Searcher::new(
                                catalogue.clone(),
                                soundcloud.clone(),
                            )
                            .same_song(&wanted)
                            .await
                            .ok_or("This song isn't on YouTube or SoundCloud.")?;
                            saved.remember(&spotify_uri, &found).await;
                            let _ = commands.send(Command::Edge(crate::edge::Request::Matches));
                            found.uri
                        }
                    };
                    match stream_target(
                        &found,
                        None,
                        &catalogue,
                        soundcloud.as_ref(),
                        &credentials,
                        &http,
                    )
                    .await
                    {
                        Ok(value) => return Ok(value),
                        // A saved match can stop playing, for example when a
                        // video is removed. Search once more.
                        Err(_) if from_saved => {
                            saved.forget(&spotify_uri).await;
                            from_saved = false;
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            stream_target(
                &uri,
                share_url.as_deref(),
                &catalogue,
                soundcloud.as_ref(),
                &credentials,
                &http,
            )
            .await
        }
    }

    /// All three sources at once. A source that can't answer just adds nothing.
    fn remote_search(
        &self,
        query: String,
    ) -> impl std::future::Future<Output = serde_json::Value> + Send + 'static {
        let api = Arc::clone(&self.api);
        let soundcloud = self.soundcloud_client();
        async move {
            let spotify = async {
                let client = api.client_for(Operation::CatalogSearch).await.ok()?;
                client.search(&query, &["track", "artist"]).await.ok()
            };
            let sound = async {
                let client = soundcloud?;
                let (tracks, users) = tokio::join!(
                    client.search_tracks(&query, 20, 0),
                    client.search_users(&query, 4, 0)
                );
                Some((
                    tracks
                        .map(|page| {
                            page.collection
                                .iter()
                                .map(|track| track.to_track(None))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                    users.map(|page| page.collection).unwrap_or_default(),
                ))
            };
            let resolver = crate::youtube::Resolver::default();
            let video = crate::youtube::browse::videos_for(&resolver, &query);
            let channels = crate::youtube::browse::channels(&resolver, &query);
            let (spotify, sound, video, channels) = tokio::join!(spotify, sound, video, channels);
            let (sound, users) = sound.unzip();
            let mut accounts: Vec<serde_json::Value> = users
                .unwrap_or_default()
                .iter()
                .take(4)
                .map(|user| {
                    serde_json::json!({
                        "source": "soundcloud",
                        "uri": crate::soundcloud::user_uri(user.id),
                        "name": user.username,
                        "image": user.avatar_url,
                    })
                })
                .collect();
            accounts.extend(
                channels
                    .unwrap_or_default()
                    .iter()
                    .take(3)
                    .map(|(card, _)| {
                        serde_json::json!({
                            "source": "youtube",
                            "uri": card.uri,
                            "name": card.title,
                            "image": card.image,
                        })
                    }),
            );
            let (tracks, artists): (Vec<Track>, Vec<serde_json::Value>) = spotify
                .map(|found| {
                    (
                        found.tracks.map(|page| page.items).unwrap_or_default(),
                        found
                            .artists
                            .map(|page| page.items)
                            .unwrap_or_default()
                            .iter()
                            .take(8)
                            .map(|artist| {
                                serde_json::json!({
                                    "id": artist.id,
                                    "name": artist.name,
                                    "image": crate::api::models::pick_image(&artist.images, 300),
                                })
                            })
                            .collect(),
                    )
                })
                .unwrap_or_default();
            serde_json::json!({
                "artists": artists,
                "accounts": accounts,
                "spotify": songs_json(&tracks),
                "soundcloud": songs_json(&sound.unwrap_or_default()),
                "youtube": songs_json(&video.unwrap_or_default()),
            })
        }
    }
}

/// Resolves one URI to the stream the phone can play: a YouTube link, a
/// shared group song, or a SoundCloud track. Spotify URIs reach here only
/// when nothing could be matched.
async fn stream_target(
    uri: &str,
    share_url: Option<&str>,
    catalogue: &crate::youtube::Catalogue,
    soundcloud: Option<&crate::soundcloud::api::Client>,
    credentials: &CredentialStore,
    http: &Http,
) -> Result<serde_json::Value, String> {
    if crate::youtube::is_track_uri(uri) {
        let (track, url) = catalogue
            .playable(uri, false)
            .await
            .map_err(|error| error.to_string())?;
        return Ok(serde_json::json!({
            "url": url,
            "mime": "audio/mp4",
            "duration_ms": track.duration_ms,
        }));
    }
    if crate::shared_songs::is_song(uri) {
        let id = crate::shared_songs::song_id(uri).ok_or("That song is gone.")?;
        let identity = crate::friends::saved_identity(credentials)
            .await
            .ok_or("Connect to your group to play this song.")?;
        let client = crate::friends::Client::configured(http.clone())?;
        let (url, mime) = client.song_stream(&identity, id).await?;
        return Ok(serde_json::json!({ "url": url, "mime": mime }));
    }
    if crate::soundcloud::track_id(uri).is_some() {
        let client = soundcloud.ok_or("SoundCloud isn't ready yet.")?;
        let track = client
            .track_for_playback(uri, share_url, None)
            .await
            .map_err(|error| error.to_string())?;
        let info = client
            .resolve_stream(&track)
            .await
            .map_err(|error| error.to_string())?;
        if info.is_preview {
            return Err("SoundCloud only plays a preview of this song.".into());
        }
        return Ok(serde_json::json!({
            "url": info.url,
            "mime": info.mime,
            "hls": matches!(info.protocol, crate::soundcloud::models::Protocol::Hls),
            "duration_ms": info.duration_ms,
        }));
    }
    Err("This song plays from Spotify.".into())
}
