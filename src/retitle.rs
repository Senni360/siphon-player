//! Cleaner names for uploads, picked quietly by Jev through the group.
//!
//! SoundCloud and YouTube titles often carry the artist, a producer credit
//! and tags such as "[FREE DL]" or "(Official Video)". The group service
//! offers readings of such a title as artist and song and lets Jev pick the
//! right one; a weak pick changes nothing. Picked readings are remembered on
//! this computer and applied whenever the upload is built again, so a row
//! never changes while it is on screen.
//!
//! Only group members ask, since the group's service holds the key. Titles
//! and uploader names are all that leave this computer.

use std::collections::{HashMap, HashSet};
use std::sync::{OnceLock, RwLock};

use serde::{Deserialize, Serialize};

use crate::api::models::{ArtistRef, Track};

/// How many uploads go in one request.
pub const BATCH: usize = 50;
/// How many readings this computer keeps.
const KEPT: usize = 5000;
const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reading {
    pub artist: String,
    pub title: String,
}

/// An upload whose title the service is asked to read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Upload {
    #[serde(skip)]
    pub uri: String,
    pub title: String,
    pub uploader: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    version: u32,
    /// By upload URI; None when no reading was confident enough.
    readings: Vec<(String, Option<Reading>)>,
}

#[derive(Default)]
struct Readings {
    known: HashMap<String, Option<Reading>>,
    /// Oldest first, for dropping the oldest beyond [`KEPT`].
    order: Vec<String>,
    pending: Vec<Upload>,
    asked: HashSet<String>,
    dirty: bool,
}

fn readings() -> &'static RwLock<Readings> {
    static READINGS: OnceLock<RwLock<Readings>> = OnceLock::new();
    READINGS.get_or_init(Default::default)
}

const SEPARATORS: [&str; 6] = [" - ", " – ", " — ", " | ", " // ", " ~ "];
const NOISE: [&str; 14] = [
    "official",
    "video",
    "audio",
    "lyric",
    "visualizer",
    "visualiser",
    "free dl",
    "free download",
    "prod.",
    "prod ",
    "out now",
    "premiere",
    "hq",
    "4k",
];

/// Whether a title probably carries more than the song's name.
pub fn looks_messy(title: &str) -> bool {
    let lower = title.to_lowercase();
    SEPARATORS.iter().any(|s| lower.contains(s))
        || lower
            .split(['(', '[', '{'])
            .skip(1)
            .any(|inside| NOISE.iter().any(|word| inside.contains(word)))
}

/// Applies a remembered reading to an upload just built from SoundCloud or
/// YouTube, or queues a messy title to be read.
pub fn apply(track: &mut Track) {
    let reading = {
        let readings = readings().read().unwrap_or_else(|p| p.into_inner());
        readings.known.get(&track.uri).cloned()
    };
    match reading {
        Some(Some(reading)) => rename(track, &reading),
        Some(None) => {}
        None if looks_messy(&track.name) => {
            let mut readings = readings().write().unwrap_or_else(|p| p.into_inner());
            if readings.asked.insert(track.uri.clone()) {
                readings.pending.push(Upload {
                    uri: track.uri.clone(),
                    title: track.name.clone(),
                    uploader: track
                        .artists
                        .first()
                        .map(|a| a.name.clone())
                        .unwrap_or_default(),
                });
            }
        }
        None => {}
    }
}

fn rename(track: &mut Track, reading: &Reading) {
    track.name.clone_from(&reading.title);
    let same = |a: &ArtistRef| a.name.eq_ignore_ascii_case(&reading.artist);
    if track.artists.iter().any(same) {
        return;
    }
    match track.artists.first_mut() {
        // A YouTube channel name links nowhere: the performer replaces it.
        Some(first) if first.uri.is_none() => first.name.clone_from(&reading.artist),
        // A SoundCloud uploader links to their profile, which stays theirs:
        // the performer is named first, without a link.
        _ => track.artists.insert(
            0,
            ArtistRef {
                name: reading.artist.clone(),
                ..Default::default()
            },
        ),
    }
}

/// Up to [`BATCH`] uploads waiting to be read.
pub fn take_pending() -> Vec<Upload> {
    let mut readings = readings().write().unwrap_or_else(|p| p.into_inner());
    let count = readings.pending.len().min(BATCH);
    readings.pending.drain(..count).collect()
}

pub fn has_pending() -> bool {
    !readings()
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .pending
        .is_empty()
}

/// Remembers the service's answers, in the order the uploads were sent.
/// Unanswered uploads may be asked again later.
pub fn record(uploads: Vec<Upload>, answers: Option<Vec<Option<Reading>>>) {
    let mut readings = readings().write().unwrap_or_else(|p| p.into_inner());
    let Some(answers) = answers.filter(|a| a.len() == uploads.len()) else {
        for upload in uploads {
            readings.asked.remove(&upload.uri);
        }
        return;
    };
    for (upload, reading) in uploads.into_iter().zip(answers) {
        let reading = reading.filter(valid);
        if readings.known.insert(upload.uri.clone(), reading).is_none() {
            readings.order.push(upload.uri);
        }
    }
    let excess = readings.order.len().saturating_sub(KEPT);
    let dropped: Vec<String> = readings.order.drain(..excess).collect();
    for uri in dropped {
        readings.known.remove(&uri);
    }
    readings.dirty = true;
}

fn valid(reading: &Reading) -> bool {
    let fine = |text: &str, max: usize| {
        !text.trim().is_empty()
            && text.chars().count() <= max
            && !text.chars().any(char::is_control)
    };
    fine(&reading.artist, 200) && fine(&reading.title, 300)
}

/// Reads remembered readings, once at startup. A missing, corrupt or
/// older-format file is ignored.
pub fn load(bytes: &[u8]) {
    let Ok(saved) = serde_json::from_slice::<Saved>(bytes) else {
        return;
    };
    if saved.version != VERSION {
        return;
    }
    let mut readings = readings().write().unwrap_or_else(|p| p.into_inner());
    for (uri, reading) in saved.readings.into_iter().take(KEPT) {
        if reading.as_ref().is_none_or(valid)
            && readings.known.insert(uri.clone(), reading).is_none()
        {
            readings.order.push(uri);
        }
    }
}

/// The file's new contents, when something changed since the last save.
pub fn take_snapshot() -> Option<Vec<u8>> {
    let mut readings = readings().write().unwrap_or_else(|p| p.into_inner());
    if !readings.dirty {
        return None;
    }
    readings.dirty = false;
    let saved = Saved {
        version: VERSION,
        readings: readings
            .order
            .iter()
            .filter_map(|uri| Some((uri.clone(), readings.known.get(uri)?.clone())))
            .collect(),
    };
    serde_json::to_vec(&saved).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload(uri: &str, title: &str, artist: ArtistRef) -> Track {
        Track {
            uri: uri.into(),
            name: title.into(),
            artists: vec![artist],
            ..Default::default()
        }
    }

    fn channel(name: &str) -> ArtistRef {
        ArtistRef {
            name: name.into(),
            ..Default::default()
        }
    }

    fn profile(name: &str) -> ArtistRef {
        ArtistRef {
            id: Some("1".into()),
            name: name.into(),
            uri: Some("soundcloud:user:1".into()),
        }
    }

    // One test: the readings are process-wide, and these URIs are its own.
    #[test]
    fn messy_titles_are_read_once_and_applied_when_built_again() {
        assert!(looks_messy("DJ Shadow - Midnight [FREE DL]"));
        assert!(looks_messy("Song (Official Video)"));
        assert!(!looks_messy("Song (Bicep Remix)"));
        assert!(!looks_messy("Rock-a-bye"));

        let yt = "youtube:video:retitleTest";
        let sc = "soundcloud:track:9100001";
        let clean = "soundcloud:track:9100002";
        let mut track = upload(
            yt,
            "DJ Shadow - Midnight (Official Video)",
            channel("fan99"),
        );
        apply(&mut track);
        apply(&mut upload(
            sc,
            "Song | Artist [FREE DL]",
            profile("uploader"),
        ));
        apply(&mut upload(clean, "Song", profile("uploader")));
        // Unchanged until read, and asked about once.
        assert_eq!(track.name, "DJ Shadow - Midnight (Official Video)");
        apply(&mut track.clone());
        let mut pending = take_pending();
        pending.retain(|u| u.uri == yt || u.uri == sc || u.uri == clean);
        let uris: Vec<&str> = pending.iter().map(|u| u.uri.as_str()).collect();
        assert_eq!(uris, [yt, sc]);
        assert_eq!(pending[0].uploader, "fan99");

        // A failed request may ask again; an answer is remembered.
        record(pending.clone(), None);
        apply(&mut track.clone());
        assert!(take_pending().iter().any(|u| u.uri == yt));
        let reading = |artist: &str, title: &str| {
            Some(Reading {
                artist: artist.into(),
                title: title.into(),
            })
        };
        record(
            pending,
            Some(vec![
                reading("DJ Shadow", "Midnight"),
                reading("Artist", "Song"),
            ]),
        );

        let mut track = upload(
            yt,
            "DJ Shadow - Midnight (Official Video)",
            channel("fan99"),
        );
        apply(&mut track);
        assert_eq!(track.name, "Midnight");
        assert_eq!(track.artists.len(), 1);
        assert_eq!(track.artists[0].name, "DJ Shadow");

        // A SoundCloud profile keeps its link; the performer leads, unlinked.
        let mut track = upload(sc, "Song | Artist [FREE DL]", profile("uploader"));
        apply(&mut track);
        assert_eq!(track.name, "Song");
        let names: Vec<&str> = track.artists.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Artist", "uploader"]);
        assert_eq!(track.artists[0].uri, None);
        assert_eq!(track.artists[1].uri.as_deref(), Some("soundcloud:user:1"));

        // Saved and read back.
        let saved = take_snapshot().expect("changed");
        assert!(String::from_utf8_lossy(&saved).contains("DJ Shadow"));
        assert_eq!(take_snapshot(), None);
        load(&saved);
        load(b"not json");
    }
}
