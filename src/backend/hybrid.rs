//! Runtime adapters only. The authoritative order lives in App's HybridQueue.
use super::*;
use crate::player::{HybridEvent, LoadSpec, PlaybackTag, RepeatMode};
use crate::soundcloud::{api::Client, engine as sc, models as wire};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub enum HybridCommand {
    Claim,
    Load {
        tag: PlaybackTag,
        track: Box<Track>,
        position_ms: u32,
        play: bool,
    },
    Preload {
        tag: PlaybackTag,
        track: Box<Track>,
    },
    Playing {
        tag: PlaybackTag,
        playing: bool,
        sequence: u64,
    },
    Seek {
        tag: PlaybackTag,
        position_ms: u32,
    },
    Stop,
    /// An explicit Spotify replacement may proceed only after OutputReleased.
    Release,
    Resolve {
        generation: u64,
        uri: String,
    },
}

#[derive(Default)]
pub(super) struct HybridRuntime {
    client: Option<Client>,
    pub(super) account_generation: u64,
    pub(super) engine: Option<sc::Engine>,
    tag: Option<PlaybackTag>,
    track: Option<Track>,
    position_ms: u32,
    playing: bool,
    pub(super) owned: bool,
    claimed: bool,
    pub(super) releasing: bool,
    pending: Option<HybridCommand>,
    pub(super) player_waiting: Vec<PlayerCommand>,
    owner_generation: u64,
    player_waiting_generation: u64,
    streams: HashMap<String, sc::Stream>,
    preload_uri: Option<String>,
    pub(super) restart_pending: bool,
    pub(super) shutdown_pending: bool,
    pub(super) signout_pending: bool,
}

impl Worker {
    /// Catalogue adapters clone this handle to share discovery, cooldown and
    /// account scope with playback instead of constructing another client.
    pub(super) fn soundcloud_client(&self) -> Option<Client> {
        self.hybrid.client.clone()
    }

    pub(super) fn init_soundcloud(&mut self) {
        // Worker construction and this small cache read run on the backend.
        let saved = std::fs::read_to_string(self.dirs.state.join("soundcloud-client.json"))
            .ok()
            .and_then(|s| serde_json::from_str::<String>(&s).ok());
        self.hybrid.client = Some(Client::new(self.http.clone(), saved, None));
        let _ = self.commands.send(Command::RestoreSoundCloud);
    }

    pub(super) fn restore_soundcloud_account(&self) {
        let connection = crate::soundcloud::account::Connection::begin(&self.credentials);
        let path = self.dirs.soundcloud_account_file();
        let client = self
            .soundcloud_client()
            .expect("SoundCloud initialized before commands");
        let commands = self.commands.clone();
        tokio::spawn(async move {
            let state =
                tokio::task::spawn_blocking(move || crate::soundcloud::account::State::load(&path))
                    .await;
            if let Ok(Ok(state)) = state {
                let result = connection.restore(&state, &client).await;
                if connection.current() {
                    let _ =
                        commands.send(Command::SoundCloudAccountRestored { connection, result });
                }
            }
        });
    }

    pub(super) fn set_soundcloud_account(&mut self, account: crate::soundcloud::account::Account) {
        let id = self.hybrid.client.as_ref().and_then(Client::client_id);
        self.hybrid.client = Some(Client::new(
            self.http.clone(),
            id,
            account.token().map(|s| s.expose().to_owned()),
        ));
        self.hybrid.account_generation = self.hybrid.account_generation.wrapping_add(1);
        self.hybrid.streams.clear();
        self.hybrid.preload_uri = None;
        if let Some(engine) = &self.hybrid.engine {
            engine.command(sc::Command::ClearPreload);
        }
        self.emit(Event::SoundCloudAccount(account));
    }

    pub(super) async fn hybrid_command(&mut self, command: HybridCommand) {
        match command {
            HybridCommand::Claim => {
                self.hybrid.new_owner_intent();
                self.hybrid.owned = true;
                self.hybrid.tag = None;
                self.hybrid.pending = None;
                self.resume = None;
                self.resume_verify = None;
                if self.hybrid.claimed {
                    self.release_soundcloud();
                } else if let Some(engine) = &self.engine
                    && let Err(error) = engine.stop_for_handoff().await
                {
                    self.emit(Event::Error(error.to_string()));
                }
            }
            HybridCommand::Resolve { generation, uri } => {
                self.resolve_hybrid_context(generation, uri);
            }
            HybridCommand::Load {
                tag,
                track,
                position_ms,
                play,
            } => {
                self.hybrid.new_owner_intent();
                self.hybrid.owned = true;
                self.hybrid.tag = Some(tag);
                self.hybrid.track = Some(*track.clone());
                self.hybrid.position_ms = position_ms;
                self.hybrid.playing = play;
                self.resume = None;
                self.resume_verify = None;
                // Only the newest intent survives a handoff in flight.
                if self.hybrid.releasing {
                    self.hybrid.pending = Some(HybridCommand::Load {
                        tag,
                        track,
                        position_ms,
                        play,
                    });
                    return;
                }
                if !crate::soundcloud::is_track_uri(&track.uri) {
                    if self.hybrid.claimed {
                        self.hybrid.pending = Some(HybridCommand::Load {
                            tag,
                            track,
                            position_ms,
                            play,
                        });
                        self.release_soundcloud();
                        return;
                    }
                    if let Some(engine) = &self.engine {
                        let spec = LoadSpec {
                            uris: vec![track.uri],
                            offset_index: Some(0),
                            position_ms,
                            play,
                            shuffle: Some(false),
                            repeat: Some(RepeatMode::Off),
                            autoplay: false,
                            ..LoadSpec::default()
                        };
                        if let Err(error) =
                            engine.hybrid_load(tag, spec, self.engine_notify()).await
                        {
                            self.emit(Event::Hybrid {
                                tag,
                                event: HybridEvent::Failed(error.to_string()),
                            });
                        }
                    } else {
                        self.emit(Event::Hybrid {
                            tag,
                            event: HybridEvent::Failed(
                                "Enable playback on this computer to play Spotify tracks".into(),
                            ),
                        });
                    }
                } else if let Some(stream) = self.hybrid.streams.get(&track.uri).cloned() {
                    self.load_soundcloud(tag, stream).await;
                } else {
                    self.resolve_soundcloud_stream(tag, track.uri, false, None);
                }
            }
            HybridCommand::Preload { tag, track } => {
                if self.hybrid.tag != Some(tag) {
                    return;
                }
                self.hybrid.preload_uri = Some(track.uri.clone());
                if crate::soundcloud::is_track_uri(&track.uri) {
                    if let Some(stream) = self.hybrid.streams.get(&track.uri).cloned() {
                        if let Some(engine) = &self.hybrid.engine {
                            engine.command(sc::Command::Preload(stream));
                        }
                    } else {
                        self.resolve_soundcloud_stream(tag, track.uri, true, None);
                    }
                } else if let Some(engine) = &self.engine {
                    let _ = engine.preload(&track.uri);
                }
            }
            HybridCommand::Playing {
                tag,
                playing,
                sequence,
            } => {
                if self.hybrid.tag != Some(tag) {
                    return;
                }
                self.hybrid.playing = playing;
                if let Some(HybridCommand::Load { play, .. }) = &mut self.hybrid.pending {
                    *play = playing;
                }
                if self.hybrid.claimed {
                    if let Some(engine) = &self.hybrid.engine {
                        engine.command(if playing {
                            sc::Command::Play
                        } else {
                            sc::Command::Pause
                        });
                    }
                } else if self
                    .hybrid
                    .track
                    .as_ref()
                    .is_some_and(|t| !crate::soundcloud::is_track_uri(&t.uri))
                    && let Some(engine) = &self.engine
                {
                    engine.hybrid_transport(playing);
                }
                self.emit(Event::Hybrid {
                    tag,
                    event: HybridEvent::PlayingAcknowledged { sequence },
                });
            }
            HybridCommand::Seek { tag, position_ms } => {
                if self.hybrid.tag != Some(tag) {
                    return;
                }
                self.hybrid.position_ms = position_ms;
                if let Some(HybridCommand::Load {
                    position_ms: pending,
                    ..
                }) = &mut self.hybrid.pending
                {
                    *pending = position_ms;
                }
                if self.hybrid.claimed {
                    if let Some(engine) = &self.hybrid.engine {
                        engine.command(sc::Command::Seek(position_ms));
                    }
                } else if self
                    .hybrid
                    .track
                    .as_ref()
                    .is_some_and(|t| !crate::soundcloud::is_track_uri(&t.uri))
                    && let Some(engine) = &self.engine
                {
                    engine.hybrid_seek(position_ms);
                }
            }
            command @ (HybridCommand::Stop | HybridCommand::Release) => {
                self.hybrid.new_owner_intent();
                let release = matches!(command, HybridCommand::Release);
                self.hybrid.tag = None;
                self.hybrid.pending = None;
                self.hybrid.playing = false;
                self.hybrid.owned = !release;
                if self.hybrid.claimed {
                    self.release_soundcloud();
                } else if let Some(engine) = &self.engine {
                    let _ = engine.stop_for_handoff().await;
                }
            }
        }
    }

    pub(super) fn release_soundcloud(&mut self) {
        if !self.hybrid.releasing {
            self.hybrid.releasing = true;
            if let Some(engine) = &self.hybrid.engine {
                engine.command(sc::Command::Stop);
            }
        }
    }

    async fn load_soundcloud(&mut self, tag: PlaybackTag, stream: sc::Stream) {
        if self.hybrid.tag != Some(tag) {
            return;
        }
        let result = async {
            let spotify = self
                .engine
                .as_ref()
                .ok_or_else(|| "Enable local playback before playing SoundCloud".to_owned())?;
            if !self.hybrid.claimed {
                spotify
                    .stop_for_handoff()
                    .await
                    .map_err(|e| e.to_string())?;
            }
            if self.hybrid.engine.is_none() {
                let output = spotify.external_output().ok_or_else(|| {
                    "The audio output is not ready. Try playing again.".to_owned()
                })?;
                let commands = self.commands.clone();
                let notify: sc::Notify = Arc::new(move |event| {
                    let _ = commands.send(Command::SoundCloudEngine(event));
                });
                self.hybrid.engine = Some(
                    sc::Engine::new(
                        tokio::runtime::Handle::current(),
                        self.http.clone(),
                        output,
                        notify,
                    )
                    .map_err(|e| e.to_string())?,
                );
            }
            self.hybrid.claimed = true;
            self.hybrid
                .engine
                .as_ref()
                .unwrap()
                .command(sc::Command::Tagged {
                    tag,
                    command: Box::new(sc::Command::Load {
                        stream,
                        start_position_ms: self.hybrid.position_ms,
                        autoplay: self.hybrid.playing,
                    }),
                });
            if let Some(stream) = self
                .hybrid
                .preload_uri
                .as_ref()
                .and_then(|uri| self.hybrid.streams.get(uri))
                .cloned()
            {
                self.hybrid
                    .engine
                    .as_ref()
                    .unwrap()
                    .command(sc::Command::Preload(stream));
            }
            Ok::<_, String>(())
        }
        .await;
        if let Err(error) = result {
            self.emit(Event::Hybrid {
                tag,
                event: HybridEvent::Failed(error),
            });
        }
    }

    fn resolve_soundcloud_stream(
        &self,
        tag: PlaybackTag,
        uri: String,
        preload: bool,
        refresh: Option<u64>,
    ) {
        let account_generation = self.hybrid.account_generation;
        let client = self.hybrid.client.as_ref().unwrap().clone();
        let commands = self.commands.clone();
        let path = self.dirs.state.join("soundcloud-client.json");
        tokio::spawn(async move {
            let result = async {
                let id = crate::soundcloud::track_id(&uri)
                    .ok_or_else(|| "Invalid SoundCloud track".to_owned())?;
                let track = client.track(id).await.map_err(|e| e.to_string())?;
                let info = client
                    .resolve_stream(&track)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(sc::Stream {
                    uri: uri.clone(),
                    url: info.url,
                    protocol: match info.protocol {
                        wire::Protocol::Hls => sc::Protocol::Hls,
                        wire::Protocol::Progressive => sc::Protocol::Progressive,
                    },
                    mime: info.mime,
                    duration_ms: info.duration_ms,
                    is_preview: info.is_preview,
                })
            }
            .await;
            persist_client_id(&client, path).await;
            let _ = commands.send(Command::SoundCloudStream {
                account_generation,
                tag,
                preload,
                refresh,
                result,
            });
        });
    }

    pub(super) async fn soundcloud_stream(
        &mut self,
        tag: PlaybackTag,
        preload: bool,
        refresh: Option<u64>,
        result: Result<sc::Stream, String>,
    ) {
        if self.hybrid.tag != Some(tag) {
            return;
        }
        match result {
            Ok(stream) => {
                if preload && self.hybrid.preload_uri.as_deref() != Some(&stream.uri) {
                    return;
                }
                self.hybrid.streams.retain(|uri, _| {
                    self.hybrid.track.as_ref().is_some_and(|t| &t.uri == uri)
                        || self.hybrid.preload_uri.as_ref() == Some(uri)
                });
                self.hybrid
                    .streams
                    .insert(stream.uri.clone(), stream.clone());
                if let Some(request_id) = refresh {
                    if let Some(engine) = &self.hybrid.engine {
                        engine.command(sc::Command::Refresh { request_id, stream });
                    }
                } else if preload {
                    if let Some(engine) = &self.hybrid.engine {
                        engine.command(sc::Command::Preload(stream));
                    }
                } else {
                    self.load_soundcloud(tag, stream).await;
                }
            }
            Err(error) if !preload => self.emit(Event::Hybrid {
                tag,
                event: HybridEvent::Failed(error),
            }),
            Err(_) => {} // A failed lookahead must not interrupt the current track.
        }
    }

    pub(super) async fn soundcloud_event(&mut self, tagged: sc::Event) {
        let (tag, event) = match tagged {
            sc::Event::Tagged { tag, event } => (tag, *event),
            _ => return,
        };
        if matches!(event, sc::Event::OutputReleased) {
            self.hybrid.claimed = false;
            self.hybrid.releasing = false;
            if self.hybrid.shutdown_pending {
                self.hybrid.shutdown_pending = false;
                let _ = self.commands.send(Command::Shutdown);
                return;
            }
            if self.hybrid.signout_pending {
                self.hybrid.signout_pending = false;
                let _ = self.commands.send(Command::SignOut);
                return;
            }
            if self.hybrid.restart_pending {
                self.hybrid.restart_pending = false;
                self.hybrid.engine = None;
                self.replace_engine();
                return;
            }
            if let Some(command) = self.hybrid.pending.take() {
                self.hybrid_command(command).await;
            } else if let Some(engine) = &self.engine {
                for command in self.hybrid.take_player_waiting() {
                    let _ = engine.command(command);
                }
            }
            return;
        }
        if self.hybrid.tag != Some(tag) || self.hybrid.releasing {
            return;
        }
        let event = match event {
            sc::Event::State(state) => HybridEvent::State(state),
            sc::Event::EndOfTrack { .. } => HybridEvent::EndOfTrack,
            sc::Event::Error { error, .. } => HybridEvent::Failed(error.to_string()),
            sc::Event::RefreshRequired {
                request_id, uri, ..
            } => {
                self.resolve_soundcloud_stream(tag, uri, false, Some(request_id));
                return;
            }
            _ => return,
        };
        self.emit(Event::Hybrid { tag, event });
    }

    fn resolve_hybrid_context(&self, generation: u64, uri: String) {
        let account_generation = self.hybrid.account_generation;
        let client = self.hybrid.client.as_ref().unwrap().clone();
        let api = Arc::clone(&self.api);
        let engine = self.engine.clone();
        let commands = self.commands.clone();
        let path = self.dirs.state.join("soundcloud-client.json");
        tokio::spawn(async move {
            let result = async {
                if let Some(id) = uri.strip_prefix("spotify:album:") {
                    let api = api.client_for(Operation::Catalog).await.map_err(|e| e.to_string())?;
                    let mut album = api.album(id).await.map_err(|e| e.to_string())?;
                    album.tracks = None;
                    let mut tracks = Vec::new();
                    loop {
                        let page = api.album_tracks(id, tracks.len() as u32, 50).await.map_err(|e| e.to_string())?;
                        let empty = page.items.is_empty();
                        tracks.extend(page.items.into_iter().map(|mut track| { track.album = Some(album.clone()); track }));
                        if tracks.len() >= page.total as usize { break; }
                        if empty || tracks.len() > 100_000 { return Err("Could not load the complete album".into()); }
                    }
                    return Ok(crate::hybrid_queue::Context { uri: uri.clone(), name: album.name, tracks });
                }
                if uri.starts_with("spotify:") && uri.ends_with(":collection") {
                    let api = api.client_for(Operation::UserData).await.map_err(|e| e.to_string())?;
                    let mut tracks = Vec::new();
                    loop {
                        let page = api.saved_tracks(tracks.len() as u32, 50).await.map_err(|e| e.to_string())?;
                        let empty = page.items.is_empty();
                        tracks.extend(page.items.into_iter().map(|saved| saved.track));
                        if tracks.len() >= page.total as usize { break; }
                        if empty || tracks.len() > 100_000 { return Err("Could not load every liked song".into()); }
                    }
                    return Ok(crate::hybrid_queue::Context { uri: uri.clone(), name: "Liked Songs".into(), tracks });
                }
                if let Some(id) = uri.strip_prefix("spotify:playlist:") {
                    let engine = engine.ok_or_else(|| {
                        "Enable local playback to load this mixed playlist".to_owned()
                    })?;
                    let header = session_reads::playlist(engine.session(), id)
                        .await
                        .map_err(|_| "Could not read the Spotify playlist".to_owned())?;
                    let mut tracks = Vec::new();
                    let mut offset = 0;
                    loop {
                        let page = session_reads::items(engine.session(), id, offset, 100)
                            .await
                            .map_err(|_| {
                                "Could not read every row of the Spotify playlist".to_owned()
                            })?;
                        let count = page.items.len() as u32;
                        for item in page.items {
                            // Preserve null positions as unavailable rows for overlay anchors.
                            tracks.push(match item.playable() {
                                Some(PlayableItem::Track(t)) => t.clone(),
                                _ => Track {
                                    is_playable: Some(false),
                                    ..Track::default()
                                },
                            });
                        }
                        offset += count;
                        if offset >= page.total {
                            break;
                        }
                        if count == 0 || tracks.len() > 100_000 {
                            return Err("The playlist is too large or incomplete".into());
                        }
                    }
                    return Ok(crate::hybrid_queue::Context {
                        uri: uri.clone(),
                        name: header.name,
                        tracks,
                    });
                }
                if uri.starts_with("spotify:") { return Err("Open a complete song list to keep the mixed queue when starting this collection".into()); }
                let resolved = if let Some(id) = crate::soundcloud::track_id(&uri) {
                    wire::Resolved::Track(client.track(id).await.map_err(|e| e.to_string())?)
                } else if let Some(id) = crate::soundcloud::playlist_id(&uri) {
                    wire::Resolved::Playlist(client.playlist(id).await.map_err(|e| e.to_string())?)
                } else {
                    client.resolve(&uri).await.map_err(|e| e.to_string())?
                };
                match resolved {
                    wire::Resolved::Track(track) => Ok(crate::hybrid_queue::Context {
                        uri: crate::soundcloud::track_uri(track.id),
                        name: track.title.clone().unwrap_or_else(|| "SoundCloud".into()),
                        tracks: vec![track.to_track(None)],
                    }),
                    wire::Resolved::Playlist(playlist) => Ok(crate::hybrid_queue::Context {
                        uri: crate::soundcloud::playlist_uri(playlist.id),
                        name: playlist.title.clone(),
                        tracks: playlist.to_tracks(),
                    }),
                    _ => Err("Open a SoundCloud song or playlist link to play it".into()),
                }
            }
            .await;
            persist_client_id(&client, path).await;
            let _ = commands.send(Command::HybridContextResolved {
                account_generation,
                generation,
                result,
            });
        });
    }
}

impl HybridRuntime {
    fn new_owner_intent(&mut self) {
        self.owner_generation = self.owner_generation.wrapping_add(1);
        self.player_waiting.clear();
    }

    pub(super) fn defer_player(&mut self, command: PlayerCommand) {
        self.player_waiting_generation = self.owner_generation;
        self.player_waiting.push(command);
    }

    fn take_player_waiting(&mut self) -> Vec<PlayerCommand> {
        if self.owned || self.player_waiting_generation != self.owner_generation {
            self.player_waiting.clear();
        }
        std::mem::take(&mut self.player_waiting)
    }

    pub(super) fn holds_output(&self) -> bool {
        self.claimed || self.releasing
    }
    pub(super) fn tag(&self) -> Option<PlaybackTag> {
        self.tag
    }
}

pub(super) async fn persist_client_id(client: &Client, path: std::path::PathBuf) {
    if let Some(id) = client.client_id() {
        // Public client ID only. Signed URLs and account tokens never go here.
        let _ = tokio::task::spawn_blocking(move || {
            if let Ok(bytes) = serde_json::to_vec(&id) {
                let _ = fastframe_file::write(&path, bytes);
            }
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_hybrid_new_claim_discards_older_deferred_spotify_load() {
        let (runtime, mut worker, _) =
            super::super::authorization_tests::worker("hybrid-superseded-handoff");
        runtime.block_on(async {
            // SoundCloud A -> pure Spotify X is waiting for OutputReleased.
            // A newer mixed start B arrives before that acknowledgement.
            worker.hybrid.releasing = true;
            worker.hybrid.claimed = true;
            worker
                .hybrid
                .player_waiting
                .push(PlayerCommand::Load(LoadSpec {
                    uris: vec!["spotify:track:old-intent".into()],
                    play: true,
                    ..Default::default()
                }));
            worker.hybrid_command(HybridCommand::Claim).await;
            assert!(
                worker.hybrid.player_waiting.is_empty(),
                "the old pure Spotify load must not run when the new mixed queue stops"
            );
        });
    }

    #[test]
    fn review_hybrid_account_change_rejects_old_private_stream_resolution() {
        let (runtime, mut worker, _) =
            super::super::authorization_tests::worker("hybrid-old-account-stream");
        runtime.block_on(async {
            let tag = PlaybackTag {
                generation: 4,
                occurrence: crate::hybrid_queue::EntryId(6),
            };
            worker.hybrid.tag = Some(tag);
            worker.hybrid.preload_uri = Some("soundcloud:track:7".into());
            // The old account's resolution was already in flight when the
            // account was disconnected. Its result must not refill the cache.
            worker.set_soundcloud_account(crate::soundcloud::account::Account::Disconnected);
            worker
                .soundcloud_stream(
                    tag,
                    true,
                    None,
                    Ok(sc::Stream {
                        uri: "soundcloud:track:7".into(),
                        url: "https://audio.example/private-old-account".into(),
                        protocol: sc::Protocol::Progressive,
                        mime: "audio/mpeg".into(),
                        duration_ms: 120_000,
                        is_preview: false,
                    }),
                )
                .await;
            assert!(
                worker.hybrid.streams.is_empty(),
                "a result authorized by the old account cannot be reused after disconnect"
            );
        });
    }

    #[test]
    fn newer_ownership_intents_replace_deferred_legacy_commands() {
        let (runtime, mut worker, _) =
            super::super::authorization_tests::worker("hybrid-intent-generations");
        runtime.block_on(async {
            worker.hybrid.releasing = true;
            worker.hybrid_command(HybridCommand::Release).await;
            worker.hybrid.defer_player(PlayerCommand::Load(LoadSpec {
                uris: vec!["spotify:track:old".into()],
                ..Default::default()
            }));
            let old_generation = worker.hybrid.owner_generation;
            worker.hybrid_command(HybridCommand::Release).await;
            assert_ne!(worker.hybrid.owner_generation, old_generation);
            let latest = PlayerCommand::Load(LoadSpec {
                uris: vec!["spotify:track:new".into()],
                ..Default::default()
            });
            worker.hybrid.defer_player(latest.clone());
            assert_eq!(worker.hybrid.take_player_waiting(), vec![latest]);
            worker.hybrid.defer_player(PlayerCommand::Toggle);
            worker.hybrid_command(HybridCommand::Claim).await;
            worker.hybrid_command(HybridCommand::Stop).await;
            assert!(worker.hybrid.take_player_waiting().is_empty());
        });
    }

    #[test]
    fn account_generation_filters_stream_and_context_replies_in_command_loop() {
        let (runtime, mut worker, events) =
            super::super::authorization_tests::worker("hybrid-account-replies");
        let tag = PlaybackTag {
            generation: 4,
            occurrence: crate::hybrid_queue::EntryId(6),
        };
        worker.hybrid.tag = Some(tag);
        let (commands, receiver) = mpsc::unbounded_channel();
        commands
            .send(Command::SoundCloudAccount(
                crate::soundcloud::account::Account::Disconnected,
            ))
            .unwrap();
        for account_generation in [0, 1] {
            commands
                .send(Command::SoundCloudStream {
                    account_generation,
                    tag,
                    preload: false,
                    refresh: None,
                    result: Ok(sc::Stream {
                        uri: format!("soundcloud:track:{}", account_generation + 10),
                        url: "https://audio.example/stream".into(),
                        protocol: sc::Protocol::Progressive,
                        mime: "audio/mpeg".into(),
                        duration_ms: 100,
                        is_preview: false,
                    }),
                })
                .unwrap();
            commands
                .send(Command::HybridContextResolved {
                    account_generation,
                    generation: account_generation + 10,
                    result: Ok(crate::hybrid_queue::Context {
                        uri: "soundcloud:playlist:7".into(),
                        name: "Set".into(),
                        tracks: vec![],
                    }),
                })
                .unwrap();
        }
        commands.send(Command::Shutdown).unwrap();
        runtime.block_on(worker.run(receiver));
        assert_eq!(worker.hybrid.streams.len(), 1);
        assert!(worker.hybrid.streams.contains_key("soundcloud:track:11"));
        let contexts: Vec<_> = events
            .try_iter()
            .filter_map(|event| match event {
                Event::HybridContext { generation, .. } => Some(generation),
                _ => None,
            })
            .collect();
        assert_eq!(contexts, [11]);
    }

    #[test]
    fn empty_spotify_load_does_not_release_hybrid_ownership() {
        let (runtime, mut worker, events) =
            super::super::authorization_tests::worker("hybrid-invalid-replacement");
        worker.hybrid.owned = true;
        let (commands, receiver) = mpsc::unbounded_channel();
        commands
            .send(Command::Player(PlayerCommand::Load(LoadSpec::default())))
            .unwrap();
        commands.send(Command::Shutdown).unwrap();
        runtime.block_on(worker.run(receiver));
        assert!(worker.hybrid.owned);
        assert!(events.try_iter().any(
            |event| matches!(event, Event::Error(message) if message.contains("nothing to play"))
        ));
    }

    #[test]
    fn hybrid_handoff_keeps_controls_used_before_output_release() {
        let (runtime, mut worker, _) = super::super::authorization_tests::worker("hybrid-handoff");
        runtime.block_on(async {
            worker.hybrid.releasing = true;
            worker.hybrid.claimed = true;
            let tag = PlaybackTag { generation: 4, occurrence: crate::hybrid_queue::EntryId(6) };
            worker.hybrid_command(HybridCommand::Load { tag, track: Box::new(Track { uri: "spotify:track:next".into(), ..Default::default() }), position_ms: 0, play: true }).await;
            worker.hybrid_command(HybridCommand::Playing { tag, playing: false, sequence: 1 }).await;
            worker.hybrid_command(HybridCommand::Seek { tag, position_ms: 12_000 }).await;
            assert!(matches!(worker.hybrid.pending, Some(HybridCommand::Load { tag: actual, position_ms: 12_000, play: false, .. }) if actual == tag));
            let stale = PlaybackTag { generation: 3, ..tag };
            worker.hybrid_command(HybridCommand::Playing { tag: stale, playing: true, sequence: 2 }).await;
            assert!(!worker.hybrid.playing);
        });
    }

    #[test]
    fn hybrid_backend_refuses_remote_requests_before_dispatching() {
        let (runtime, mut worker, events) =
            super::super::authorization_tests::worker("hybrid-remote");
        let _entered = runtime.enter();
        worker.hybrid.owned = true;
        worker.dispatch(ApiRequest::Transfer {
            device_id: "speaker".into(),
            play: true,
        });
        worker.activate_receiver(crate::zeroconf::Receiver {
            name: "Cast".into(),
            device_id: None,
            address: "127.0.0.1".parse().unwrap(),
            port: 8009,
            path: String::new(),
            cast: true,
        });
        let events: Vec<_> = events.try_iter().collect();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| matches!(event, Event::Error(message) if message.contains("only on this computer"))));
    }
}
