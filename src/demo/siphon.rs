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
            name: "Siphon backend fixtures".into(),
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
