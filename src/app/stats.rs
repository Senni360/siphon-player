//! Keeps the listening log, and the Group page's statistics, up to date.
use std::collections::VecDeque;

use super::*;
use crate::friends::live::Op;
use crate::stats::{Lookup, Period, Play, PlayArtist, Report, Tab};

/// How long a report stands before the page asks for a newer one.
const REPORT_FRESH: Duration = Duration::from_secs(60);
/// How often Spotify is asked what played on other devices. It keeps 50 plays.
const RECENTS_EVERY: Duration = Duration::from_secs(30 * 60);
/// How often the group is told this listener's totals.
const SHARE_EVERY: Duration = Duration::from_secs(60 * 60);
/// How often the group's totals are read while looking at them.
const GROUP_READ_EVERY: Duration = Duration::from_secs(60);
/// One genre question at a time: the batch request is gone from Spotify's API.
const LOOKUP_EVERY: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct View {
    pub tab: Tab,
    pub period: Period,
    pub reports: HashMap<Period, Report>,
    pub importing: bool,
    asked: HashMap<Period, Instant>,
    lookups: VecDeque<Lookup>,
    last_lookup: Option<Instant>,
    looked_up_any: bool,
    last_recents: Option<Instant>,
    last_share: Option<Instant>,
    last_group_read: Option<Instant>,
    shared_since: HashMap<Period, bool>,
}

fn played_at(text: &str) -> Option<i64> {
    text.parse::<jiff::Timestamp>()
        .ok()
        .map(jiff::Timestamp::as_second)
}

impl App {
    /// What a song playing here will be logged as, when it has finished.
    pub(super) fn stats_play_of(&self, now: &NowPlaying) -> Option<Play> {
        if now.is_episode || now.title.trim().is_empty() {
            return None;
        }
        Some(Play {
            at: 0,
            ms: now.duration_ms,
            uri: now.uri.clone(),
            title: now.title.clone(),
            artists: now
                .artists
                .iter()
                .map(|artist| PlayArtist {
                    id: artist
                        .id
                        .clone()
                        .filter(|_| now.uri.starts_with("spotify:")),
                    name: artist.name.clone(),
                })
                .collect(),
            album: now.album_name.clone(),
            art: now.art_small.clone().or_else(|| now.art_url.clone()),
            genre: crate::stats::soundcloud_genre(&now.uri),
            estimated: false,
        })
    }

    /// Logs the song that just stopped being the current one, if it was heard
    /// long enough to count. Paused time is not in `listened`.
    pub(super) fn stats_finish_listening(&mut self) {
        let Some(listening) = self.listening.as_mut() else {
            return;
        };
        let heard = listening.listened
            + listening
                .playing_since
                .map(|since| since.elapsed())
                .unwrap_or_default();
        let Some(mut play) = listening.play.clone() else {
            return;
        };
        if heard < crate::history::counts_after(play.ms) {
            return;
        }
        // Counted now, so a later end of the same stretch does not count it again.
        listening.listened = Duration::ZERO;
        if listening.playing_since.is_some() {
            listening.playing_since = Some(Instant::now());
        }
        play.ms = u32::try_from(heard.as_millis()).unwrap_or(u32::MAX);
        play.at = jiff::Timestamp::now().as_second();
        self.backend.send(Command::StatsRecord {
            plays: vec![play],
            check: false,
        });
    }

    /// What Spotify says played lately, which is the only word on other devices.
    pub(super) fn stats_note_recent(&mut self, history: &[crate::api::models::PlayHistory]) {
        let plays: Vec<Play> = history
            .iter()
            .filter_map(|played| {
                let at = played_at(played.played_at.as_deref()?)?;
                let track = &played.track;
                if track.duration_ms == 0 || track.name.trim().is_empty() {
                    return None;
                }
                Some(Play {
                    at,
                    ms: track.duration_ms,
                    uri: track.uri.clone(),
                    title: track.name.clone(),
                    artists: track
                        .artists
                        .iter()
                        .map(|artist| PlayArtist {
                            id: artist.id.clone(),
                            name: artist.name.clone(),
                        })
                        .collect(),
                    album: track
                        .album
                        .as_ref()
                        .map_or_else(String::new, |a| a.name.clone()),
                    art: track
                        .album
                        .as_ref()
                        .and_then(|a| crate::api::models::pick_image(&a.images, 64))
                        .map(str::to_owned),
                    genre: None,
                    estimated: true,
                })
            })
            .collect();
        if !plays.is_empty() {
            self.backend
                .send(Command::StatsRecord { plays, check: true });
        }
    }

    pub(super) fn stats_action(&mut self, action: crate::stats::Action) {
        use crate::stats::Action as Stats;
        match action {
            Stats::Tab(tab) => {
                self.stats.tab = tab;
                if tab == Tab::Stats {
                    self.stats_ask(self.stats.period, false);
                }
            }
            Stats::Period(period) => {
                self.stats.period = period;
                self.stats_ask(period, false);
            }
            Stats::Import => {
                if !self.stats.importing {
                    self.stats.importing = true;
                    self.backend.choose_stats_import();
                }
            }
        }
    }

    fn stats_ask(&mut self, period: Period, again: bool) {
        let fresh = self
            .stats
            .asked
            .get(&period)
            .is_some_and(|at| at.elapsed() < REPORT_FRESH);
        if fresh && !again {
            return;
        }
        self.stats.asked.insert(period, Instant::now());
        self.backend.send(Command::StatsReport { period });
    }

    /// Everything changed: ask for what is on show again.
    fn stats_all_changed(&mut self) {
        self.stats.asked.clear();
        self.stats.last_share = None;
        if self.stats.tab == Tab::Stats {
            self.stats_ask(self.stats.period, true);
        }
    }

    pub(super) fn handle_stats_update(&mut self, update: crate::stats::Update) {
        use crate::stats::Update;
        match update {
            Update::Report(period, report) => {
                if period == self.stats.period {
                    self.stats.lookups = report.lookups.iter().cloned().collect();
                }
                if self.settings.group_sharing == Some(true)
                    && self.friends_feed.profile.is_some()
                    && let Some(key) = period.shared_key()
                    && self.stats.shared_since.remove(&period).unwrap_or(false)
                {
                    let request = self.live.request(Op::PutStats {
                        period: key,
                        summary: Box::new(report.summary()),
                    });
                    self.backend.send(Command::Live(request));
                }
                self.stats.reports.insert(period, *report);
            }
            Update::Imported(Ok(count)) => {
                self.stats.importing = false;
                self.toast(
                    gettext(self.locale, "Added {count} plays from Spotify")
                        .replace("{count}", &count.to_string()),
                );
                self.stats_all_changed();
            }
            Update::Imported(Err(error)) => {
                self.stats.importing = false;
                if !error.is_empty() {
                    self.toast_error(error);
                }
            }
        }
    }

    pub(super) fn stats_artist_genres(&mut self, id: String, genres: Vec<String>) {
        self.backend.send(Command::StatsGenres {
            artists: vec![(id, genres)],
            names: Vec::new(),
        });
        self.stats_after_lookup();
    }

    /// A song names the artists its imported plays could not.
    pub(super) fn stats_song_artists(&mut self, track: &Track) {
        let names: Vec<(String, String)> = track
            .artists
            .iter()
            .filter_map(|artist| Some((artist.name.clone(), artist.id.clone()?)))
            .collect();
        for (_, id) in &names {
            self.stats.lookups.push_front(Lookup::Artist(id.clone()));
        }
        self.backend.send(Command::StatsGenres {
            artists: Vec::new(),
            names,
        });
    }

    fn stats_after_lookup(&mut self) {
        self.stats.looked_up_any = true;
    }

    pub(super) fn stats_cleared(&mut self) {
        self.backend.send(Command::StatsClear);
        self.stats.reports.clear();
        self.stats_all_changed();
        if self.friends_feed.profile.is_some() {
            self.stats_unshare();
        }
    }

    /// Takes this listener's totals away from the group.
    pub(super) fn stats_unshare(&mut self) {
        let request = self.live.request(Op::DeleteStats);
        self.backend.send(Command::Live(request));
        self.stats.last_share = None;
    }

    /// Called every background pass; does little unless the page is on show.
    pub(super) fn stats_tick(&mut self, looking: bool) {
        if self.offline {
            return;
        }
        if self.user.is_some()
            && self
                .stats
                .last_recents
                .is_none_or(|at| at.elapsed() >= RECENTS_EVERY)
        {
            self.stats.last_recents = Some(Instant::now());
            self.backend.api(ApiRequest::RecentlyPlayed {
                who: RecentsFor::Stats,
                generation: 0,
                before: None,
                limit: 50,
            });
        }
        let in_group = self.friends_feed.profile.is_some();
        if in_group
            && self.settings.group_sharing == Some(true)
            && self
                .stats
                .last_share
                .is_none_or(|at| at.elapsed() >= SHARE_EVERY)
        {
            self.stats.last_share = Some(Instant::now());
            for period in [Period::ThisMonth, Period::AllTime] {
                self.stats.shared_since.insert(period, true);
                self.stats_ask(period, true);
            }
        }
        if !looking || self.stats.tab != Tab::Stats {
            return;
        }
        self.stats_ask(self.stats.period, false);
        if in_group
            && let Some(key) = self.stats.period.shared_key()
            && self
                .stats
                .last_group_read
                .is_none_or(|at| at.elapsed() >= GROUP_READ_EVERY)
        {
            self.stats.last_group_read = Some(Instant::now());
            let request = self.live.request(Op::GetStats(key));
            self.backend.send(Command::Live(request));
        }
        if self
            .stats
            .last_lookup
            .is_none_or(|at| at.elapsed() >= LOOKUP_EVERY)
        {
            match self.stats.lookups.pop_front() {
                Some(lookup) => {
                    self.stats.last_lookup = Some(Instant::now());
                    self.backend.api(match lookup {
                        Lookup::Artist(id) => ApiRequest::ArtistGenres { id },
                        Lookup::Song(id) => ApiRequest::StatsSong { id },
                    });
                }
                // Those answers now count in what is on show.
                None if std::mem::take(&mut self.stats.looked_up_any) => {
                    self.stats_ask(self.stats.period, true);
                }
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::test_app;
    use super::*;

    fn now_playing(uri: &str) -> NowPlaying {
        NowPlaying {
            local: true,
            device_name: None,
            device_kind: None,
            uri: uri.into(),
            id: None,
            title: "Song".into(),
            artists: vec![ArtistRef {
                id: Some("artist".into()),
                name: "Ana".into(),
                uri: None,
            }],
            subtitle: String::new(),
            album_name: "Album".into(),
            album_id: None,
            show_id: None,
            art_url: None,
            art_small: None,
            duration_ms: 200_000,
            position_ms: 0,
            playing: true,
            loading: false,
            shuffle: false,
            repeat: RepeatMode::Off,
            volume_percent: 100,
            can_control: true,
            can_set_volume: true,
            is_episode: false,
            resuming: false,
        }
    }

    fn listening(heard: Duration) -> Listening {
        Listening {
            uri: "spotify:track:x".into(),
            listened: heard,
            playing_since: None,
            recorded: false,
            play: None,
        }
    }

    fn logged(app: &App) -> Vec<Play> {
        app.backend
            .take_stats_records()
            .into_iter()
            .flatten()
            .collect()
    }

    #[test]
    fn a_song_heard_long_enough_is_logged_with_the_time_heard() {
        let mut app = test_app("stats-log");
        let mut held = listening(Duration::from_secs(90));
        held.play = app.stats_play_of(&now_playing("spotify:track:x"));
        app.listening = Some(held);
        app.stats_finish_listening();
        let plays = logged(&app);
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].ms, 90_000);
        assert_eq!(plays[0].title, "Song");
        assert_eq!(plays[0].artists[0].id.as_deref(), Some("artist"));
        assert!(!plays[0].estimated);
        assert!(plays[0].at > 0);
    }

    #[test]
    fn a_new_song_from_the_spotify_engine_logs_the_one_it_ended() {
        let mut app = test_app("stats-track-change");
        app.remote = None;
        app.selected_device = None;
        let mut held = listening(Duration::from_secs(90));
        held.play = app.stats_play_of(&now_playing("spotify:track:x"));
        let state = |sequence: u64, uri: &str| crate::player::LocalState {
            connected: true,
            playback: crate::player::Playback::Playing,
            track_sequence: sequence,
            track: Some(crate::player::LocalTrack {
                uri: uri.into(),
                title: "Song".into(),
                duration_ms: 200_000,
                ..Default::default()
            }),
            ..Default::default()
        };
        app.local_ready = true;
        app.handle_local(state(1, "spotify:track:x"));
        app.listening = Some(held);
        app.handle_local(state(2, "spotify:track:y"));
        let plays = logged(&app);
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].uri, "spotify:track:x");
        assert!(!plays[0].estimated);
    }

    #[test]
    fn quitting_logs_the_song_still_playing() {
        let mut app = test_app("stats-quit");
        let mut held = listening(Duration::from_secs(90));
        held.play = app.stats_play_of(&now_playing("spotify:track:x"));
        app.listening = Some(held);
        app.shutdown();
        assert_eq!(logged(&app).len(), 1);
    }

    #[test]
    fn a_skip_is_not_logged() {
        let mut app = test_app("stats-skip");
        let mut held = listening(Duration::from_secs(5));
        held.play = app.stats_play_of(&now_playing("spotify:track:x"));
        app.listening = Some(held);
        app.stats_finish_listening();
        assert!(logged(&app).is_empty());
    }

    #[test]
    fn stopping_twice_counts_once() {
        let mut app = test_app("stats-once");
        let mut held = listening(Duration::from_secs(90));
        held.play = app.stats_play_of(&now_playing("spotify:track:x"));
        app.listening = Some(held);
        app.stats_finish_listening();
        app.stats_finish_listening();
        assert_eq!(logged(&app).len(), 1);
    }

    #[test]
    fn an_episode_is_not_a_song() {
        let app = test_app("stats-episode");
        let mut episode = now_playing("spotify:episode:x");
        episode.is_episode = true;
        assert_eq!(app.stats_play_of(&episode), None);
    }

    #[test]
    fn spotifys_list_is_logged_as_estimates_to_be_checked_against_the_log() {
        let mut app = test_app("stats-recent");
        let played = crate::api::models::PlayHistory {
            track: Track {
                uri: "spotify:track:y".into(),
                name: "Elsewhere".into(),
                duration_ms: 180_000,
                ..Track::default()
            },
            played_at: Some("2026-11-01T10:00:00Z".into()),
            context: None,
        };
        app.stats_note_recent(std::slice::from_ref(&played));
        let plays = logged(&app);
        assert_eq!(plays.len(), 1);
        assert!(plays[0].estimated);
        assert_eq!(plays[0].ms, 180_000);
    }

    #[test]
    fn totals_reach_the_group_only_with_sharing_on() {
        let mut app = test_app("stats-share");
        app.friends_feed.profile = Some(crate::friends::Profile {
            user_id: "me".into(),
            display_name: "Me".into(),
        });
        let tick_ops = |app: &mut App| {
            app.stats.shared_since.insert(Period::ThisMonth, true);
            app.handle_stats_update(crate::stats::Update::Report(
                Period::ThisMonth,
                Box::default(),
            ));
            app.backend.take_live_requests()
        };
        assert!(tick_ops(&mut app).is_empty());
        app.settings.group_sharing = Some(false);
        assert!(tick_ops(&mut app).is_empty());
        app.settings.group_sharing = Some(true);
        let sent = tick_ops(&mut app);
        assert!(matches!(
            sent.as_slice(),
            [crate::friends::live::Request {
                op: Op::PutStats {
                    period: "month",
                    ..
                },
                ..
            }]
        ));
    }
}
