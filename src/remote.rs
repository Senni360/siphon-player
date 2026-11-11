//! Encore on a phone uses this computer: the same accounts, library and queue. The
//! phone leaves commands with the group service, this computer picks them up and
//! answers them, and it plays the sound. Every song stays with its own source, and no
//! sign-in ever leaves this computer: only songs, names and pictures do.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::api::models::{PlayableItem, Track, pick_image};

/// With no phone there, how often to look whether one has turned up.
pub const IDLE_POLL: Duration = Duration::from_secs(15);
/// With a phone watching and no event stream, how quickly its commands are picked up.
pub const WATCHED_POLL: Duration = Duration::from_secs(1);
/// The least time between telling the phone two changes. A change is told at
/// once otherwise: the phone hears it over its event stream within moments.
pub const STATE_EVERY: Duration = Duration::from_millis(150);
/// How often to look for a change when nothing else wakes the app.
pub const STATE_LOOK: Duration = Duration::from_millis(1000);
/// How often it's told regardless, which is how it knows this computer is there.
pub const STATE_AT_LEAST: Duration = Duration::from_secs(8);
/// How long an answer that has to load first is worked on.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(40);
const ART_PIXELS: u32 = 300;
/// The most songs in one answer: they all pass through the service.
pub const MOST_SONGS: usize = 200;

/// One thing the phone asked for.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Incoming {
    pub id: String,
    #[serde(flatten)]
    pub command: Command,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    Toggle,
    Next,
    Previous,
    Seek {
        ms: u32,
    },
    Volume {
        percent: u8,
    },
    Shuffle,
    Repeat,
    Like {
        uri: String,
    },
    Play {
        uris: Vec<String>,
        index: u32,
    },
    PlayContext {
        uri: String,
    },
    Queue {
        uri: String,
    },
    /// A song someone shared with the group, by its link.
    PlayLink {
        url: String,
    },
    Home,
    Lyrics,
    Library,
    Playlist {
        #[serde(rename = "target")]
        id: String,
    },
    Album {
        #[serde(rename = "target")]
        id: String,
    },
    /// A mixed playlist, which lives only on this computer.
    Mixed {
        #[serde(rename = "target")]
        id: String,
    },
    Liked,
    SoundcloudLikes,
    YoutubeLikes,
    Artist {
        #[serde(rename = "target")]
        id: String,
    },
    Source {
        uri: String,
    },
    Search {
        query: String,
    },
    /// A song for this computer to download. It comes with what it's called, so a
    /// Spotify song can be found, and with its own address where it has one.
    Download {
        uri: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        artists: Vec<String>,
        #[serde(default)]
        duration_ms: u32,
        #[serde(default)]
        link: Option<String>,
    },
    /// Where a song's audio is, for the phone to play. A Spotify song comes with
    /// what it's called, so the same song can be found on YouTube or SoundCloud.
    Stream {
        uri: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        artists: Vec<String>,
        #[serde(default)]
        duration_ms: Option<u32>,
    },
}

/// Whether a phone may have this computer play or change `uri`. Anything else,
/// such as a link or a local path, isn't something it can ask for.
pub fn allowed_uri(uri: &str) -> bool {
    uri.len() <= 200
        && !uri.contains(['/', '\\', '?', '#', ' '])
        && (crate::youtube::is_external(uri)
            || [
                "spotify:track:",
                "spotify:episode:",
                "spotify:album:",
                "spotify:playlist:",
                "spotify:artist:",
            ]
            .iter()
            .any(|prefix| uri.starts_with(prefix)))
}

/// A song as the phone shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Song {
    pub uri: String,
    pub name: String,
    pub artists: Vec<String>,
    /// The Spotify artist to open, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    pub duration_ms: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plays: Option<u64>,
    /// The song's own address, to share it with the group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

impl From<&Track> for Song {
    fn from(track: &Track) -> Self {
        Self {
            uri: track.uri.clone(),
            name: track.name.clone(),
            artists: track
                .artists
                .iter()
                .map(|artist| artist.name.clone())
                .collect(),
            artist_id: track
                .artists
                .iter()
                .find_map(|artist| artist.uri.as_deref()?.strip_prefix("spotify:artist:"))
                .map(str::to_owned),
            image: track
                .album
                .as_ref()
                .and_then(|album| pick_image(&album.images, ART_PIXELS))
                .map(str::to_owned),
            duration_ms: track.duration_ms,
            plays: track.play_count,
            link: track.external_urls.spotify.clone(),
        }
    }
}

pub fn songs<'a>(tracks: impl IntoIterator<Item = &'a Track>) -> Vec<Song> {
    tracks
        .into_iter()
        .take(MOST_SONGS)
        .map(Song::from)
        .collect()
}

pub fn song_of(item: &PlayableItem) -> Option<Song> {
    match item {
        PlayableItem::Track(track) => Some(Song::from(track)),
        _ => None,
    }
}

/// What this computer does for a phone beyond playing. A phone only offers what
/// is named here, so one linked to an older Encore never asks for it.
pub const CAN: &[&str] = &["download"];

/// How playback is going, for the phone.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub can: &'static [&'static str],
    pub song: Option<Song>,
    pub liked: bool,
    pub position_ms: u32,
    pub playing: bool,
    pub loading: bool,
    pub shuffle: bool,
    pub repeat: &'static str,
    pub volume: u8,
    /// Where the sound is, when that's somewhere else.
    pub device: Option<String>,
    pub next: Vec<Song>,
}

impl Snapshot {
    /// Whether the phone would see anything new, apart from the clock moving on as
    /// expected. A jump of a couple of seconds or more means someone sought.
    pub fn differs(&self, sent: &Self, since: Duration) -> bool {
        let mut same = self.clone();
        same.position_ms = sent.position_ms;
        if same != *sent {
            return true;
        }
        let expected = u64::from(sent.position_ms)
            + if sent.playing {
                since.as_millis() as u64
            } else {
                0
            };
        u64::from(self.position_ms).abs_diff(expected) > 2_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_read_the_way_the_phone_sends_them() {
        let read = |text: &str| serde_json::from_str::<Incoming>(text).unwrap().command;
        let id = r#""id":"1""#;
        assert_eq!(read(&format!(r#"{{{id},"kind":"next"}}"#)), Command::Next);
        assert_eq!(
            read(&format!(r#"{{{id},"kind":"seek","ms":5}}"#)),
            Command::Seek { ms: 5 }
        );
        assert_eq!(
            read(&format!(r#"{{{id},"kind":"play","uris":["a"],"index":0}}"#)),
            Command::Play {
                uris: vec!["a".into()],
                index: 0
            }
        );
        assert_eq!(
            read(&format!(r#"{{{id},"kind":"soundcloud_likes"}}"#)),
            Command::SoundcloudLikes
        );
        // The message has an id of its own, so what it's about is called something else.
        let artist: Incoming =
            serde_json::from_str(r#"{"id":"1","kind":"artist","target":"art0"}"#).unwrap();
        assert_eq!(artist.id, "1");
        assert_eq!(artist.command, Command::Artist { id: "art0".into() });
        assert_eq!(
            read(&format!(
                r#"{{{id},"kind":"stream","uri":"soundcloud:track:7"}}"#
            )),
            Command::Stream {
                uri: "soundcloud:track:7".into(),
                name: None,
                artists: Vec::new(),
                duration_ms: None,
            }
        );
        assert!(serde_json::from_str::<Incoming>(r#"{"id":"1","kind":"format_disk"}"#).is_err());
        assert!(serde_json::from_str::<Incoming>(r#"{"id":"1","kind":"seek"}"#).is_err());
    }

    #[test]
    fn a_phone_can_only_ask_for_songs_and_pages_of_the_sources() {
        assert!(allowed_uri("spotify:track:abc"));
        assert!(allowed_uri("spotify:playlist:abc"));
        assert!(allowed_uri("soundcloud:track:12"));
        assert!(allowed_uri("youtube:video:aB9_-012xyz"));
        assert!(!allowed_uri("https://evil.example/x"));
        assert!(!allowed_uri("C:\\Windows\\song.mp3"));
        assert!(!allowed_uri("spotify:user:abc"));
        assert!(!allowed_uri("local:track"));
        assert!(!allowed_uri(&format!("spotify:track:{}", "a".repeat(300))));
    }

    #[test]
    fn only_a_real_change_is_worth_telling_the_phone() {
        let sent = Snapshot {
            position_ms: 10_000,
            playing: true,
            ..Default::default()
        };
        let later = |position_ms| Snapshot {
            position_ms,
            ..sent.clone()
        };
        let second = Duration::from_secs(1);
        assert!(!later(11_000).differs(&sent, second), "the clock moving on");
        assert!(later(60_000).differs(&sent, second), "someone sought");
        let paused = Snapshot {
            playing: false,
            ..later(11_000)
        };
        assert!(paused.differs(&sent, second));
    }

    #[test]
    fn a_song_for_the_phone_carries_only_what_it_shows() {
        let track = Track {
            name: "Song".into(),
            uri: "spotify:track:a".into(),
            artists: vec![crate::api::models::ArtistRef {
                id: Some("b".into()),
                name: "Artist".into(),
                uri: Some("spotify:artist:b".into()),
            }],
            ..Default::default()
        };
        let song = Song::from(&track);
        assert_eq!(song.artists, ["Artist"]);
        assert_eq!(song.artist_id.as_deref(), Some("b"));
        let text = serde_json::to_string(&song).unwrap();
        assert!(!text.contains("external"));
    }
}
