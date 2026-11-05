//! A linked phone's commands, carried out as if pressed here, and its questions,
//! answered from what this computer already knows or goes and loads.
use std::time::Instant;

use serde_json::{Value, json};

use super::*;
use crate::backend::remote::{RemoteEvent, RemoteRequest};
use crate::remote::{
    ANSWER_WITHIN, Command as Phone, Incoming, STATE_AT_LEAST, STATE_EVERY, Snapshot, allowed_uri,
    song_of, songs,
};

/// How long a code stays usable, which is also how long it's shown.
pub const CODE_LASTS: Duration = Duration::from_secs(300);

/// What's kept about the phone between frames.
#[derive(Default)]
pub struct Remote {
    pub started: bool,
    pub watched: bool,
    last_state: Option<(Snapshot, Instant)>,
    last_try: Option<Instant>,
    /// Questions whose answers are still loading.
    waiting: Vec<(String, Phone, Instant)>,
    /// A code to show until it runs out, or why there isn't one.
    pub code: Option<Result<(String, Instant), String>>,
    pub asking_code: bool,
    /// Commands that have arrived, carried out on the next frame.
    incoming: Vec<Incoming>,
}

impl App {
    pub(super) fn remote_event(&mut self, event: RemoteEvent) {
        match event {
            RemoteEvent::Watched(watched) => {
                self.phone.watched = watched;
                self.phone.last_state = None;
            }
            RemoteEvent::Commands(commands) => {
                self.phone.incoming.extend(commands);
            }
            RemoteEvent::Code(answer) => {
                self.phone.asking_code = false;
                let linked = answer.is_ok();
                self.phone.code = Some(answer.map(|code| (code, Instant::now())));
                if linked && !self.settings.phone_linked {
                    self.settings.phone_linked = true;
                    self.save_settings();
                }
                self.remote_start();
            }
        }
    }

    fn remote_start(&mut self) {
        if !self.phone.started {
            self.phone.started = true;
            self.backend.send(Command::Remote(RemoteRequest::Start));
        }
    }

    pub(crate) fn remote_ask_code(&mut self) {
        self.phone.asking_code = true;
        self.backend.send(Command::Remote(RemoteRequest::LinkCode {
            client_id: self.settings.web_client_id.clone(),
        }));
    }

    /// Each frame: keep the connection up, tell the phone what's changed, and
    /// finish any answer whose page has now loaded.
    pub(super) fn remote_tick(&mut self, ctx: &egui::Context) {
        if !self.settings.phone_linked {
            return;
        }
        self.remote_start();
        for command in std::mem::take(&mut self.phone.incoming) {
            self.remote_command(command, ctx);
        }
        if !self.phone.waiting.is_empty() {
            let waiting = std::mem::take(&mut self.phone.waiting);
            for (id, question, since) in waiting {
                match self.remote_answer(&question) {
                    Some(value) => self.remote_reply(id, value),
                    None if since.elapsed() >= ANSWER_WITHIN => {
                        self.remote_reply(id, json!({ "error": "Encore couldn't load that." }));
                    }
                    None => self.phone.waiting.push((id, question, since)),
                }
            }
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if !self.phone.watched {
            return;
        }
        ctx.request_repaint_after(STATE_EVERY);
        if self
            .phone
            .last_try
            .is_some_and(|at| at.elapsed() < STATE_EVERY)
        {
            return;
        }
        self.phone.last_try = Some(Instant::now());
        let snapshot = self.remote_snapshot();
        let tell = match &self.phone.last_state {
            None => true,
            Some((sent, at)) => {
                at.elapsed() >= STATE_AT_LEAST || snapshot.differs(sent, at.elapsed())
            }
        };
        if tell {
            let value = serde_json::to_value(&snapshot).unwrap_or_default();
            self.backend
                .send(Command::Remote(RemoteRequest::State(value)));
            self.phone.last_state = Some((snapshot, Instant::now()));
        }
    }

    fn remote_snapshot(&self) -> Snapshot {
        let upcoming: Vec<_> = self
            .queue
            .get()
            .map(|queue| queue.queue.iter().take(5).filter_map(song_of).collect())
            .unwrap_or_default();
        let Some(now) = self.now_playing() else {
            return Snapshot {
                next: upcoming,
                repeat: "off",
                ..Default::default()
            };
        };
        let song = self
            .now_playing_item()
            .as_ref()
            .and_then(song_of)
            .unwrap_or_else(|| crate::remote::Song {
                uri: now.uri.clone(),
                name: now.title.clone(),
                artists: now
                    .artists
                    .iter()
                    .map(|artist| artist.name.clone())
                    .collect(),
                image: now.art_url.clone(),
                duration_ms: now.duration_ms,
                ..Default::default()
            });
        Snapshot {
            liked: self.is_saved(&now.uri).unwrap_or(false),
            song: Some(song),
            position_ms: now.position_ms,
            playing: now.playing,
            loading: now.loading,
            shuffle: now.shuffle,
            repeat: match now.repeat {
                RepeatMode::Off => "off",
                RepeatMode::Context => "all",
                RepeatMode::Track => "one",
            },
            volume: now.volume_percent,
            device: (!now.local).then(|| now.device_name.clone()).flatten(),
            next: upcoming,
        }
    }

    fn remote_reply(&self, id: String, value: Value) {
        self.backend
            .send(Command::Remote(RemoteRequest::Reply { id, value }));
    }

    fn remote_command(&mut self, incoming: Incoming, ctx: &egui::Context) {
        let Incoming { id, command } = incoming;
        let action = match &command {
            Phone::Toggle => Some(Action::TogglePlay),
            Phone::Next => Some(Action::Next),
            Phone::Previous => Some(Action::Previous),
            Phone::Seek { ms } => Some(Action::Seek(*ms)),
            Phone::Volume { percent } => Some(Action::SetVolume((*percent).min(100))),
            Phone::Shuffle => Some(Action::ToggleShuffle),
            Phone::Repeat => Some(Action::CycleRepeat),
            Phone::Like { uri } if allowed_uri(uri) => Some(Action::ToggleSaved(uri.clone())),
            Phone::Play { uris, index } => {
                let uris: Vec<String> = uris
                    .iter()
                    .filter(|uri| allowed_uri(uri))
                    .cloned()
                    .collect();
                (!uris.is_empty()).then_some(Action::PlayUris {
                    uris,
                    index: *index,
                })
            }
            Phone::PlayContext { uri } if allowed_uri(uri) => Some(Action::PlayContext {
                uri: uri.clone(),
                offset_uri: None,
                offset_index: None,
            }),
            Phone::Queue { uri } if allowed_uri(uri) => Some(Action::AddToQueue {
                uri: uri.clone(),
                label: String::new(),
            }),
            // Only a link the app can play: nothing here opens a browser over there.
            Phone::PlayLink { url } => (url.len() <= 2_000 && crate::link::parse(url).is_some())
                .then(|| crate::ui::group::play_action(url)),
            Phone::Like { .. } | Phone::PlayContext { .. } | Phone::Queue { .. } => None,
            Phone::Search { query } => {
                let query = query.trim().chars().take(200).collect();
                self.backend
                    .send(Command::Remote(RemoteRequest::Search { id, query }));
                return;
            }
            question => {
                self.remote_prepare(question);
                match self.remote_answer(question) {
                    Some(value) => self.remote_reply(id, value),
                    None => self
                        .phone
                        .waiting
                        .push((id, question.clone(), Instant::now())),
                }
                return;
            }
        };
        if let Some(action) = action {
            // Applied here and now: the phone's next look shows the result.
            self.apply(action, ctx);
            self.phone.last_try = None;
        }
        self.remote_reply(id, json!({ "ok": true }));
    }

    /// Starts loading what a question is about, as opening its page would.
    fn remote_prepare(&mut self, question: &Phone) {
        match question {
            Phone::Playlist { id } => self.ensure_loaded(Page::Playlist(id.clone())),
            Phone::Album { id } => self.ensure_loaded(Page::Album(id.clone())),
            Phone::Liked => self.ensure_loaded(Page::LikedSongs),
            Phone::SoundcloudLikes => self.ensure_loaded(Page::SoundCloudLikes),
            Phone::Artist { id } => self.ensure_loaded(Page::Artist(id.clone())),
            Phone::Source { uri } => self.ensure_loaded(Page::SoundCloud(uri.clone())),
            Phone::Home => self.ask_for_you(),
            _ => {}
        }
    }

    /// The answer to a question, or None while it's still loading.
    fn remote_answer(&mut self, question: &Phone) -> Option<Value> {
        Some(match question {
            Phone::Home => {
                let shown = self.for_you.get()?;
                json!({
                    "picks": songs(shown.picks.iter().map(|pick| &pick.track)),
                    "following": songs(&shown.following),
                })
            }
            Phone::Library => {
                let playlists: Vec<Value> = self
                    .library
                    .playlists
                    .get()
                    .map(|lists| {
                        lists
                            .iter()
                            .map(|list| {
                                json!({
                                    "id": list.id,
                                    "uri": list.uri,
                                    "name": list.name,
                                    "image": pick_image(&list.images, 300),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                json!({
                    "playlists": playlists,
                    "soundcloud": self.soundcloud_library.snapshot.likes_loaded,
                    "youtube": self.youtube_likes().is_some(),
                })
            }
            Phone::Playlist { id } => {
                let page = self.playlist_pages.get(id)?;
                if !page.items.loaded_once {
                    return None;
                }
                let tracks: Vec<&Track> = page
                    .items
                    .items
                    .iter()
                    .filter_map(|row| match &row.item {
                        Some(PlayableItem::Track(track)) => Some(track),
                        _ => None,
                    })
                    .collect();
                json!({
                    "name": page.playlist.get().map(|list| list.name.clone()),
                    "uri": page.playlist.get().map(|list| list.uri.clone()),
                    "songs": songs(tracks),
                })
            }
            Phone::Album { id } => {
                let page = self.album_pages.get(id)?;
                let album = page.album.get()?;
                if !page.tracks.loaded_once {
                    return None;
                }
                let image = pick_image(&album.images, 300).map(str::to_owned);
                let mut rows = songs(&page.tracks.items);
                for row in &mut rows {
                    row.image.clone_from(&image);
                }
                json!({ "name": album.name, "uri": album.uri, "image": image, "songs": rows })
            }
            Phone::Liked => {
                if !self.library.liked.loaded_once {
                    return None;
                }
                json!({
                    "name": "Liked Songs",
                    "songs": songs(self.library.liked.items.iter().map(|saved| &saved.track)),
                })
            }
            Phone::SoundcloudLikes => {
                let snapshot = &self.soundcloud_library.snapshot;
                if !snapshot.likes_loaded {
                    return None;
                }
                json!({ "name": "SoundCloud likes", "songs": songs(&snapshot.likes) })
            }
            Phone::YoutubeLikes => {
                json!({ "name": "YouTube likes", "songs": songs(self.youtube_likes()?) })
            }
            Phone::Artist { id } => return self.remote_artist(id),
            Phone::Source { uri } => {
                let page = self.soundcloud_pages.get(uri)?;
                let content = page.content.get()?;
                json!({
                    "name": content.title,
                    "image": content.profile.as_ref().and_then(|profile| profile.image.clone()),
                    "songs": songs(&content.tracks),
                })
            }
            _ => json!({ "ok": true }),
        })
    }

    /// An artist's whole page. The parts from the other sources come as they're
    /// found, so the phone asks again while `looking` is true.
    fn remote_artist(&mut self, id: &str) -> Option<Value> {
        self.ask_artist_sources(id);
        let page = self.artist_pages.get(id)?;
        let artist = page.artist.get()?;
        if matches!(page.top_tracks, Loadable::NotLoaded | Loadable::Loading) {
            return None;
        }
        let sources = &page.sources;
        let accounts: Vec<Value> = [
            ("soundcloud", sources.soundcloud.get()),
            ("youtube", sources.youtube.get()),
        ]
        .into_iter()
        .filter_map(|(source, found)| {
            let account = found?.account.as_ref()?;
            Some(json!({
                "source": source,
                "name": account.name,
                "uri": account.uri,
                "image": account.image,
            }))
        })
        .collect();
        let albums: Vec<Value> = page
            .albums
            .values()
            .flat_map(|list| list.items.iter())
            .take(40)
            .map(|album| {
                json!({
                    "id": album.id,
                    "name": album.name,
                    "year": album.year(),
                    "image": pick_image(&album.images, 300),
                })
            })
            .collect();
        Some(json!({
            "name": artist.name,
            "uri": artist.uri,
            "image": pick_image(&artist.images, 600),
            "followers": artist.followers.as_ref().map(|followers| followers.total),
            "popular": songs(page.top_tracks.get().map_or(&[][..], Vec::as_slice)),
            "not_on_spotify": songs(&sources.songs),
            "also_as": sources.alternates.iter().map(|account| &account.name).collect::<Vec<_>>(),
            "alternate": songs(&sources.alternate_songs),
            "produced": songs(&sources.produced),
            "unreleased": songs(&sources.unreleased),
            "edits": songs(&sources.edits),
            "accounts": accounts,
            "albums": albums,
            "looking": sources.looking(),
        }))
    }
}
