//! A linked phone's commands, carried out as if pressed here, and its questions,
//! answered from what this computer already knows or goes and loads.
use std::time::Instant;

use serde_json::{Value, json};

use super::*;
use crate::api::models::{ArtistRef, ExternalUrls, PlayableItem, Track};
use crate::backend::remote::{RemoteEvent, RemoteRequest};
use crate::remote::{
    ANSWER_WITHIN, Command as Phone, Incoming, STATE_AT_LEAST, STATE_EVERY, STATE_LOOK, Snapshot,
    allowed_uri, presave_uri, song_of, songs,
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
    pub(super) waiting: Vec<(String, Phone, Instant)>,
    /// A code to show until it runs out, or why there isn't one.
    pub code: Option<Result<(String, Instant), String>>,
    pub asking_code: bool,
    /// Commands that have arrived, carried out on the next frame.
    incoming: Vec<Incoming>,
    /// Command ids already taken from either path, so a command the relay and
    /// the edge both deliver is acted on once. Five minutes matches the edge's
    /// own dedupe window.
    seen: Vec<(String, Instant)>,
    /// What the phone plays itself, while it does.
    pub there: Option<PlayingThere>,
}

/// Longer than this without word from the phone, and what it played is forgotten.
const THERE_FORGOTTEN_MS: i64 = 120_000;
/// How long a press here shows on the phone's song before the phone agrees.
const THERE_HOLD: Duration = Duration::from_secs(4);

/// A song playing on the linked phone itself, as Spotify shows another device.
#[derive(Clone, Debug)]
pub struct PlayingThere {
    pub song: crate::friends::PhoneSong,
    playing: bool,
    position_ms: f64,
    /// When `position_ms` was true, here.
    at: Instant,
    pub device: Option<String>,
    /// A press here, shown at once until the phone says the same or time's up.
    held: Option<(bool, Instant)>,
}

impl PlayingThere {
    pub fn playing(&self) -> bool {
        match self.held {
            Some((playing, since)) if since.elapsed() < THERE_HOLD => playing,
            _ => self.playing,
        }
    }

    /// Where the song has got to by now.
    pub fn position_ms(&self) -> u32 {
        let moved = if self.playing {
            self.at.elapsed().as_millis() as f64
        } else {
            0.0
        };
        let at = self.position_ms + moved;
        let end = if self.song.duration_ms > 0.0 {
            self.song.duration_ms
        } else {
            f64::MAX
        };
        at.clamp(0.0, end) as u32
    }
}

fn is_question(command: &Phone) -> bool {
    matches!(
        command,
        Phone::Home
            | Phone::Lyrics
            | Phone::Library
            | Phone::Playlist { .. }
            | Phone::Album { .. }
            | Phone::Mixed { .. }
            | Phone::Liked
            | Phone::Saved { .. }
            | Phone::SoundcloudLikes
            | Phone::YoutubeLikes
            | Phone::Artist { .. }
            | Phone::Source { .. }
    )
}

/// A new release as the phone shows it. An album or EP opens as a list by its
/// id; a SoundCloud set by its URI; a song out on its own plays as that song.
fn release(project: &crate::for_you::Project) -> Value {
    let kind = match project.kind {
        crate::for_you::Kind::Album => "album",
        crate::for_you::Kind::Ep => "ep",
        crate::for_you::Kind::Single => "single",
    };
    json!({
        "uri": project.uri,
        "id": project.uri.strip_prefix("spotify:album:"),
        "name": project.name,
        "artists": project
            .artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>(),
        "image": project.image(300),
        "kind": kind,
        "songs": project.songs,
        "released": project.released,
        "reason": project.reason,
        "song": project.track.as_ref().map(crate::remote::Song::from),
    })
}

/// Whether a question is about something a phone may ask about. Ids go into addresses
/// here, so only plain ones get that far.
pub(super) fn fair_question(command: &Phone) -> bool {
    let plain = |id: &str| {
        !id.is_empty() && id.len() <= 64 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
    };
    match command {
        Phone::Playlist { id }
        | Phone::Album { id }
        | Phone::Artist { id }
        | Phone::Mixed { id } => plain(id),
        Phone::Source { uri } => {
            crate::soundcloud::is_page(uri) || crate::youtube::browse::is_page(uri)
        }
        Phone::Saved { uris } => uris.len() <= 50 && uris.iter().all(|uri| allowed_uri(uri)),
        _ => true,
    }
}

/// What pressing something on the phone comes to here. Anything it can't ask for,
/// such as a link that isn't a song, comes to nothing.
pub(super) fn phone_action(command: &Phone) -> Option<Action> {
    match command {
        Phone::Toggle => Some(Action::TogglePlay),
        Phone::Next => Some(Action::Next),
        Phone::Previous => Some(Action::Previous),
        Phone::Seek { ms } => Some(Action::Seek(*ms)),
        Phone::Volume { percent } => Some(Action::SetVolume((*percent).min(100))),
        Phone::Shuffle => Some(Action::ToggleShuffle),
        Phone::Repeat => Some(Action::CycleRepeat),
        Phone::Like { uri } if allowed_uri(uri) => Some(Action::ToggleSaved(uri.clone())),
        // Only a resolved album URI: a phone may pre-save an upcoming album,
        // never write another kind of thing to the library.
        Phone::Presave { uri, saved } if presave_uri(uri) => Some(Action::SetUpcomingSaved {
            album_uri: uri.clone(),
            saved: *saved,
        }),
        Phone::Play { uris, index } => {
            // The index is into what the phone sent, so it has to survive the filter.
            let wanted = uris.get(*index as usize)?;
            let uris: Vec<String> = uris
                .iter()
                .filter(|uri| allowed_uri(uri))
                .cloned()
                .collect();
            let index = uris.iter().position(|uri| uri == wanted)? as u32;
            Some(Action::PlayUris { uris, index })
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
        Phone::Download {
            uri,
            name,
            artists,
            duration_ms,
            link,
        } if allowed_uri(uri) => Some(Action::Download(vec![PlayableItem::Track(Track {
            uri: uri.clone(),
            name: name.chars().take(300).collect(),
            artists: artists
                .iter()
                .take(20)
                .map(|name| ArtistRef {
                    name: name.chars().take(200).collect(),
                    ..Default::default()
                })
                .collect(),
            duration_ms: *duration_ms,
            // Only a SoundCloud song's own page is kept: it is what gets downloaded.
            external_urls: ExternalUrls {
                spotify: crate::soundcloud::permalink(link.as_deref()).map(str::to_owned),
            },
            ..Default::default()
        })])),
        // Only a link the app can play: nothing here opens a browser over there.
        Phone::PlayLink { url } => (url.len() <= 2_000 && crate::link::parse(url).is_some())
            .then(|| crate::ui::group::play_action(url)),
        _ => None,
    }
}

impl App {
    pub(super) fn remote_event(&mut self, event: RemoteEvent) {
        match event {
            RemoteEvent::Watched(watched) => {
                // The code that just linked a phone is spent.
                if watched && !self.phone.watched {
                    self.phone.code = None;
                }
                self.phone.watched = watched;
                self.phone.last_state = None;
            }
            RemoteEvent::Commands(commands) => {
                self.accept_commands(commands);
            }
            RemoteEvent::Phone(word) => self.heard_from_phone(word),
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

    /// Takes the commands neither path has delivered before. The relay and the
    /// edge socket can both carry the same command while one reconnects; its id
    /// decides.
    pub(super) fn accept_commands(&mut self, commands: Vec<Incoming>) {
        const REMEMBER: Duration = Duration::from_secs(300);
        let now = Instant::now();
        self.phone
            .seen
            .retain(|(_, at)| now.duration_since(*at) < REMEMBER);
        for command in commands {
            if self.phone.seen.iter().any(|(id, _)| id == &command.id) {
                continue;
            }
            self.phone.seen.push((command.id.clone(), now));
            self.phone.incoming.push(command);
        }
    }

    /// Takes in what the phone plays itself. A word older than one already shown
    /// is passed over, and a press here holds until the phone agrees.
    pub(super) fn heard_from_phone(&mut self, word: Option<(crate::friends::PhonePlaying, i64)>) {
        let Some((phone, age_ms)) = word.filter(|(_, age)| *age < THERE_FORGOTTEN_MS) else {
            self.phone.there = None;
            return;
        };
        let Some(song) = phone.song.filter(|song| !song.name.is_empty()) else {
            self.phone.there = None;
            return;
        };
        let at = Instant::now()
            .checked_sub(Duration::from_millis(age_ms.max(0) as u64))
            .unwrap_or_else(Instant::now);
        let held = self.phone.there.as_ref().and_then(|there| {
            there
                .held
                .filter(|(wanted, since)| *wanted != phone.playing && since.elapsed() < THERE_HOLD)
        });
        // The same word again, from a look after the stream already said it.
        if let Some(there) = &self.phone.there
            && there.song == song
            && there.playing == phone.playing
            && there.at > at
        {
            return;
        }
        self.phone.there = Some(PlayingThere {
            song,
            playing: phone.playing,
            position_ms: phone.position_ms,
            at,
            device: phone.device,
            held,
        });
    }

    /// Works the phone's own player, and shows the result here at once.
    pub(super) fn phone_control(&mut self, kind: &'static str) {
        if kind == "toggle"
            && let Some(there) = self.phone.there.as_mut()
        {
            there.held = Some((!there.playing(), Instant::now()));
        }
        self.backend
            .send(Command::Remote(RemoteRequest::Control { kind, ms: None }));
    }

    /// Plays here what the phone was playing, from where it had got to, and stops
    /// it there: the song moves, as it does between Spotify's devices.
    pub(super) fn take_from_phone(&mut self) {
        let Some(there) = self.phone.there.take() else {
            return;
        };
        self.backend.send(Command::Remote(RemoteRequest::Control {
            kind: "pause",
            ms: None,
        }));
        if !allowed_uri(&there.song.uri) {
            return;
        }
        let mut request = PlayRequest::tracks(vec![there.song.uri.clone()]).starting_at_index(0);
        request.position_ms = there.position_ms();
        self.play_request(request, false);
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
        // Whether or not anyone is watching the relay, the phone may still be
        // in the edge room. Keep looking so a pause here reaches it at once.
        ctx.request_repaint_after(STATE_LOOK);
        if self
            .phone
            .last_try
            .is_some_and(|at| at.elapsed() < STATE_EVERY)
        {
            // Looked just now: look again as soon as allowed, so a change made
            // in between isn't left for the slow look.
            ctx.request_repaint_after(STATE_EVERY);
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
            if self.phone.watched {
                // The relay handler also hands the same state to the edge, so
                // the two paths never send it twice.
                self.backend
                    .send(Command::Remote(RemoteRequest::State(value)));
            } else {
                self.backend
                    .send(Command::Edge(crate::edge::Request::State(value)));
            }
            self.phone.last_state = Some((snapshot, Instant::now()));
        }
    }

    pub(super) fn remote_snapshot(&self) -> Snapshot {
        let upcoming: Vec<_> = self
            .queue
            .get()
            .map(|queue| queue.queue.iter().take(5).filter_map(song_of).collect())
            .unwrap_or_default();
        let Some(now) = self.now_playing() else {
            return Snapshot {
                can: crate::remote::CAN,
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
        let mut song = song;
        if song.artist_id.is_none() {
            song.artist_id = self
                .now_playing_item()
                .as_ref()
                .and_then(|item| match item {
                    PlayableItem::Track(track) => track.artists.iter().find_map(|artist| {
                        let (id, _) = self.artist_links.get(artist.uri.as_deref()?)?;
                        Some(id.clone())
                    }),
                    _ => None,
                });
        }
        Snapshot {
            can: crate::remote::CAN,
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

    pub(super) fn remote_command(&mut self, incoming: Incoming, ctx: &egui::Context) {
        let Incoming { id, command } = incoming;
        let action = match &command {
            Phone::Stream {
                uri,
                name,
                artists,
                duration_ms,
            } => {
                if !allowed_uri(uri) || uri.starts_with("spotify:episode:") {
                    self.remote_reply(id, json!({ "error": "This can't play on a phone." }));
                    return;
                }
                let share_url = self
                    .track_cache
                    .get(uri)
                    .and_then(|track| track.external_urls.spotify.clone());
                // A Spotify song is looked for elsewhere by what it's called: as this
                // computer knows it, or else as the phone describes it.
                let known = self
                    .track_cache
                    .get(uri)
                    .filter(|track| track.duration_ms > 0);
                let wanted = uri
                    .starts_with("spotify:track:")
                    .then(|| known.cloned())
                    .flatten()
                    .or_else(|| {
                        uri.starts_with("spotify:track:").then(|| Track {
                            uri: uri.clone(),
                            name: name.clone().unwrap_or_default().chars().take(300).collect(),
                            artists: artists
                                .iter()
                                .take(5)
                                .map(|artist| crate::api::models::ArtistRef {
                                    name: artist.chars().take(200).collect(),
                                    ..Default::default()
                                })
                                .collect(),
                            duration_ms: duration_ms.unwrap_or_default(),
                            ..Default::default()
                        })
                    });
                self.backend.send(Command::Remote(RemoteRequest::Stream {
                    id,
                    uri: uri.clone(),
                    share_url,
                    wanted: wanted.map(Box::new),
                }));
                // The edge cannot produce YouTube audio; put this one there
                // too so the phone can play it later with this computer off.
                if crate::youtube::is_track_uri(uri) {
                    self.backend
                        .send(Command::Edge(crate::edge::Request::Fetch(vec![
                            uri.clone(),
                        ])));
                }
                return;
            }
            Phone::AddToPlaylist {
                target,
                uri,
                name,
                artists,
                duration_ms,
                image,
            } => {
                // Only a playlist this computer offered, and only a song it
                // may play. Added as it is, without the duplicate question: a
                // dialog here would wait on a screen nobody is looking at.
                let destination = self
                    .editable_playlists()
                    .into_iter()
                    .find(|(id, _)| id == target)
                    .map(|(_, name)| name);
                let value = match destination {
                    Some(playlist_name) if allowed_uri(uri) => {
                        let track = Track {
                            uri: uri.clone(),
                            name: name.chars().take(300).collect(),
                            artists: artists
                                .iter()
                                .take(20)
                                .map(|name| ArtistRef {
                                    name: name.chars().take(200).collect(),
                                    ..Default::default()
                                })
                                .collect(),
                            duration_ms: *duration_ms,
                            album: image
                                .as_deref()
                                .filter(|url| url.starts_with("https://"))
                                .map(|url| crate::api::models::Album {
                                    images: vec![crate::api::models::Image {
                                        url: url.to_owned(),
                                        ..Default::default()
                                    }],
                                    ..Default::default()
                                }),
                            ..Default::default()
                        };
                        self.actions.push(Action::ConfirmAddToPlaylist {
                            playlist_id: target.clone(),
                            playlist_name,
                            items: vec![PlayableItem::Track(track)],
                            position: None,
                        });
                        json!({ "ok": true })
                    }
                    _ => json!({ "error": "That playlist can't take songs from here." }),
                };
                self.remote_reply(id, value);
                return;
            }
            Phone::Search { query } => {
                let query = query.trim().chars().take(200).collect();
                self.backend
                    .send(Command::Remote(RemoteRequest::Search { id, query }));
                return;
            }
            question if is_question(question) && !fair_question(question) => {
                self.remote_reply(id, json!({ "error": "Encore couldn't load that." }));
                return;
            }
            question if is_question(question) => {
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
            pressed => phone_action(pressed),
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
            Phone::Home => {
                self.ask_for_you();
                self.ask_artist_news();
            }
            Phone::Lyrics => self.request_lyrics(),
            _ => {}
        }
    }

    /// The answer to a question, or None while it's still loading.
    pub(super) fn remote_answer(&mut self, question: &Phone) -> Option<Value> {
        Some(match question {
            Phone::Home => {
                let shown = self.for_you.get()?;
                let made: Vec<Value> = self
                    .home
                    .made_for_you
                    .get()
                    .map(|lists| {
                        lists
                            .iter()
                            .take(8)
                            .map(|list| {
                                json!({
                                    "id": list.id,
                                    "name": list.name,
                                    "image": pick_image(&list.images, 300),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // New releases, as Home's large card leads with them: albums,
                // EPs and sets with what they are, or a song out on its own.
                let releases: Vec<Value> = shown
                    .projects
                    .iter()
                    .filter(|project| crate::for_you::project_allowed_on_home(project))
                    .take(12)
                    .map(release)
                    .collect();
                json!({
                    "picks": songs(shown.picks.iter().map(|pick| &pick.track)),
                    "following": songs(&shown.following),
                    "recent": songs(self.recents_view.iter().take(12).map(|play| &play.track)),
                    "news": songs(&self.artist_news),
                    "made_for_you": made,
                    "releases": releases,
                })
            }
            Phone::Saved { uris } => {
                let saved: serde_json::Map<String, Value> = uris
                    .iter()
                    .filter_map(|uri| Some((uri.clone(), Value::Bool(self.is_saved(uri)?))))
                    .collect();
                json!({ "saved": saved })
            }
            Phone::Lyrics => match &self.lyrics {
                Loadable::Loading | Loadable::NotLoaded => return None,
                Loadable::Failed(_) => json!({ "lines": [] }),
                Loadable::Loaded(lyrics) => {
                    let lyrics = lyrics.as_ref();
                    json!({
                        "uri": self.lyrics_uri,
                        "synced": lyrics.is_some_and(|found| found.synced),
                        "instrumental": lyrics.is_some_and(|found| found.instrumental),
                        "lines": lyrics.map_or(&[][..], |found| found.lines.as_slice()),
                    })
                }
            },
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
                let mixed: Vec<Value> = self
                    .mixed_playlists
                    .playlists
                    .iter()
                    .map(|list| {
                        let image = list.entries.iter().find_map(|entry| {
                            let album = entry.track.album.as_ref()?;
                            pick_image(&album.images, 300).map(str::to_owned)
                        });
                        json!({
                            "id": list.id,
                            "name": list.name,
                            "image": image,
                            "count": list.entries.len(),
                        })
                    })
                    .collect();
                // The playlists a song can be added to: mixed ones, and the
                // Spotify ones this account may edit.
                let editable: Vec<Value> = self
                    .mixed_playlists
                    .playlists
                    .iter()
                    .map(|list| json!({ "id": list.uri(), "name": list.name, "mixed": true }))
                    .chain(
                        self.library
                            .playlists
                            .get()
                            .into_iter()
                            .flatten()
                            .filter_map(|list| {
                                self.can_edit_playlist(list).then(
                                    || json!({ "id": list.id, "name": list.name, "mixed": false }),
                                )
                            }),
                    )
                    .collect();
                json!({
                    "playlists": playlists,
                    "mixed": mixed,
                    "editable": editable,
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
            Phone::Mixed { id } => {
                let list = self.mixed_playlists.get(id)?;
                let (name, uri, tracks) = (list.name.clone(), list.uri(), list.to_tracks());
                // Played from the phone, its SoundCloud and YouTube songs must be known here.
                self.cache_soundcloud_tracks(&tracks);
                json!({ "name": name, "uri": uri, "songs": songs(&tracks) })
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
        if matches!(page.upcoming, Loadable::Loading) {
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
                "followers": account.followers,
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
        let upcoming: Vec<Value> = page
            .upcoming
            .get()
            .map(|releases| {
                releases
                    .iter()
                    .map(|release| {
                        json!({
                            "uri": release.uri,
                            "album_uri": release.album_uri,
                            "name": release.name,
                            "image": release.image,
                            "release": release.release,
                            "kind": release.kind,
                            "saved": release.saved,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(json!({
            "name": artist.name,
            "uri": artist.uri,
            "following": self.is_saved(&artist.uri).unwrap_or(false),
            "image": pick_image(&artist.images, 600),
            "followers": artist.followers.as_ref().map(|followers| followers.total),
            "popular": songs(page.top_tracks.get().map_or(&[][..], Vec::as_slice)),
            "upcoming": upcoming,
            "not_on_spotify": songs(&sources.songs),
            "also_as": sources.alternates.iter().map(|account| &account.name).collect::<Vec<_>>(),
            "alternate": songs(&sources.alternate_songs),
            "sets": sources.sets,
            "produced": songs(&sources.produced),
            "unreleased": songs(&sources.unreleased),
            "edits": songs(&sources.edits),
            "accounts": accounts,
            "albums": albums,
            "looking": sources.looking(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The relay and the edge socket can both deliver the same command while
    /// one reconnects; its id is what stops a second press.
    #[test]
    fn the_same_command_from_both_paths_is_acted_on_once() {
        let mut app = crate::app::tests::test_app("edge-command-dedupe");
        let next = Incoming {
            id: "phone-1".into(),
            command: Phone::Next,
        };
        app.accept_commands(vec![next.clone()]);
        app.accept_commands(vec![next]);
        assert_eq!(app.phone.incoming.len(), 1);
        app.accept_commands(vec![Incoming {
            id: "phone-2".into(),
            command: Phone::Toggle,
        }]);
        assert_eq!(app.phone.incoming.len(), 2);
        // A different id is a different press, even for the same button.
        app.accept_commands(vec![Incoming {
            id: "phone-3".into(),
            command: Phone::Next,
        }]);
        assert_eq!(app.phone.incoming.len(), 3);
        app.backend.shutdown();
    }
}
