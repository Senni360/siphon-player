//! Deterministic, invented backend fixtures for the native frontend handoff.
//! No accounts, helper processes, network reads, or file writes are performed.

use crate::api::models::{Artist, ArtistRef, ExternalUrls, Track};
use crate::credits::{Credit, Credits};
use crate::friends::{Feed, Model, NewPost, Post, Profile, Source};
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
                        role: Some("Producer".into()),
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
            item: NewPost {
                url: track.external_urls.spotify.clone().unwrap(),
                source: [Source::Spotify, Source::Soundcloud, Source::Youtube][index],
                title: track.name.clone(),
                artists: track.artists.iter().map(|a| a.name.clone()).collect(),
                artwork_url: None,
                duration_ms: track.duration_ms,
                note: "Fictional sharing example".into(),
            },
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
                    content: following::Content::Track(track.clone()),
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

/// Populate existing surfaces without choosing a new layout or visual system.
pub fn apply(app: &mut crate::app::App, scenario: Scenario) {
    let fixture = fixture(scenario);
    app.credits.clear();
    for (uri, state) in fixture.credits {
        let request = app.credits.begin(&uri).unwrap();
        match state {
            Loadable::Loaded(value) => app.credits.finish(&uri, request, Ok(value)),
            Loadable::Failed(error) => app.credits.finish(&uri, request, Err(error)),
            _ => {}
        }
    }
    app.lyrics_uri = Some(SPOTIFY_URI.into());
    app.lyrics = fixture.lyrics;
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
