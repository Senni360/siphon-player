//! What the group's account layer asks of the rest of the app: remembering the
//! chosen group, reading a playlist whole to share it or play it in a party,
//! opening a party's link, and telling Discord what plays.
use super::*;
use crate::discord::{self, Party, Status};
use crate::friends::account;
use crate::friends::live::{self, Session};
use crate::friends::{DEFAULT_BASE_URL, NewPost, SongsFor, Source};
use crate::model::GroupPlaylistUse;

/// How often Discord's status is looked at. It only goes out when it changed.
const DISCORD_TICK: Duration = Duration::from_secs(3);
/// A jump this large in where a song should end is a seek, worth saying.
const DISCORD_DRIFT_MS: u64 = 3_000;
/// Longer paused than this and there is nothing to show anymore.
const DISCORD_PAUSED: Duration = Duration::from_secs(5 * 60);
fn wall_ms() -> u64 {
    jiff::Timestamp::now().as_millisecond().max(0) as u64
}

fn songs_of(tracks: Vec<crate::api::models::Track>) -> Vec<NewPost> {
    tracks
        .into_iter()
        .filter_map(|track| {
            NewPost::from_item(&crate::api::models::PlayableItem::Track(track))
                .ok()
                .filter(|song| !song.title.trim().is_empty())
        })
        .take(1000)
        .collect()
}

/// How often a song played before `started`: today and in all that is kept. The
/// play in progress is written down part of the way through, so it can't count
/// itself and change the line halfway.
fn plays_before(
    plays: &[crate::api::models::PlayHistory],
    uri: &str,
    started: jiff::Timestamp,
    midnight: Option<jiff::Timestamp>,
) -> (usize, usize) {
    let (mut today, mut ever) = (0, 0);
    for play in plays.iter().filter(|play| play.track.uri == uri) {
        let Some(at) = play
            .played_at
            .as_deref()
            .and_then(|at| at.parse::<jiff::Timestamp>().ok())
        else {
            continue;
        };
        if at < started {
            ever += 1;
            today += usize::from(midnight.is_some_and(|midnight| at >= midnight));
        }
    }
    (today, ever)
}

/// Whether two statuses say the same, allowing for a clock that moved a little.
fn same_status(a: &Option<Status>, b: &Option<Status>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            let close = |a: Option<u64>, b: Option<u64>| match (a, b) {
                (Some(a), Some(b)) => a.abs_diff(b) <= DISCORD_DRIFT_MS,
                (None, None) => true,
                _ => false,
            };
            close(a.started_ms, b.started_ms)
                && close(a.ends_ms, b.ends_ms)
                && Status {
                    started_ms: None,
                    ends_ms: None,
                    ..a.clone()
                } == Status {
                    started_ms: None,
                    ends_ms: None,
                    ..b.clone()
                }
        }
        _ => false,
    }
}

impl App {
    /// Called with the chosen group from settings, before anything is read.
    pub(super) fn group_restore_choice(&self) {
        account::set_current_group(self.settings.group_current.clone());
    }

    /// Looks at what an answer from the account layer changed.
    pub(super) fn group_after_event(&mut self) {
        let chosen = account::current_group();
        if self.friends_feed.account.is_some() && chosen != self.settings.group_current {
            let had = std::mem::replace(&mut self.settings.group_current, chosen);
            self.mark_settings_dirty();
            // Another group means another room. A party in the old one is over here.
            if had.is_some() {
                self.live_group_changed();
            }
        }
        if let Some((name, songs, purpose)) = self.friends_feed.songs_ready.take() {
            self.group_songs_ready(name, songs, purpose);
        }
    }

    fn group_songs_ready(&mut self, name: String, songs: Vec<NewPost>, purpose: SongsFor) {
        if songs.is_empty() {
            return;
        }
        match purpose {
            SongsFor::PartyBackground => {
                self.apply_background(name, songs);
            }
            SongsFor::SaveCopy => {
                let tracks: Vec<_> = songs.iter().filter_map(NewPost::to_track).collect();
                let id = self.mixed_playlists.import(name, String::new(), tracks);
                self.save_playlist_sources();
                self.open(Page::MixedPlaylist(id));
                self.toast(gettext(
                    self.locale,
                    "Saved to your playlists on this computer",
                ));
            }
            SongsFor::Play => {
                let tracks: Vec<_> = songs.iter().filter_map(NewPost::to_track).collect();
                let id = self.mixed_playlists.import(name, String::new(), tracks);
                self.save_playlist_sources();
                let uri = self
                    .mixed_playlists
                    .get(&id)
                    .map(crate::mixed_playlists::Playlist::uri);
                if let Some(uri) = uri {
                    self.actions.push(Action::PlayContext {
                        uri,
                        offset_uri: None,
                        offset_index: None,
                    });
                }
            }
        }
    }

    fn apply_background(&mut self, name: String, items: Vec<NewPost>) {
        let count = items.len();
        self.live_action(live::Action::SetBackground { name, items });
        self.toast(format!(
            "{count} {}",
            gettext(self.locale, "songs are the party's background now")
        ));
    }

    /// The Spotify playlists in this listener's library, as far as they are known.
    pub(crate) fn library_playlists(&self) -> Vec<crate::api::models::Playlist> {
        self.library.playlists.get().cloned().unwrap_or_default()
    }

    /// Reads the list named by `uri` whole, then does `purpose` with it.
    pub(super) fn group_playlist(&mut self, uri: String, name: String, purpose: GroupPlaylistUse) {
        if let Some(id) = uri.strip_prefix(crate::mixed_playlists::PLAYLIST_PREFIX) {
            let tracks = self
                .mixed_playlists
                .get(id)
                .map(crate::mixed_playlists::Playlist::to_tracks)
                .unwrap_or_default();
            self.group_playlist_ready(name, tracks, purpose);
            return;
        }
        let Some(id) = uri.strip_prefix("spotify:playlist:").map(str::to_owned) else {
            return;
        };
        self.pending_group_playlists
            .insert(id.clone(), (name, purpose));
        self.toast(gettext(self.locale, "Reading the playlist…"));
        self.ensure_loaded(Page::Playlist(id.clone()));
        self.group_playlists_loaded(&id);
        if self.pending_group_playlists.contains_key(&id) {
            self.load_more(Page::Playlist(id));
        }
    }

    /// A page of a playlist arrived. Once it is whole, it goes where it was meant to.
    pub(super) fn group_playlists_loaded(&mut self, id: &str) {
        let complete = self
            .playlist_pages
            .get(id)
            .is_some_and(|page| page.items.is_complete() && page.items.base_offset == 0);
        if !complete {
            return;
        }
        let Some((name, purpose)) = self.pending_group_playlists.remove(id) else {
            return;
        };
        let tracks = self
            .playlist_pages
            .get(id)
            .map(|page| {
                page.items
                    .items
                    .iter()
                    .filter_map(|row| match row.playable() {
                        Some(PlayableItem::Track(track)) => Some(track.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.group_playlist_ready(name, tracks, purpose);
    }

    fn group_playlist_ready(
        &mut self,
        name: String,
        tracks: Vec<crate::api::models::Track>,
        purpose: GroupPlaylistUse,
    ) {
        let items = songs_of(tracks);
        if items.is_empty() {
            self.toast_error(gettext(self.locale, "That playlist has no songs to share"));
            return;
        }
        match purpose {
            GroupPlaylistUse::PartyBackground => self.apply_background(name, items),
            GroupPlaylistUse::Share { to } => {
                let count = items.len();
                self.actions
                    .push(Action::Friends(crate::friends::Action::SharePlaylist {
                        name,
                        items,
                        to,
                    }));
                self.toast(format!(
                    "{count} {}",
                    gettext(self.locale, "songs shared with the group")
                ));
            }
        }
    }

    /// A party's link was opened. It never starts anything by itself: music
    /// starting unasked is exactly what a link must not be able to do.
    pub(super) fn join_party_link(&mut self, group: String, party: String) {
        let found = self.friends_feed.account.as_ref().and_then(|account| {
            account
                .groups
                .iter()
                .find(|each| each.id == group)
                .map(|each| each.name.clone())
        });
        let Some(group_name) = found else {
            self.toast_error(gettext(
                self.locale,
                "You're not in that group. Ask its admin for an invite",
            ));
            return;
        };
        if account::current_group().as_deref() != Some(group.as_str()) {
            self.actions
                .push(Action::Friends(crate::friends::Action::SelectGroup(group)));
        }
        self.open(Page::Group);
        self.dialog = Some(Dialog::Confirm {
            title: gettext(self.locale, "Join this party?").into_owned(),
            message: format!(
                "{} {group_name}. {}",
                gettext(self.locale, "A party in"),
                gettext(
                    self.locale,
                    "Music starts as soon as you join, and only the host controls it."
                )
            ),
            confirm: gettext(self.locale, "Join").into_owned(),
            action: Box::new(Action::Live(live::Action::JoinChannel(party))),
        });
    }

    fn discord_status(&self) -> Option<Status> {
        let now = self.now_playing_live()?;
        let item = self.live_item(&now);
        let private = item
            .as_ref()
            .is_none_or(|item| discord::is_private_link(&item.url));
        let shown = item.filter(|_| !private);
        // A private song says nothing about itself, not even how often it plays.
        let moment = shown
            .as_ref()
            .and_then(|item| self.discord_moment(&now, item));
        let hover = shown.as_ref().and_then(|_| self.discord_hover());
        let (title, artists, artwork_url, link) = match shown {
            Some(item) => {
                let link = match item.source {
                    Source::Spotify => "Open in Spotify",
                    Source::Soundcloud => "Open in SoundCloud",
                    Source::Youtube => "Open on YouTube",
                };
                (
                    item.title,
                    item.artists.join(", "),
                    item.artwork_url
                        .filter(|art| !discord::is_private_link(art)),
                    Some((link, item.url)),
                )
            }
            None => (
                format!("Listening on {}", crate::brand::NAME),
                String::new(),
                None,
                None,
            ),
        };
        let wall = wall_ms();
        let position = u64::from(now.position_ms);
        let started_ms = now.playing.then(|| wall.saturating_sub(position));
        let ends_ms = (now.playing && now.duration_ms > 0)
            .then(|| wall + u64::from(now.duration_ms).saturating_sub(position));
        let base = std::env::var("SIPHON_FRIENDS_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.into());
        // Only for a song the card shows the cover of, so never a private one.
        let badge = artwork_url
            .as_ref()
            .and_then(|_| self.discord_glow(&now))
            .and_then(|glow| discord::lit_badge(&base, glow));
        let party = self.live.my_channel().and_then(|channel| {
            let group = account::current_group()?;
            Some(Party {
                id: channel.id.clone(),
                name: channel.name.clone(),
                size: channel.members.len() as u32,
                join_url: (channel.locked && base.starts_with("https://")).then(|| {
                    format!(
                        "{}/open/party/{group}/{}",
                        base.trim_end_matches('/'),
                        channel.id
                    )
                }),
            })
        });
        let party = party.or_else(|| match &self.live.session {
            Session::Following(user) => {
                let name = self
                    .friends_feed
                    .members
                    .iter()
                    .find(|member| member.user_id == *user)
                    .map_or_else(String::new, |member| member.display_name.clone());
                Some(Party {
                    id: format!("along-{user}"),
                    name: format!("With {name}"),
                    size: 2,
                    join_url: None,
                })
            }
            _ => None,
        });
        Some(Status {
            title,
            artists,
            artwork_url,
            link,
            started_ms,
            ends_ms,
            paused: !now.playing,
            badge,
            hover,
            moment,
            party,
        })
    }

    /// The colour the window's glow has for this song, when it has one.
    fn discord_glow(&self, now: &NowPlaying) -> Option<[u8; 3]> {
        if !self.settings.accent_from_art {
            return None;
        }
        let art = now.art_small.as_ref().or(now.art_url.as_ref())?;
        let glow = crate::ui::glow_color(*self.accents.get(art)?);
        Some([glow.r(), glow.g(), glow.b()])
    }

    /// What makes this song worth a line today, best first. People in the group
    /// are only ever counted: they share with the group, not with Discord.
    fn discord_moment(&self, now: &NowPlaying, item: &NewPost) -> Option<String> {
        let group = account::current_group().and_then(|id| {
            self.friends_feed
                .account
                .as_ref()?
                .groups
                .iter()
                .find(|group| group.id == id)
                .map(|group| group.name.clone())
        });
        let same = |other: &NewPost| {
            other.url == item.url || (other.uri.is_some() && other.uri == item.uri)
        };
        if let Some(group) = &group {
            let me = self.me();
            let others = self
                .live
                .snapshot
                .presence
                .iter()
                .filter(|person| person.playing && Some(&person.user_id) != me.as_ref())
                .filter(|person| person.item.as_ref().is_some_and(same))
                .count();
            match others {
                0 => {}
                1 => return Some(format!("1 other in {group} is on this right now")),
                many => return Some(format!("{many} others in {group} are on this right now")),
            }
        }
        let started =
            jiff::Timestamp::now() - jiff::SignedDuration::from_millis(i64::from(now.position_ms));
        let midnight = jiff::Zoned::now()
            .start_of_day()
            .map(|day| day.timestamp())
            .ok();
        let (today, ever) = plays_before(self.plays.plays(), &now.uri, started, midnight);
        if let Some(line) = discord::encore_line(today, 0) {
            return Some(line);
        }
        if let Some(group) = &group
            && self.friends_feed.posts.iter().any(|post| same(&post.item))
        {
            return Some(format!("Shared in {group}"));
        }
        discord::encore_line(0, ever)
    }

    /// Something no other player's card can say: how much of this month went here.
    fn discord_hover(&self) -> Option<String> {
        let minutes = self
            .stats
            .reports
            .get(&crate::stats::Period::ThisMonth)?
            .minutes;
        (minutes >= 60).then(|| {
            format!(
                "{} hours on {} this month",
                minutes / 60,
                crate::brand::NAME
            )
        })
    }

    /// Tells Discord what plays. `now` skips the wait, for a switch just flipped.
    pub(super) fn discord_tick(&mut self, now: bool) {
        if !now
            && self
                .discord_last
                .as_ref()
                .is_some_and(|(at, _)| at.elapsed() < DISCORD_TICK)
        {
            return;
        }
        let mut status = if self.settings.discord_presence && !self.offline {
            self.discord_status()
        } else {
            None
        };
        // Paused a long while, there is nothing to show.
        if let Some(current) = &status
            && current.paused
        {
            let idle = self.discord_paused_at.get_or_insert_with(Instant::now);
            if idle.elapsed() >= DISCORD_PAUSED {
                status = None;
            }
        } else {
            self.discord_paused_at = None;
        }
        let last = self
            .discord_last
            .as_ref()
            .and_then(|(_, sent)| sent.clone());
        let changed = !same_status(&last, &status);
        if !changed {
            if let Some(entry) = &mut self.discord_last {
                entry.0 = Instant::now();
            }
            return;
        }
        if status.is_some() && !self.settings.discord_notice_seen {
            self.settings.discord_notice_seen = true;
            self.mark_settings_dirty();
            self.toast_with(
                format!(
                    "{} {}",
                    crate::brand::NAME,
                    gettext(
                        self.locale,
                        "now shows your song on your Discord profile while Discord is open"
                    )
                ),
                gettext(self.locale, "Turn off").into_owned(),
                Action::SetDiscordPresence(false),
            );
        }
        self.backend.send(Command::Discord(status.clone()));
        self.discord_last = Some((Instant::now(), status));
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_app;
    use super::*;

    fn status() -> Status {
        Status {
            title: "T".into(),
            artists: "A".into(),
            artwork_url: None,
            link: None,
            started_ms: Some(1_000),
            ends_ms: Some(100_000),
            paused: false,
            badge: None,
            hover: None,
            moment: None,
            party: None,
        }
    }

    #[test]
    fn a_clock_moving_a_little_is_not_news_to_discord_but_a_seek_is() {
        let moved = Status {
            started_ms: Some(2_000),
            ends_ms: Some(101_000),
            ..status()
        };
        assert!(same_status(&Some(status()), &Some(moved)));
        let seeked = Status {
            started_ms: Some(40_000),
            ends_ms: Some(140_000),
            ..status()
        };
        assert!(!same_status(&Some(status()), &Some(seeked)));
        assert!(!same_status(&Some(status()), &None));
        assert!(same_status(&None, &None));
    }

    #[test]
    fn a_song_counts_its_earlier_plays_but_never_the_one_in_progress() {
        let play = |uri: &str, at: &str| crate::api::models::PlayHistory {
            track: crate::api::models::Track {
                uri: uri.into(),
                ..Default::default()
            },
            played_at: Some(at.into()),
            context: None,
        };
        let at = |text: &str| text.parse::<jiff::Timestamp>().unwrap();
        let plays = [
            // Written down thirty seconds into the play in progress.
            play("song", "2026-11-02T20:00:30Z"),
            play("song", "2026-11-02T19:00:00Z"),
            play("song", "2026-11-02T18:00:00Z"),
            play("other", "2026-11-02T17:00:00Z"),
            play("song", "2026-11-01T12:00:00Z"),
        ];
        assert_eq!(
            plays_before(
                &plays,
                "song",
                at("2026-11-02T20:00:00Z"),
                Some(at("2026-11-02T00:00:00Z")),
            ),
            (2, 3)
        );
        assert_eq!(
            plays_before(&plays, "new", at("2026-11-02T20:00:00Z"), None),
            (0, 0)
        );
    }

    #[test]
    fn turning_discord_off_clears_what_it_shows() {
        let mut app = test_app("discord-off");
        app.discord_last = Some((Instant::now(), Some(status())));
        app.settings.discord_presence = false;
        app.discord_tick(true);
        assert!(matches!(app.discord_last, Some((_, None))));
    }

    #[test]
    fn a_party_link_outside_your_groups_starts_nothing() {
        let mut app = test_app("party-link-stranger");
        app.join_party_link(
            "00000000-0000-4000-8000-000000000009".into(),
            "00000000-0000-4000-8000-000000000008".into(),
        );
        assert!(app.dialog.is_none());
        assert_eq!(app.live.session, Session::Alone);
    }

    #[test]
    fn a_party_link_in_your_group_asks_before_any_music_starts() {
        let mut app = test_app("party-link-ask");
        let group = "00000000-0000-4000-8000-000000000009";
        app.friends_feed.account = Some(account::Account {
            user_id: "me".into(),
            display_name: "Me".into(),
            spotify_id: None,
            soundcloud: None,
            devices: vec![],
            groups: serde_json::from_value(serde_json::json!([{ "id": group, "name": "Friends" }]))
                .unwrap(),
        });
        app.join_party_link(group.into(), "00000000-0000-4000-8000-000000000008".into());
        assert!(matches!(app.dialog, Some(Dialog::Confirm { .. })));
        assert_eq!(app.live.session, Session::Alone, "nothing has happened yet");
    }

    #[test]
    fn songs_come_from_tracks_with_a_name_and_a_link() {
        let songs = songs_of(vec![
            crate::api::models::Track {
                uri: "spotify:track:abc".into(),
                name: "Abc".into(),
                ..Default::default()
            },
            crate::api::models::Track {
                uri: "spotify:track:def".into(),
                ..Default::default()
            },
        ]);
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].url, "https://open.spotify.com/track/abc");
    }
}
