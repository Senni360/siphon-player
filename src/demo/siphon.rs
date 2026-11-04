//! Deterministic, invented backend fixtures for the native frontend handoff.
//! No accounts, helper processes, network reads, or file writes are performed.

use crate::api::models::{Artist, ArtistRef, ExternalUrls, Track};
use crate::credits::{Credit, Credits};
use crate::friends::{Feed, Model, NewPost, Post, Profile, Reply, Source};
use crate::lyrics::{Line, Lyrics};
use crate::model::Loadable;
use crate::soundcloud::following;

pub const SPOTIFY_URI: &str = "spotify:track:0000000000000000000001";
pub const SOUNDCLOUD_URI: &str = "soundcloud:track:424242";
pub const YOUTUBE_URI: &str = "youtube:video:DEMOvideo01";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    Ready,
    Empty,
    Loading,
    Error,
    Unavailable,
    LongTitles,
}

impl Scenario {
    pub const ALL: [Self; 6] = [
        Self::Ready,
        Self::Empty,
        Self::Loading,
        Self::Error,
        Self::Unavailable,
        Self::LongTitles,
    ];

    pub fn from_flag(flag: &str) -> Option<Self> {
        match flag {
            "siphon-ready" => Some(Self::Ready),
            "siphon-empty" => Some(Self::Empty),
            "siphon-loading" => Some(Self::Loading),
            "siphon-error" => Some(Self::Error),
            "siphon-unavailable" => Some(Self::Unavailable),
            "siphon-long-titles" => Some(Self::LongTitles),
            _ => None,
        }
    }
}

/// These are real application model types filled with explicitly fictional data.
/// A failed read and a successful unavailable result remain distinct.
pub struct Fixture {
    pub tracks: Vec<Track>,
    pub credits: Vec<(String, Loadable<Option<Credits>>)>,
    pub lyrics: Loadable<Option<Lyrics>>,
    pub friends: Loadable<Feed>,
    pub following: Loadable<following::Snapshot>,
}

pub fn fixture(scenario: Scenario) -> Fixture {
    let tracks = if scenario == Scenario::Empty {
        Vec::new()
    } else {
        [
            (SPOTIFY_URI, "Night Bus", "Demo Ensemble", "https://open.spotify.com/track/0000000000000000000001"),
            (SOUNDCLOUD_URI, "Unfinished Light (Private Sketch)", "Demo Producer", "https://soundcloud.com/demo-producer/unfinished-light/s-DEMOONLY"),
            (YOUTUBE_URI, "Window Seat [Instrumental]", "Demo Beat Channel", "https://www.youtube.com/watch?v=DEMOvideo01"),
        ].into_iter().enumerate().map(|(index, (uri, title, artist, url))| Track {
            uri: uri.into(),
            id: uri.rsplit(':').next().map(str::to_owned),
            name: if scenario == Scenario::LongTitles {
                format!("{title} (Extended Studio Version, Alternate Arrangement, Unmastered Session Recording, Take 14)")
            } else { title.into() },
            artists: vec![ArtistRef { name: artist.into(), ..Default::default() }],
            duration_ms: 180_000 + index as u32 * 12_000,
            release_date: Some("2026-10-01".into()),
            play_count: (index != 0).then_some(1_234),
            external_urls: ExternalUrls { spotify: Some(url.into()) },
            is_playable: Some(scenario != Scenario::Unavailable),
            ..Default::default()
        }).collect()
    };
    let credits = tracks
        .iter()
        .map(|track| {
            let state = match scenario {
                Scenario::Loading => Loadable::Loading,
                Scenario::Error => {
                    Loadable::Failed("Demo: credits request failed. Try again.".into())
                }
                Scenario::Unavailable => Loadable::Loaded(None),
                _ if track.uri == YOUTUBE_URI => Loadable::Loaded(None),
                _ => Loadable::Loaded(Some(Credits {
                    contributors: vec![Credit {
                        name: "Demo Producer".into(),
                        role: Some(
                            if track.uri == SOUNDCLOUD_URI {
                                "Writer / Composer"
                            } else {
                                "Producer"
                            }
                            .into(),
                        ),
                        group: Some("Production".into()),
                        artist_uri: None,
                    }],
                    sources: vec!["Invented demo metadata".into()],
                })),
            };
            (track.uri.clone(), state)
        })
        .collect();
    let lyrics = match scenario {
        Scenario::Loading => Loadable::Loading,
        Scenario::Error => Loadable::Failed("Demo: lyrics request failed. Try again.".into()),
        Scenario::Empty | Scenario::Unavailable => Loadable::Loaded(None),
        _ => Loadable::Loaded(Some(Lyrics {
            lines: vec![Line {
                at_ms: Some(0),
                text: "A fictional line for the native demo".into(),
            }],
            synced: true,
            instrumental: false,
        })),
    };
    let posts = tracks
        .iter()
        .enumerate()
        .map(|(index, track)| Post {
            id: format!("demo-post-{index}"),
            author_id: "demo-friend".into(),
            author_name: "Demo Friend".into(),
            created_at: 1_793_361_600_000,
            like_count: 0,
            liked: false,
            affinity: None,
            item: NewPost {
                url: track.external_urls.spotify.clone().unwrap(),
                source: [Source::Spotify, Source::Soundcloud, Source::Youtube][index],
                title: track.name.clone(),
                artists: track.artists.iter().map(|a| a.name.clone()).collect(),
                artwork_url: None,
                duration_ms: track.duration_ms,
                note: "Fictional sharing example".into(),
                uri: None,
                to: Vec::new(),
            },
            replies: Vec::new(),
        })
        .collect();
    let friends = match scenario {
        Scenario::Loading => Loadable::Loading,
        Scenario::Error => {
            Loadable::Failed("Demo: friends service is unavailable. Retry is available.".into())
        }
        Scenario::Unavailable => Loadable::NotLoaded,
        _ => Loadable::Loaded(Feed {
            posts,
            next_cursor: None,
            members: None,
            ..Feed::default()
        }),
    };
    let following = match scenario {
        Scenario::Loading => Loadable::Loading,
        Scenario::Error => Loadable::Failed("Demo: SoundCloud following request failed.".into()),
        Scenario::Unavailable => Loadable::NotLoaded,
        _ => Loadable::Loaded(following::Snapshot {
            items: tracks
                .iter()
                .filter(|track| track.uri == SOUNDCLOUD_URI)
                .map(|track| following::Item {
                    kind: following::Kind::Reposted,
                    by: Artist {
                        name: "Demo Followed Producer".into(),
                        uri: "soundcloud:user:424245".into(),
                        ..Default::default()
                    },
                    created_at: "2026-10-01T12:00:00Z".into(),
                    content: following::Content::Track(Box::new(track.clone())),
                })
                .collect(),
            next: None,
            loaded: true,
        }),
    };
    Fixture {
        tracks,
        credits,
        lyrics,
        friends,
        following,
    }
}

/// The demo's group: four invented friends sharing songs from the main
/// demo's catalogue, with a thread and one song sent only to the viewer.
/// `songs` are (url, title, artist, artwork) in the order they were shared,
/// newest first.
pub fn group(songs: &[(String, String, String, Option<String>)], now_ms: u64) -> Model {
    const VIEWER: &str = "00000000-0000-4000-8000-000000000001";
    const PEOPLE: [(&str, &str); 3] = [
        ("00000000-0000-4000-8000-000000000002", "Maya"),
        ("00000000-0000-4000-8000-000000000003", "Jonas"),
        ("00000000-0000-4000-8000-000000000004", "Teo"),
    ];
    let notes = [
        "that bassline at 1:12 is going straight into a flip",
        "",
        "slowed this down 8% and it sits perfect at 92",
        "for the sunday session",
        "",
        "chops on this are unreal",
    ];
    let minute = 60_000;
    let ages = [4, 38, 95, 60 * 5, 60 * 26, 60 * 50];
    let mut model = Model::default();
    model.restored = true;
    model.profile = Some(Profile {
        user_id: VIEWER.into(),
        display_name: "You".into(),
    });
    model.members = std::iter::once((VIEWER, "You"))
        .chain(PEOPLE)
        .map(|(id, name)| Profile {
            user_id: id.into(),
            display_name: name.into(),
        })
        .collect();
    model.posts = songs
        .iter()
        .take(ages.len())
        .enumerate()
        .map(|(index, (url, title, artist, artwork))| {
            let (author_id, author_name) = if index == 3 {
                (VIEWER, "You")
            } else {
                PEOPLE[index % PEOPLE.len()]
            };
            let created_at = now_ms.saturating_sub(ages[index] * minute);
            let reply = |n: u64, who: usize, text: &str| Reply {
                id: format!("00000000-0000-4000-9000-{:012}", index as u64 * 10 + n),
                author_id: if who == 9 { VIEWER } else { PEOPLE[who].0 }.into(),
                author_name: if who == 9 { "You" } else { PEOPLE[who].1 }.into(),
                created_at: created_at + (n + 1) * 3 * minute,
                text: text.into(),
            };
            Post {
                id: format!("00000000-0000-4000-a000-{index:012}"),
                author_id: author_id.into(),
                author_name: author_name.into(),
                created_at,
                like_count: [2, 0, 3, 1, 0, 1][index],
                liked: index == 2,
                replies: match index {
                    0 => vec![
                        reply(0, 1, "send stems when you have them"),
                        reply(1, 9, "same, want to try it at 140"),
                    ],
                    2 => vec![reply(0, 0, "what key is this in")],
                    _ => Vec::new(),
                },
                affinity: None,
                item: NewPost {
                    url: url.clone(),
                    source: Source::Spotify,
                    title: title.clone(),
                    artists: vec![artist.clone()],
                    artwork_url: artwork.clone(),
                    duration_ms: 200_000,
                    note: notes[index].into(),
                    uri: None,
                    // Jonas sent the second one to the viewer only.
                    to: if index == 1 {
                        vec![VIEWER.into()]
                    } else {
                        Vec::new()
                    },
                },
            }
        })
        .collect();
    model
}

/// Populate existing surfaces without choosing a new layout or visual system.
pub fn apply(app: &mut crate::app::App, scenario: Scenario) {
    let fixture = fixture(scenario);
    app.soundcloud_library.reset();
    app.soundcloud_account = crate::soundcloud::account::Account::Disconnected;
    if scenario != Scenario::Unavailable {
        super::soundcloud_profile(app);
        if let Some(user) = app.soundcloud_account.user().cloned() {
            app.soundcloud_account = crate::soundcloud::account::Account::Session {
                user,
                token: crate::soundcloud::account::SessionToken::parse(
                    "2-000000-dummy_session_token",
                )
                .unwrap(),
            };
        }
        app.soundcloud_library.snapshot.reposts.loaded = matches!(
            scenario,
            Scenario::Ready | Scenario::Empty | Scenario::LongTitles
        );
    }
    let youtube_likes = match scenario {
        Scenario::Loading => Loadable::Loading,
        Scenario::Error => Loadable::Failed("Demo: YouTube library could not be read.".into()),
        Scenario::Unavailable => Loadable::NotLoaded,
        _ => Loadable::Loaded(crate::youtube::likes::Likes {
            tracks: fixture
                .tracks
                .iter()
                .filter(|track| crate::youtube::is_track_uri(&track.uri))
                .cloned()
                .collect(),
            ..Default::default()
        }),
    };
    app.set_demo_youtube_likes(youtube_likes);
    app.credits.clear();
    for (uri, state) in fixture.credits {
        let request = app.credits.begin(&uri).unwrap();
        match state {
            Loadable::Loaded(value) => app.credits.finish(&uri, request, Ok(value)),
            Loadable::Failed(error) => app.credits.finish(&uri, request, Err(error)),
            _ => {}
        }
    }
    if let Some(track) = fixture.tracks.first() {
        app.install_demo_track_details(track, fixture.lyrics);
    } else {
        app.lyrics_uri = None;
        app.lyrics = fixture.lyrics;
    }
    let mut friends = Model::default();
    friends.restored = true;
    if scenario != Scenario::Unavailable {
        friends.profile = Some(Profile {
            user_id: "demo-viewer".into(),
            display_name: "Demo Listener".into(),
        });
    }
    match fixture.friends {
        Loadable::Loaded(feed) => {
            friends.posts = feed.posts;
            friends.next_cursor = feed.next_cursor;
        }
        Loadable::Loading => friends.busy = true,
        Loadable::Failed(error) => friends.error = Some(error),
        Loadable::NotLoaded => {}
    }
    app.friends_feed = friends;
    app.soundcloud_library.following.status = match fixture.following {
        Loadable::Loaded(snapshot) => {
            app.soundcloud_library.snapshot.following = snapshot;
            following::Status::Ready
        }
        Loadable::Loading => following::Status::Loading,
        Loadable::Failed(error) => following::Status::Error(error),
        Loadable::NotLoaded => following::Status::ConnectSoundCloud,
    };
    app.mixed_playlists
        .playlists
        .retain(|p| p.id != "demo-siphon");
    app.mixed_playlists
        .playlists
        .push(crate::mixed_playlists::Playlist {
            id: "demo-siphon".into(),
            name: "Encore backend fixtures".into(),
            description: "Invented offline data for frontend development".into(),
            created: 1_793_361_600,
            modified: 1_793_361_600,
            entries: fixture
                .tracks
                .into_iter()
                .enumerate()
                .map(|(index, track)| crate::mixed_playlists::Entry {
                    id: format!("demo-siphon-row-{index}"),
                    permalink: track.external_urls.spotify.clone(),
                    track,
                })
                .collect(),
        });
    app.open(crate::model::Page::MixedPlaylist("demo-siphon".into()));
}

/// Sample friends for the Group page's Listening now.
#[cfg(feature = "demo")]
pub(super) fn live(app: &mut crate::app::App, joined: bool) {
    use crate::friends::live::{Channel, Entry, Presence, Session, Snapshot};

    let song = |id: &str, title: &str, artist: &str, source: Source| NewPost {
        url: match source {
            Source::Spotify => format!("https://open.spotify.com/track/{id}"),
            Source::Soundcloud => format!("https://soundcloud.com/demo/{id}"),
            Source::Youtube => "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_owned(),
        },
        source,
        title: title.to_owned(),
        artists: vec![artist.to_owned()],
        artwork_url: None,
        duration_ms: 214_000,
        note: String::new(),
        uri: None,
        to: Vec::new(),
    };
    let me = app
        .friends_feed
        .profile
        .get_or_insert_with(|| Profile {
            user_id: "demo-viewer".into(),
            display_name: "Demo Listener".into(),
        })
        .user_id
        .clone();
    app.friends_feed.members = [
        ("ana", "Ana"),
        ("bo", "Bo"),
        ("cleo", "Cleo"),
        ("dev", "Dev"),
    ]
    .into_iter()
    .map(|(id, name)| Profile {
        user_id: id.into(),
        display_name: name.into(),
    })
    .chain(std::iter::once(Profile {
        user_id: me.clone(),
        display_name: "Demo Listener".into(),
    }))
    .collect();
    let presence =
        |user: &str, item: Option<NewPost>, playing: bool, following: Option<&str>| Presence {
            user_id: user.into(),
            item,
            position_ms: 42_000,
            playing,
            at: 0,
            following: following.map(str::to_owned),
        };
    let entry = |id: &str, item: NewPost, by: &str| Entry {
        id: id.into(),
        item,
        added_by: by.into(),
    };
    let mut members = vec!["cleo".to_owned(), "dev".to_owned()];
    if joined {
        members.push(me.clone());
    }
    app.live.snapshot = Snapshot {
        now: 0,
        presence: vec![
            presence(
                "ana",
                Some(song(
                    "a1",
                    "Midnight In A Perfect World",
                    "DJ Shadow",
                    Source::Spotify,
                )),
                true,
                None,
            ),
            presence(
                "bo",
                Some(song(
                    "b1",
                    "Late Night Tales (Unreleased Mix)",
                    "Moonlit Sessions",
                    Source::Soundcloud,
                )),
                false,
                None,
            ),
            presence(
                "cleo",
                Some(song("c1", "Teardrop", "Massive Attack", Source::Spotify)),
                true,
                None,
            ),
            presence(
                "dev",
                Some(song("c1", "Teardrop", "Massive Attack", Source::Spotify)),
                true,
                None,
            ),
        ],
        channels: vec![Channel {
            id: "friday".into(),
            name: "Friday night".into(),
            host_id: "cleo".into(),
            created_at: 0,
            members,
            current: Some(entry(
                "e0",
                song("c1", "Teardrop", "Massive Attack", Source::Spotify),
                "cleo",
            )),
            position_ms: 42_000,
            playing: true,
            at: 0,
            queue: vec![
                entry(
                    "e1",
                    song("q1", "Roygbiv", "Boards of Canada", Source::Spotify),
                    "dev",
                ),
                entry(
                    "e2",
                    song(
                        "q2",
                        "Warehouse Rework",
                        "Unknown Artist",
                        Source::Soundcloud,
                    ),
                    "cleo",
                ),
                entry(
                    "e3",
                    song("q3", "Porcelain", "Moby", Source::Spotify),
                    "dev",
                ),
            ],
            locked: true,
            controllers: vec![],
            skip_requests: vec![],
            background: None,
        }],
    };
    if joined {
        app.settings.group_sharing = Some(true);
        app.live.session = Session::Channel("friday".into());
    }
    let me = app
        .friends_feed
        .profile
        .as_ref()
        .map_or_else(String::new, |profile| profile.user_id.clone());
    let members: Vec<serde_json::Value> = app
        .friends_feed
        .members
        .iter()
        .map(|member| {
            let owner = member.user_id == me;
            serde_json::json!({
                "user_id": member.user_id,
                "display_name": member.display_name,
                "role": if owner { "owner" } else { "member" },
                "spotify_linked": true,
            })
        })
        .collect();
    app.friends_feed.account = serde_json::from_value(serde_json::json!({
        "user_id": me,
        "display_name": "You",
        "spotify_id": "demo-listener",
        "devices": [
            { "id": "d1", "name": "DESKTOP", "created_at": 0, "current": true },
            { "id": "d2", "name": "Studio laptop", "created_at": 0 },
        ],
        "groups": [
            {
                "id": "g1", "name": "Friends", "owner_id": me, "role": "owner",
                "rights": { "share": true, "host": true, "queue": true, "invite": true },
                "members": members, "invite": "demo-invite-code", "pending": ["newcomer"],
            },
            { "id": "g2", "name": "Studio", "role": "member", "members": [] },
        ],
    }))
    .ok();
    crate::friends::account::set_current_group(Some("g1".into()));
    let shared = |id: u8, title: &str, artist: &str, by: &str, by_name: &str, seconds: u32| {
        crate::shared_songs::SharedSong {
            id: format!("00000000-0000-4000-8000-0000000000{id:02}"),
            title: title.into(),
            artists: vec![artist.into()],
            duration_ms: seconds * 1000,
            size: u64::from(seconds) * 40_000,
            by: by.into(),
            by_name: by_name.into(),
            created_at: 0,
        }
    };
    app.friends_feed.shared_songs = vec![
        shared(1, "basement take 3", "Maya", "maya", "Maya", 184),
        shared(
            2,
            "Glass Ceiling (rough mix)",
            "Jonas",
            "jonas",
            "Jonas",
            241,
        ),
        shared(3, "untitled loop 92bpm", "", "teo", "Teo", 96),
        shared(4, "Bonobo flip (rough)", "Dev", "dev", "Dev", 203),
    ];
    app.friends_feed.songs_used = 412 * 1024 * 1024;
    app.friends_feed.songs_capacity = 900 * 1024 * 1024;
}

/// A season of sample plays, and friends who share theirs.
#[cfg(feature = "demo")]
pub(super) fn stats(app: &mut crate::app::App) {
    use crate::friends::live::MemberStats;
    use crate::stats::{Genres, Period, Play, PlayArtist, Summary, Tab, Top, report};
    live(app, false);
    app.settings.group_sharing = Some(true);
    app.stats.tab = Tab::Stats;
    app.stats.period = Period::AllTime;
    let now = jiff::Zoned::now();
    let songs = [
        (
            "Midnight In A Perfect World",
            "DJ Shadow",
            "Endtroducing",
            "trip hop",
        ),
        ("Teardrop", "Massive Attack", "Mezzanine", "trip hop"),
        (
            "Roygbiv",
            "Boards of Canada",
            "Music Has the Right to Children",
            "idm",
        ),
        ("Porcelain", "Moby", "Play", "downtempo"),
        ("Rosewood", "Bonobo", "Fragments", "downtempo"),
        ("Windowlicker", "Aphex Twin", "Windowlicker", "idm"),
        ("Angel", "Massive Attack", "Mezzanine", "trip hop"),
        ("Kerala", "Bonobo", "Migration", "downtempo"),
    ];
    let mut genres = Genres::default();
    let mut plays = Vec::new();
    for day in 0..420i64 {
        for slot in 0..(1 + (day * 7 % 9)) {
            let (title, artist, album, genre) = songs[((day * 3 + slot * 5) % 8) as usize];
            let id = format!("demo-{artist}");
            genres.artists.insert(id.clone(), vec![genre.to_owned()]);
            plays.push(Play {
                at: now.timestamp().as_second() - day * 86_400 - slot * 3_700,
                ms: 215_000,
                uri: format!("spotify:track:demo{}", title.len()),
                title: title.to_owned(),
                artists: vec![PlayArtist {
                    id: Some(id),
                    name: artist.to_owned(),
                }],
                album: album.to_owned(),
                art: None,
                genre: None,
                estimated: day < 3 && slot == 0,
            });
        }
    }
    let mine = report(plays, Period::AllTime, &genres, &now);
    let friend = |user: &str, minutes: u64, artists: &[&str]| MemberStats {
        user_id: user.to_owned(),
        summary: Summary {
            minutes,
            plays: minutes / 3,
            artists: artists
                .iter()
                .map(|name| Top {
                    name: (*name).to_owned(),
                    minutes: minutes / artists.len() as u64,
                    plays: 10,
                    ..Top::default()
                })
                .collect(),
            songs: vec![Top {
                name: "Teardrop".into(),
                detail: "Massive Attack".into(),
                plays: 44,
                minutes: 170,
                ..Top::default()
            }],
            genres: vec![Top {
                name: "trip hop".into(),
                minutes: minutes / 2,
                plays: 44,
                ..Top::default()
            }],
            hours: (0..24).map(|hour| (hour * 7 % 24) as u64).collect(),
            month: String::new(),
        },
    };
    let mut shared = vec![
        friend("ana", 9_400, &["Massive Attack", "Bonobo", "Burial"]),
        friend("bo", 4_100, &["Burial", "Four Tet"]),
    ];
    let viewer = app
        .friends_feed
        .profile
        .as_ref()
        .map_or_else(String::new, |profile| profile.user_id.clone());
    shared.push(MemberStats {
        user_id: viewer,
        summary: mine.summary(),
    });
    app.live.group_stats.insert("all", shared);
    app.stats.reports.insert(Period::AllTime, mine);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixtures_keep_errors_empty_and_unsupported_results_distinct() {
        let ready = fixture(Scenario::Ready);
        assert_eq!(ready.tracks.len(), 3);
        assert!(matches!(ready.credits[2].1, Loadable::Loaded(None)));
        assert!(matches!(
            fixture(Scenario::Loading).credits[0].1,
            Loadable::Loading
        ));
        assert!(matches!(
            fixture(Scenario::Error).lyrics,
            Loadable::Failed(_)
        ));
        assert!(matches!(
            fixture(Scenario::Unavailable).friends,
            Loadable::NotLoaded
        ));
        assert!(fixture(Scenario::Empty).tracks.is_empty());
        assert!(
            fixture(Scenario::Unavailable)
                .tracks
                .iter()
                .all(|track| track.is_playable == Some(false))
        );
    }

    #[test]
    fn fixture_tracks_round_trip_with_stable_identity_and_optional_metadata() {
        for scenario in Scenario::ALL {
            let tracks = fixture(scenario).tracks;
            let encoded = serde_json::to_string(&tracks).unwrap();
            let restored: Vec<Track> = serde_json::from_str(&encoded).unwrap();
            assert_eq!(tracks, restored);
            assert_eq!(tracks, fixture(scenario).tracks);
        }
    }
}
