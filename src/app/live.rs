//! Drives this computer's player from the group's listening session, and
//! tells the group what plays here.
//!
//! Listening along follows one member: whatever they do plays here too, and
//! touching the controls here means leaving. In a channel the controls change
//! what plays for everyone in it, and the queue belongs to all of them.
use super::*;
use crate::friends::NewPost;
use crate::friends::live::{
    self, HEARTBEAT, LOOKING_POLL, Op, Outcome, PENDING_CHANNEL, Place, Session, TOGETHER_POLL,
    Wanted,
};

/// How often the session is checked against the player.
const TICK: Duration = Duration::from_millis(500);
/// Further than this from where the session is, the player is moved.
const DRIFT_MS: u64 = 2_500;
/// After moving the player, it gets this long to settle before the next.
const SETTLE: Duration = Duration::from_secs(5);
/// A song that has not started by now is asked for again.
const RETRY_START: Duration = Duration::from_secs(10);
const START_ATTEMPTS: u32 = 3;
/// A song counts as finished this long after its length has passed, so a
/// listener a moment behind still hears its end.
const END_GRACE_MS: u64 = 1_500;
/// A position this close to the end does not start a song again.
const NEAR_END_MS: u64 = 1_000;
/// A read not answered by now was lost on the way.
const READ_LOST: Duration = Duration::from_secs(20);

#[derive(Default)]
pub(super) struct Driver {
    last_tick: Option<Instant>,
    last_read: Option<Instant>,
    published: Option<Published>,
    loaded: Option<Loaded>,
    last_correction: Option<Instant>,
    ended: Option<(String, Instant)>,
}

struct Published {
    url: Option<String>,
    playing: bool,
    following: Option<String>,
    position_ms: u64,
    at: Instant,
}

/// The song this session last asked the player for.
struct Loaded {
    url: String,
    uri_before: Option<String>,
    /// What the player calls it: not always what was asked for, as Spotify
    /// relinks songs and a SoundCloud link names no id.
    uri: Option<String>,
    attempts: u32,
    at: Instant,
}

fn wall_ms() -> u64 {
    jiff::Timestamp::now().as_millisecond().max(0) as u64
}

fn play_request(wanted: &Wanted) -> Option<PlayRequest> {
    let uri = crate::link::parse(&wanted.item.url)?;
    let mut request = if uri.starts_with("spotify:track:") {
        PlayRequest::tracks(vec![uri]).starting_at_index(0)
    } else {
        PlayRequest::context(uri)
    };
    request.position_ms = u32::try_from(wanted.position_ms).unwrap_or(u32::MAX);
    Some(request)
}

impl App {
    fn me(&self) -> Option<String> {
        self.friends_feed
            .profile
            .as_ref()
            .map(|p| p.user_id.clone())
    }

    fn live_send(&mut self, op: Op) {
        let request = self.live.request(op);
        self.backend.send(Command::Live(request));
    }

    fn live_read_now(&mut self) {
        if let Some(request) = self.live.read() {
            self.live_driver.last_read = Some(Instant::now());
            self.backend.send(Command::Live(request));
        }
    }

    /// What plays here, as the group would be told or a channel would queue it.
    /// None for what has no share link: an episode or a local file.
    fn live_item(&self, now: &NowPlaying) -> Option<NewPost> {
        if now.is_episode {
            return None;
        }
        let track = if self.hybrid_owned() {
            self.hybrid_current_item().and_then(|item| match item {
                PlayableItem::Track(mut track) => {
                    track.duration_ms = now.duration_ms;
                    Some(track)
                }
                _ => None,
            })
        } else {
            None
        }
        .or_else(|| {
            crate::soundcloud::is_uri(&now.uri)
                .then(|| self.soundcloud_track(&now.uri).cloned())
                .flatten()
        })
        .unwrap_or_else(|| crate::history::played_track(now));
        NewPost::from_item(&PlayableItem::Track(track))
            .ok()
            .filter(|item| !item.title.trim().is_empty())
    }

    /// A song named only by its URI, with whatever the app knows about it.
    fn live_item_of(&self, uri: &str, label: &str) -> Option<NewPost> {
        let track = self.hybrid_track(uri, Some(label));
        NewPost::from_item(&PlayableItem::Track(track)).ok()
    }

    pub(super) fn live_action(&mut self, action: live::Action) {
        let Some(me) = self.me() else {
            return;
        };
        let now_ms = self.live.server_now(wall_ms());
        match action {
            live::Action::SetSharing(share) => {
                self.settings.group_sharing = Some(share);
                self.mark_settings_dirty();
            }
            live::Action::ListenAlong(user) => {
                if user == me {
                    return;
                }
                if self.live.leader_of(&user) == Some(me.as_str()) {
                    self.toast(gettext(
                        self.locale,
                        "They are already listening along with you",
                    ));
                    return;
                }
                self.live_leave_session(&me);
                self.live.follow(user);
                self.live_driver.last_correction = None;
                self.live_read_now();
            }
            live::Action::Leave => self.live_leave_session(&me),
            live::Action::StartChannel(name) => {
                let name = name.trim().to_owned();
                if name.is_empty() {
                    return;
                }
                let (item, position_ms, playing) = match self.now_playing_live() {
                    Some(now) => (
                        self.live_item(&now),
                        u64::from(now.position_ms),
                        now.playing,
                    ),
                    None => (None, 0, false),
                };
                self.live_leave_following();
                self.live
                    .start_channel(&me, &name, item.clone(), position_ms, playing, now_ms);
                self.live_driver.loaded = None;
                self.live_send(Op::Create {
                    name,
                    item,
                    position_ms,
                    playing,
                });
            }
            live::Action::JoinChannel(id) => {
                self.live_leave_following();
                self.live.join_channel(&me, &id);
                self.live_driver.loaded = None;
                self.live_driver.last_correction = None;
                self.live_send(Op::Join(id));
            }
            live::Action::Add(item, place) => self.live_add(&me, item, place, now_ms),
            live::Action::Remove(entry) => {
                let Some(channel) = self.live_channel_id() else {
                    return;
                };
                self.live.remove(&entry);
                self.live_send(Op::Remove { channel, entry });
            }
            live::Action::Move { entry, index } => {
                let Some(channel) = self.live_channel_id() else {
                    return;
                };
                self.live.move_entry(&entry, index);
                self.live_send(Op::Move {
                    channel,
                    entry,
                    index,
                });
            }
            live::Action::PlayEntry(entry) => {
                let Some(channel) = self.live_channel_id() else {
                    return;
                };
                self.live.advance(Some(&entry), now_ms);
                self.live_send(Op::Next {
                    channel,
                    from: None,
                    to: Some(entry),
                });
                self.live_after_control();
            }
        }
    }

    /// The channel this listener is in, once the service knows it. A change
    /// made before then would have nowhere to go.
    fn live_channel_id(&mut self) -> Option<String> {
        match &self.live.session {
            Session::Channel(id) if id == PENDING_CHANNEL => {
                self.toast(gettext(self.locale, "The channel is still starting"));
                None
            }
            Session::Channel(id) => Some(id.clone()),
            _ => None,
        }
    }

    fn live_add(&mut self, me: &str, item: NewPost, place: Place, now: u64) {
        let Some(channel) = self.live_channel_id() else {
            return;
        };
        self.live.add(me, item.clone(), place, now);
        self.live_send(Op::Add {
            channel,
            item,
            place,
        });
        self.live_after_control();
    }

    fn live_leave_following(&mut self) {
        if matches!(self.live.session, Session::Following(_)) {
            self.live.session = Session::Alone;
        }
    }

    fn live_leave_session(&mut self, me: &str) {
        if let Some(channel) = self.live.leave(me)
            && channel != PENDING_CHANNEL
        {
            self.live_send(Op::LeaveChannel(channel));
        }
        self.live_driver.loaded = None;
        self.live_driver.ended = None;
    }

    /// Leaving the group ends everything live, and its identity with it.
    pub(super) fn live_forget(&mut self) {
        self.live.reset();
        self.live_driver = Driver::default();
        self.backend.send(Command::Live(live::Request {
            generation: 0,
            op: Op::Forget,
        }));
    }

    /// Runs a control's result through the player without waiting for the
    /// next tick: pressing pause pauses now.
    fn live_after_control(&mut self) {
        self.live_driver.last_correction = None;
        self.live_reconcile();
    }

    fn live_position(&self) -> u64 {
        self.live
            .wanted(self.live.server_now(wall_ms()))
            .map_or(0, |wanted| wanted.position_ms)
    }

    /// Whether an action belongs to the session rather than to this player.
    /// True when it was handled here and must go no further.
    pub(super) fn live_intercept(&mut self, action: &Action) -> bool {
        if !self.live.together() {
            return false;
        }
        let starts_playback = match action {
            Action::PlayContext { .. }
            | Action::PlaySearch { .. }
            | Action::PlaySearchMatch { .. }
            | Action::PlayEpisode { .. }
            | Action::PlayUris { .. }
            | Action::PlayFromRow { .. }
            | Action::ShufflePlay(_) => true,
            // A link to a page only opens it; a link to a song plays it.
            Action::OpenLink(uri) => {
                !uri.starts_with("spotify:") || uri.starts_with("spotify:track:")
            }
            _ => false,
        };
        let Some(me) = self.me() else {
            return false;
        };
        match self.live.session.clone() {
            Session::Alone => false,
            Session::Following(_) => {
                if starts_playback
                    || matches!(
                        action,
                        Action::TogglePlay
                            | Action::Next
                            | Action::Previous
                            | Action::Seek(_)
                            | Action::SeekBy(_)
                    )
                {
                    self.live_leave_session(&me);
                    self.toast(gettext(self.locale, "Stopped listening along"));
                }
                false
            }
            Session::Channel(id) => {
                let now = self.live.server_now(wall_ms());
                if starts_playback {
                    self.live_leave_session(&me);
                    self.toast(gettext(self.locale, "Left the channel"));
                    return false;
                }
                let channel_playing = self.live.my_channel().is_some_and(|c| c.playing);
                match action {
                    Action::TogglePlay => {
                        self.live_control_playback(!channel_playing, self.live_position(), now)
                    }
                    Action::Seek(position) => {
                        self.live_control_playback(channel_playing, u64::from(*position), now)
                    }
                    Action::SeekBy(by) => {
                        let target = self.live_position().saturating_add_signed(*by);
                        self.live_control_playback(channel_playing, target, now)
                    }
                    Action::Previous => self.live_control_playback(channel_playing, 0, now),
                    Action::Next => {
                        if id != PENDING_CHANNEL {
                            let from = self
                                .live
                                .my_channel()
                                .and_then(|c| c.current.as_ref())
                                .map(|e| e.id.clone());
                            self.live.advance(None, now);
                            self.live_send(Op::Next {
                                channel: id,
                                from,
                                to: None,
                            });
                            self.live_after_control();
                        }
                    }
                    Action::AddToQueue { uri, label } => match self.live_item_of(uri, label) {
                        Some(item) => self.live_add(&me, item, Place::End, now),
                        None => {
                            self.toast(gettext(self.locale, "This can't be added to the channel"))
                        }
                    },
                    Action::QueueMany { songs } => {
                        for (uri, label) in songs {
                            if let Some(item) = self.live_item_of(uri, label) {
                                self.live_add(&me, item, Place::End, now);
                            }
                        }
                    }
                    Action::PlayNext { items } => {
                        for item in items.iter().rev() {
                            if let Ok(item) = NewPost::from_item(item) {
                                self.live_add(&me, item, Place::Next, now);
                            }
                        }
                    }
                    _ => return false,
                }
                true
            }
        }
    }

    fn live_control_playback(&mut self, playing: bool, position_ms: u64, now: u64) {
        let Some(channel) = self.live_channel_id() else {
            return;
        };
        self.live.set_playback(playing, position_ms, now);
        self.live_send(Op::Playback {
            channel,
            playing,
            position_ms,
        });
        self.live_after_control();
    }

    pub(super) fn handle_live_event(&mut self, event: live::Event) {
        let Some(me) = self.me() else {
            return;
        };
        match self.live.receive(event, &me) {
            Outcome::Nothing => {}
            Outcome::LeaderGone(user) => {
                self.live_driver.loaded = None;
                let name = self
                    .friends_feed
                    .members
                    .iter()
                    .find(|m| m.user_id == user)
                    .map_or_else(String::new, |m| m.display_name.clone());
                self.toast(if name.is_empty() {
                    gettext(self.locale, "Listening along ended").into_owned()
                } else {
                    format!("{}: {name}", gettext(self.locale, "Listening along ended"))
                });
            }
            Outcome::ChannelGone => {
                self.live_driver.loaded = None;
                self.toast(gettext(self.locale, "The channel ended"));
            }
            Outcome::WriteFailed(error) => {
                self.toast_error(error);
                self.live_read_now();
            }
        }
    }

    /// Called every frame and background pass; acts a few times a second.
    pub(super) fn live_tick(&mut self, ctx: &egui::Context, looking: bool) {
        if self.friends_feed.profile.is_none() || self.offline {
            return;
        }
        let together = self.live.together();
        if together || looking || self.live_driver.published.is_some() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        if self
            .live_driver
            .last_tick
            .is_some_and(|at| at.elapsed() < TICK)
        {
            return;
        }
        self.live_driver.last_tick = Some(Instant::now());

        let every = if together {
            Some(TOGETHER_POLL)
        } else if looking {
            Some(LOOKING_POLL)
        } else {
            None
        };
        if let Some(every) = every
            && self
                .live_driver
                .last_read
                .is_none_or(|at| at.elapsed() >= every)
        {
            if self
                .live_driver
                .last_read
                .is_some_and(|at| at.elapsed() >= READ_LOST)
            {
                self.live.read_lost();
            }
            self.live_read_now();
        }
        self.live_publish();
        if together {
            self.live_reconcile();
            self.live_report_end();
        }
    }

    /// Says what plays here when that changed, and now and then so it stays.
    fn live_publish(&mut self) {
        let sharing = self.settings.group_sharing == Some(true);
        let together = self.live.together();
        let now = self.now_playing_live();
        let item = if sharing {
            now.as_ref().and_then(|now| self.live_item(now))
        } else {
            None
        };
        if item.is_none() && !together {
            if self.live_driver.published.take().is_some() {
                self.live_send(Op::ClearPresence);
            }
            return;
        }
        let playing = now.as_ref().is_some_and(|now| now.playing);
        let position_ms = now.as_ref().map_or(0, |now| u64::from(now.position_ms));
        let following = match &self.live.session {
            Session::Following(user) => Some(user.clone()),
            _ => None,
        };
        let url = item.as_ref().map(|item| item.url.clone());
        let due = match &self.live_driver.published {
            None => true,
            Some(last) => {
                let expected = if last.playing {
                    last.position_ms + last.at.elapsed().as_millis() as u64
                } else {
                    last.position_ms
                };
                last.url != url
                    || last.playing != playing
                    || last.following != following
                    || expected.abs_diff(position_ms) > 3_000
                    || last.at.elapsed() >= HEARTBEAT
            }
        };
        if !due {
            return;
        }
        self.live_driver.published = Some(Published {
            url,
            playing,
            following: following.clone(),
            position_ms,
            at: Instant::now(),
        });
        self.live_send(Op::Presence {
            item,
            position_ms,
            playing,
            following,
        });
    }

    fn live_settled(&self) -> bool {
        self.live_driver
            .last_correction
            .is_none_or(|at| at.elapsed() >= SETTLE)
    }

    fn live_start(&mut self, wanted: &Wanted) {
        let attempts = match &self.live_driver.loaded {
            Some(loaded) if loaded.url == wanted.item.url => loaded.attempts,
            _ => 0,
        };
        if attempts >= START_ATTEMPTS {
            return;
        }
        let Some(request) = play_request(wanted) else {
            return;
        };
        let uri_before = self.now_playing_live().map(|now| now.uri);
        self.live_driver.loaded = Some(Loaded {
            url: wanted.item.url.clone(),
            uri_before,
            uri: None,
            attempts: attempts + 1,
            at: Instant::now(),
        });
        self.live_driver.last_correction = Some(Instant::now());
        self.play_request(request, false);
    }

    /// Moves the player to what the session wants, a little at a time so a
    /// slow load or a lagging answer never sets off a storm of commands.
    fn live_reconcile(&mut self) {
        let now_ms = self.live.server_now(wall_ms());
        let now = self.now_playing_live();
        let Some(wanted) = self.live.wanted(now_ms) else {
            // The leader stopped, or the channel's queue ran out.
            if now.as_ref().is_some_and(|now| now.playing) && self.live_settled() {
                self.live_driver.last_correction = Some(Instant::now());
                self.toggle_play();
            }
            return;
        };
        let duration = if wanted.item.duration_ms > 0 {
            u64::from(wanted.item.duration_ms)
        } else {
            now.as_ref().map_or(0, |now| u64::from(now.duration_ms))
        };
        let near_end = duration > 0 && wanted.position_ms + NEAR_END_MS >= duration;

        let same_song = self
            .live_driver
            .loaded
            .as_ref()
            .is_some_and(|loaded| loaded.url == wanted.item.url);
        if !same_song {
            // Already on it, as when starting a channel from the playing song.
            let playing_it = now
                .as_ref()
                .and_then(|now| self.live_item(now))
                .is_some_and(|item| item.url == wanted.item.url);
            if playing_it {
                self.live_driver.loaded = Some(Loaded {
                    url: wanted.item.url.clone(),
                    uri_before: None,
                    uri: now.as_ref().map(|now| now.uri.clone()),
                    attempts: 1,
                    at: Instant::now(),
                });
            } else if !near_end {
                self.live_start(&wanted);
                return;
            } else {
                return;
            }
        }
        let Some(now) = now else {
            let waited = self
                .live_driver
                .loaded
                .as_ref()
                .is_some_and(|loaded| loaded.at.elapsed() >= RETRY_START);
            if waited && !near_end {
                self.live_start(&wanted);
            }
            return;
        };
        let Some(loaded) = self.live_driver.loaded.as_mut() else {
            return;
        };
        if loaded.uri.is_none() {
            let changed = loaded.uri_before.as_deref() != Some(now.uri.as_str());
            let waited = loaded.at.elapsed() >= Duration::from_secs(3);
            if (changed || waited) && !now.loading {
                loaded.uri = Some(now.uri.clone());
            } else {
                if loaded.at.elapsed() >= RETRY_START && !near_end {
                    self.live_start(&wanted);
                }
                return;
            }
        }
        if loaded.uri.as_deref() != Some(now.uri.as_str()) {
            // Something else took over, such as Spotify's autoplay at a song's
            // end. Before the next song is known, that means quiet.
            if near_end {
                if now.playing && self.live_settled() {
                    self.live_driver.last_correction = Some(Instant::now());
                    self.toggle_play();
                }
            } else if self.live_settled() {
                self.live_start(&wanted);
            }
            return;
        }
        if now.loading || !self.live_settled() {
            return;
        }
        if now.playing != wanted.playing {
            if !(wanted.playing && near_end) {
                self.live_driver.last_correction = Some(Instant::now());
                self.toggle_play();
            }
            return;
        }
        if !near_end && u64::from(now.position_ms).abs_diff(wanted.position_ms) > DRIFT_MS {
            self.live_driver.last_correction = Some(Instant::now());
            self.seek(u32::try_from(wanted.position_ms).unwrap_or(u32::MAX));
        }
    }

    /// A channel moves on when its song has run out. Everyone in it says so;
    /// the service takes the first and ignores the rest.
    fn live_report_end(&mut self) {
        let Session::Channel(channel) = self.live.session.clone() else {
            return;
        };
        let now_ms = self.live.server_now(wall_ms());
        let Some(wanted) = self.live.wanted(now_ms) else {
            return;
        };
        let Some(entry) = wanted.entry.clone() else {
            return;
        };
        let duration = if wanted.item.duration_ms > 0 {
            u64::from(wanted.item.duration_ms)
        } else {
            let on_it = self
                .live_driver
                .loaded
                .as_ref()
                .is_some_and(|l| l.url == wanted.item.url);
            self.now_playing_live()
                .filter(|_| on_it)
                .map_or(0, |now| u64::from(now.duration_ms))
        };
        if !wanted.playing
            || duration == 0
            || wanted.position_ms < duration + END_GRACE_MS
            || channel == PENDING_CHANNEL
            || entry.starts_with("pending-")
        {
            return;
        }
        let reported = self
            .live_driver
            .ended
            .as_ref()
            .is_some_and(|(id, at)| *id == entry && at.elapsed() < RETRY_START);
        if reported {
            return;
        }
        self.live_driver.ended = Some((entry.clone(), Instant::now()));
        self.live_send(Op::Next {
            channel,
            from: Some(entry),
            to: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_app;
    use super::*;
    use crate::friends::live::{Channel, Entry, Presence, Snapshot};
    use crate::friends::{Profile, Source};

    const ME: &str = "00000000-0000-0000-0000-00000000000a";
    const ANA: &str = "00000000-0000-0000-0000-00000000000b";
    const CHANNEL: &str = "00000000-0000-0000-0000-00000000000c";

    fn song(title: &str) -> NewPost {
        NewPost {
            url: format!("https://open.spotify.com/track/{title}"),
            source: Source::Spotify,
            title: title.into(),
            artists: vec!["Artist".into()],
            artwork_url: None,
            duration_ms: 180_000,
            note: String::new(),
            to: Vec::new(),
        }
    }

    fn app(name: &str) -> App {
        let mut app = test_app(name);
        app.friends_feed.profile = Some(Profile {
            user_id: ME.into(),
            display_name: "Me".into(),
        });
        app
    }

    fn presence(title: &str) -> Presence {
        Presence {
            user_id: ANA.into(),
            item: Some(song(title)),
            position_ms: 20_000,
            playing: true,
            at: wall_ms(),
            following: None,
        }
    }

    fn entry(id: &str) -> Entry {
        Entry {
            id: id.into(),
            item: song(id),
            added_by: ANA.into(),
        }
    }

    fn channel() -> Channel {
        Channel {
            id: CHANNEL.into(),
            name: "Friday".into(),
            host_id: ANA.into(),
            created_at: 1,
            members: vec![ANA.into(), ME.into()],
            current: Some(entry("11111111-0000-0000-0000-000000000001")),
            position_ms: 20_000,
            playing: true,
            at: wall_ms(),
            queue: vec![entry("22222222-0000-0000-0000-000000000002")],
        }
    }

    fn in_channel(name: &str) -> App {
        let mut app = app(name);
        app.live.snapshot = Snapshot {
            now: wall_ms(),
            presence: vec![],
            channels: vec![channel()],
        };
        app.live.session = Session::Channel(CHANNEL.into());
        app
    }

    fn following(name: &str) -> App {
        let mut app = app(name);
        app.live.snapshot = Snapshot {
            now: wall_ms(),
            presence: vec![presence("leaderSong")],
            channels: vec![],
        };
        app.live.session = Session::Following(ANA.into());
        app
    }

    fn ops(app: &App) -> Vec<Op> {
        app.backend
            .take_live_requests()
            .into_iter()
            .map(|request| request.op)
            .collect()
    }

    #[test]
    fn listening_along_starts_the_leaders_song_at_their_position() {
        let mut app = following("live-follow-start");
        app.live_reconcile();
        let Some(loaded) = &app.live_driver.loaded else {
            panic!("nothing was started");
        };
        assert_eq!(loaded.url, song("leaderSong").url);
        assert_eq!(loaded.attempts, 1);
    }

    #[test]
    fn a_song_that_will_not_start_is_asked_for_a_few_times_only() {
        let mut app = following("live-follow-retries");
        for _ in 0..10 {
            let wanted = app.live.wanted(wall_ms()).unwrap();
            app.live_start(&wanted);
        }
        assert_eq!(
            app.live_driver.loaded.as_ref().unwrap().attempts,
            START_ATTEMPTS
        );
    }

    #[test]
    fn a_position_past_the_songs_end_does_not_start_it_again() {
        let mut app = app("live-follow-past-end");
        let mut ana = presence("leaderSong");
        ana.position_ms = 179_900;
        app.live.snapshot.presence = vec![ana];
        app.live.session = Session::Following(ANA.into());
        app.live_reconcile();
        assert!(app.live_driver.loaded.is_none());
    }

    #[test]
    fn the_controls_leave_when_listening_along() {
        for action in [
            Action::TogglePlay,
            Action::Next,
            Action::Previous,
            Action::Seek(5),
            Action::PlayUris {
                uris: vec!["spotify:track:x".into()],
                index: 0,
            },
        ] {
            let mut app = following("live-follow-controls");
            assert!(!app.live_intercept(&action), "the action still applies");
            assert_eq!(app.live.session, Session::Alone);
        }
    }

    #[test]
    fn queueing_does_not_leave_when_listening_along() {
        let mut app = following("live-follow-queue");
        let action = Action::AddToQueue {
            uri: "spotify:track:x".into(),
            label: "X".into(),
        };
        assert!(!app.live_intercept(&action));
        assert!(app.live.together());
    }

    #[test]
    fn the_players_controls_reach_the_channel() {
        let ctx = egui::Context::default();
        let mut app = in_channel("live-channel-apply");
        app.apply(Action::Next, &ctx);
        assert!(matches!(ops(&app).as_slice(), [Op::Next { .. }]));
        app.apply(
            Action::AddToQueue {
                uri: "spotify:track:abc".into(),
                label: "Abc".into(),
            },
            &ctx,
        );
        assert!(matches!(ops(&app).as_slice(), [Op::Add { .. }]));
    }

    #[test]
    fn nothing_is_intercepted_when_alone() {
        let mut app = app("live-alone");
        assert!(!app.live_intercept(&Action::Next));
        assert!(ops(&app).is_empty());
    }

    #[test]
    fn next_in_a_channel_moves_it_on_for_everyone_at_once() {
        let mut app = in_channel("live-channel-next");
        assert!(app.live_intercept(&Action::Next));
        let channel = app.live.my_channel().unwrap();
        assert_eq!(
            channel.current.as_ref().unwrap().id,
            "22222222-0000-0000-0000-000000000002"
        );
        assert!(channel.queue.is_empty());
        assert!(matches!(
            ops(&app).as_slice(),
            [Op::Next { from: Some(from), to: None, .. }]
                if from == "11111111-0000-0000-0000-000000000001"
        ));
    }

    #[test]
    fn pause_and_seek_in_a_channel_change_it_for_everyone() {
        let mut app = in_channel("live-channel-pause");
        assert!(app.live_intercept(&Action::TogglePlay));
        assert!(!app.live.my_channel().unwrap().playing);
        assert!(matches!(
            ops(&app).as_slice(),
            [Op::Playback { playing: false, .. }]
        ));
        assert!(app.live_intercept(&Action::Seek(42_000)));
        assert_eq!(app.live.my_channel().unwrap().position_ms, 42_000);
        assert!(matches!(
            ops(&app).as_slice(),
            [Op::Playback {
                position_ms: 42_000,
                ..
            }]
        ));
    }

    #[test]
    fn add_to_queue_in_a_channel_goes_to_the_shared_queue() {
        let mut app = in_channel("live-channel-add");
        let action = Action::AddToQueue {
            uri: "spotify:track:abc".into(),
            label: "Abc".into(),
        };
        assert!(app.live_intercept(&action));
        let channel = app.live.my_channel().unwrap();
        assert_eq!(channel.queue.last().unwrap().item.title, "Abc");
        assert_eq!(channel.queue.last().unwrap().added_by, ME);
        assert!(matches!(
            ops(&app).as_slice(),
            [Op::Add {
                place: Place::End,
                ..
            }]
        ));
        assert!(
            app.manual_queue.is_empty(),
            "this player's own queue is untouched"
        );
    }

    #[test]
    fn playing_something_else_leaves_the_channel() {
        let mut app = in_channel("live-channel-leave");
        let action = Action::PlayUris {
            uris: vec!["spotify:track:x".into()],
            index: 0,
        };
        assert!(!app.live_intercept(&action));
        assert_eq!(app.live.session, Session::Alone);
        assert!(matches!(ops(&app).as_slice(), [Op::LeaveChannel(id)] if id == CHANNEL));
    }

    #[test]
    fn a_songs_end_is_reported_once_with_its_entry() {
        let mut app = in_channel("live-channel-end");
        let channel = &mut app.live.snapshot.channels[0];
        channel.position_ms = 181_600;
        channel.at = wall_ms();
        app.live_report_end();
        app.live_report_end();
        assert!(matches!(
            ops(&app).as_slice(),
            [Op::Next { from: Some(from), .. }] if from == "11111111-0000-0000-0000-000000000001"
        ));
    }

    #[test]
    fn nothing_is_shared_until_the_listener_agrees() {
        let mut app = app("live-consent");
        assert_eq!(app.settings.group_sharing, None);
        app.live_publish();
        assert!(ops(&app).is_empty());
        app.live_action(live::Action::SetSharing(false));
        app.live_publish();
        assert!(ops(&app).is_empty());
    }

    #[test]
    fn someone_private_in_a_session_says_they_are_here_but_not_what_plays() {
        let mut app = following("live-private-session");
        app.settings.group_sharing = Some(false);
        app.live_publish();
        let sent = ops(&app);
        assert!(matches!(
            sent.as_slice(),
            [Op::Presence { item: None, following: Some(user), .. }] if user == ANA
        ));
    }

    #[test]
    fn turning_sharing_off_clears_what_was_shared() {
        let mut app = app("live-sharing-off");
        app.live_driver.published = Some(Published {
            url: Some("https://open.spotify.com/track/x".into()),
            playing: true,
            following: None,
            position_ms: 0,
            at: Instant::now(),
        });
        app.settings.group_sharing = Some(false);
        app.live_publish();
        assert!(matches!(ops(&app).as_slice(), [Op::ClearPresence]));
        assert!(app.live_driver.published.is_none());
    }

    #[test]
    fn listening_along_with_someone_who_follows_you_is_refused() {
        let mut app = app("live-follow-loop");
        let mut ana = presence("x");
        ana.following = Some(ME.into());
        app.live.snapshot.presence = vec![
            ana,
            Presence {
                user_id: ME.into(),
                ..presence("x")
            },
        ];
        app.live_action(live::Action::ListenAlong(ANA.into()));
        assert_eq!(app.live.session, Session::Alone);
    }
}
