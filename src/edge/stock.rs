//! Making the phone's playlists ready on the edge before the phone asks.
//!
//! A phone plays a Spotify song as the same song from SoundCloud or YouTube.
//! The edge streams a SoundCloud copy itself, but YouTube refuses the edge's
//! server, so a YouTube copy only plays there once this computer has put it
//! there. While this computer is on and the edge is enabled, every Spotify song
//! of the synced playlists gets a match from the current matcher, and every
//! YouTube match is fetched and uploaded, so the whole playlist plays on the
//! phone with this computer off.
//!
//! One song at a time with a rest between, on the backend runtime, so it never
//! competes with playback. Nothing here is shown or retried in a loop: a song
//! with no copy is a debug line, and the next one follows. Switching the edge
//! off ends it at once.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;

use super::Request;
use crate::api::models::{Album, ArtistRef, Track};
use crate::downloads::matching::Searcher;
use crate::downloads::saved::{Saved, SavedMatch};

/// At most this many songs are worked on in one run of the app. A larger
/// library carries on at the next start.
pub(crate) const MOST_PER_SESSION: usize = 1000;

/// The rest between two songs.
const GAP: Duration = Duration::from_secs(4);

/// How often to look whether the previous fetch has finished.
const FETCH_LOOK: Duration = Duration::from_secs(5);

/// A song of a synced playlist, as this computer knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Song {
    pub uri: String,
    pub name: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: u32,
}

impl Song {
    /// What the matcher is asked for, as the phone asks for it.
    fn wanted(&self) -> Track {
        Track {
            uri: self.uri.clone(),
            name: self.name.clone(),
            artists: self
                .artists
                .iter()
                .map(|name| ArtistRef {
                    name: name.clone(),
                    ..Default::default()
                })
                .collect(),
            album: self.album.as_ref().map(|name| Album {
                name: name.clone(),
                ..Default::default()
            }),
            duration_ms: self.duration_ms,
            ..Default::default()
        }
    }
}

/// One thing to do for the playlists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// No match the current matcher found: look for one.
    Match(Song),
    /// A YouTube copy: put it on the edge, unless it went this session.
    Fetch(String),
}

/// What is worth doing, in order: songs without a trusted match first, then the
/// YouTube copies not offered to the edge this session. A match saved before
/// matches were checked is looked for again once. `uploads` gates the fetches,
/// which are uploads; `remaining` is what this run of the app may still take.
pub(crate) fn plan(
    songs: &[Song],
    saved: &BTreeMap<String, SavedMatch>,
    sent: &HashSet<String>,
    uploads: bool,
    remaining: usize,
) -> Vec<Step> {
    if remaining == 0 {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    let songs: Vec<&Song> = songs
        .iter()
        .filter(|song| song.uri.starts_with("spotify:track:") && song.duration_ms > 0)
        .filter(|song| seen.insert(song.uri.as_str()))
        .collect();
    let trusted = |song: &Song| saved.get(&song.uri).filter(|found| found.trusted());
    let mut steps: Vec<Step> = songs
        .iter()
        .filter(|song| trusted(song).is_none())
        .map(|song| Step::Match((*song).clone()))
        .collect();
    if uploads {
        let mut offered = HashSet::new();
        for song in &songs {
            let Some(found) = trusted(song) else {
                continue;
            };
            if crate::youtube::is_track_uri(&found.uri)
                && !sent.contains(&found.uri)
                && offered.insert(found.uri.clone())
            {
                steps.push(Step::Fetch(found.uri.clone()));
            }
        }
    }
    steps.truncate(remaining);
    steps
}

/// The Spotify songs of the mixed playlists in the synced document. A Spotify
/// playlist's own rows are not in it: the edge reads those from Spotify.
pub(crate) fn songs_in(document: &Value) -> Vec<Song> {
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    document
        .get("mixed")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|list| list.get("tracks").and_then(Value::as_array))
        .flatten()
        .filter_map(|entry| {
            let track = entry.get("track").unwrap_or(entry);
            let uri = text(track.get("uri"))?;
            uri.starts_with("spotify:track:").then(|| Song {
                uri,
                name: text(track.get("name")).unwrap_or_default(),
                artists: track
                    .get("artists")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|artist| text(artist.get("name")))
                    .collect(),
                album: text(track.pointer("/album/name")),
                duration_ms: track
                    .get("duration_ms")
                    .and_then(Value::as_u64)
                    .map_or(0, |ms| u32::try_from(ms).unwrap_or(u32::MAX)),
            })
        })
        .collect()
}

/// Waits until no fetch is still downloading: the edge runtime downloads one
/// song at a time and drops a request that arrives meanwhile.
async fn wait_for_fetches(fetches: &std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>) {
    loop {
        let busy = fetches
            .lock()
            .map(|held| held.iter().any(|task| !task.is_finished()))
            .unwrap_or(false);
        if !busy {
            return;
        }
        tokio::time::sleep(FETCH_LOOK).await;
    }
}

/// Works through the newest synced playlists, one song at a time. A newer
/// snapshot replaces the one being worked through. Ends with the runtime.
pub(crate) async fn run(
    mut snapshots: tokio::sync::mpsc::UnboundedReceiver<Vec<Song>>,
    searcher: Searcher,
    saved: Saved,
    tx: tokio::sync::mpsc::UnboundedSender<Request>,
    uploads: Arc<AtomicBool>,
    fetches: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
) {
    let mut done = 0usize;
    let mut sent: HashSet<String> = HashSet::new();
    let mut looked: HashSet<String> = HashSet::new();
    let mut songs = match snapshots.recv().await {
        Some(songs) => songs,
        None => return,
    };
    loop {
        // A song already looked for this session is not looked for again,
        // found or not: the next start tries once more.
        let open: Vec<Song> = songs
            .iter()
            .filter(|song| !looked.contains(&song.uri))
            .cloned()
            .collect();
        let steps = plan(
            &open,
            &saved.all().await,
            &sent,
            uploads.load(Ordering::Relaxed),
            MOST_PER_SESSION.saturating_sub(done),
        );
        let Some(step) = steps.into_iter().next() else {
            // Everything is ready: wait for the playlists to change.
            match snapshots.recv().await {
                Some(newer) => {
                    songs = newer;
                    continue;
                }
                None => return,
            }
        };
        done += 1;
        match step {
            Step::Match(song) => {
                looked.insert(song.uri.clone());
                match searcher.same_song(&song.wanted()).await {
                    Some(found) => {
                        saved.remember(&song.uri, &found).await;
                        let _ = tx.send(Request::Matches);
                    }
                    // A title, never a link: a private one must not reach a log.
                    None => log::debug!(
                        "edge stock: no copy of {} by {}",
                        song.name,
                        song.artists.join(", ")
                    ),
                }
            }
            Step::Fetch(uri) => {
                sent.insert(uri.clone());
                wait_for_fetches(&fetches).await;
                let _ = tx.send(Request::Fetch(vec![uri]));
            }
        }
        tokio::time::sleep(GAP).await;
        // Take the newest playlists before the next song.
        while let Ok(newer) = snapshots.try_recv() {
            songs = newer;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(uri: &str, name: &str) -> Song {
        Song {
            uri: uri.into(),
            name: name.into(),
            artists: vec!["slayr".into()],
            album: None,
            duration_ms: 120_000,
        }
    }

    fn found(uri: &str, checked: u32) -> SavedMatch {
        SavedMatch {
            uri: uri.into(),
            checked,
            ..Default::default()
        }
    }

    #[test]
    fn songs_without_a_trusted_match_come_first_then_unsent_youtube_copies() {
        let songs = [
            song("spotify:track:a", "Never Go Down"),
            song("spotify:track:b", "DIFFERENT DAY"),
            song("spotify:track:c", "Toxic"),
            song("spotify:track:d", "Holding"),
            song("soundcloud:track:9", "Not Spotify"),
            song("spotify:track:a", "Never Go Down"),
        ];
        let checked = crate::downloads::saved::CHECKED;
        let saved = BTreeMap::from([
            (
                "spotify:track:a".to_owned(),
                found("youtube:video:aaaaaaaaaaa", checked),
            ),
            (
                "spotify:track:b".to_owned(),
                found("youtube:video:bbbbbbbbbbb", checked),
            ),
            // Saved before matches were checked: looked for again.
            ("spotify:track:c".to_owned(), found("soundcloud:track:5", 0)),
            (
                "spotify:track:d".to_owned(),
                found("soundcloud:track:6", checked),
            ),
        ]);
        let sent = HashSet::from(["youtube:video:bbbbbbbbbbb".to_owned()]);
        assert_eq!(
            plan(&songs, &saved, &sent, true, 1000),
            vec![
                Step::Match(song("spotify:track:c", "Toxic")),
                Step::Fetch("youtube:video:aaaaaaaaaaa".into()),
            ]
        );
        // Uploads off: matches still, fetches never.
        assert_eq!(
            plan(&songs, &saved, &sent, false, 1000),
            vec![Step::Match(song("spotify:track:c", "Toxic"))]
        );
    }

    #[test]
    fn a_run_takes_only_what_is_left_of_its_share() {
        let songs: Vec<Song> = (0..10)
            .map(|index| song(&format!("spotify:track:{index}"), "Song"))
            .collect();
        let saved = BTreeMap::new();
        let sent = HashSet::new();
        assert_eq!(plan(&songs, &saved, &sent, true, 4).len(), 4);
        assert!(plan(&songs, &saved, &sent, true, 0).is_empty());
        // A song with no length cannot be matched and is left out.
        let mut lengthless = song("spotify:track:x", "No length");
        lengthless.duration_ms = 0;
        assert!(plan(&[lengthless], &saved, &sent, true, 10).is_empty());
    }

    #[test]
    fn the_songs_are_the_spotify_rows_of_the_mixed_playlists() {
        let document = serde_json::json!({
            "playlists": [{ "uri": "spotify:playlist:x", "name": "From Spotify" }],
            "mixed": [{
                "tracks": [
                    { "id": "e1", "track": {
                        "uri": "spotify:track:a", "name": "Never Go Down",
                        "artists": [{ "name": "slayr" }], "duration_ms": 120_000,
                        "album": { "images": [] } } },
                    { "id": "e2", "track": {
                        "uri": "spotify:local:A:B:C:180", "name": "Local",
                        "artists": [{ "name": "A" }], "duration_ms": 180_000 } },
                    { "id": "e3", "track": {
                        "uri": "soundcloud:track:7", "name": "Cloud" } },
                ],
            }],
        });
        assert_eq!(
            songs_in(&document),
            vec![song("spotify:track:a", "Never Go Down")]
        );
        assert!(songs_in(&serde_json::json!({})).is_empty());
    }
}
