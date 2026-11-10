//! A song's BPM and key worked out on demand, and the state its row shows.
//!
//! The passive reading in [`crate::beat_analysis`] only hears what plays
//! through Encore's own engine. This is the half the interface asks for: a
//! row's BPM and key tool asks for one song, the backend finds that song's
//! audio (a downloaded file, or a SoundCloud, YouTube or group stream),
//! decodes it off the UI thread and answers. One request per song at a time;
//! a late answer for a request that has been replaced is dropped.

use std::collections::HashMap;

use crate::beat_analysis::Reading;
use crate::model::Loadable;

/// How far an analysis has got, for the row's progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Finding the song's audio: a file, or a stream link.
    Finding,
    /// The audio is in hand and being listened to.
    Listening,
}

/// Why Encore cannot hear a song at all, when that is settled without asking
/// the backend. A downloaded file is always worth trying, so callers check
/// for one first.
pub fn unhearable(uri: &str) -> Option<&'static str> {
    if crate::util::uri_kind(uri) == Some("local") {
        return Some("That file isn't in the folder Encore scanned.");
    }
    if uri.starts_with("spotify:") {
        return Some(
            "Spotify plays this song itself, so Encore never hears its audio. \
             Download it, or play a SoundCloud or YouTube version, to measure BPM and key.",
        );
    }
    if crate::shared_songs::is_song(uri) {
        return Some(
            "A song the group shared can be measured once it is downloaded here; \
             downloading it also counts against the group's monthly limit.",
        );
    }
    if crate::soundcloud::is_track_uri(uri) || crate::youtube::is_track_uri(uri) {
        return None;
    }
    Some("Encore can't hear this song's audio to measure BPM and key.")
}

/// Per-song readings and requests. Loading is deduplicated; a failed reading
/// can be asked for again.
#[derive(Default)]
pub struct State {
    next_request: u64,
    entries: HashMap<String, (u64, Loadable<Reading>)>,
    stages: HashMap<String, Stage>,
}

impl State {
    pub fn get(&self, uri: &str) -> Option<&Loadable<Reading>> {
        self.entries.get(uri).map(|(_, state)| state)
    }

    /// The reading when there is one; a failed or running request has none.
    pub fn reading(&self, uri: &str) -> Option<Reading> {
        self.get(uri).and_then(Loadable::get).copied()
    }

    /// The running request's number, for tests and for a late answer to be
    /// matched against.
    pub fn request(&self, uri: &str) -> Option<u64> {
        match self.entries.get(uri) {
            Some((request, Loadable::Loading)) => Some(*request),
            _ => None,
        }
    }

    /// How far the running request has got.
    pub fn stage(&self, uri: &str) -> Option<Stage> {
        self.stages.get(uri).copied()
    }

    /// Starts a request for `uri`, unless one is running or already answered.
    pub fn begin(&mut self, uri: &str) -> Option<u64> {
        if matches!(self.get(uri), Some(Loadable::Loading | Loadable::Loaded(_))) {
            return None;
        }
        self.next_request = self.next_request.wrapping_add(1);
        self.stages.remove(uri);
        self.entries
            .insert(uri.to_owned(), (self.next_request, Loadable::Loading));
        Some(self.next_request)
    }

    pub fn progress(&mut self, uri: &str, request: u64, stage: Stage) {
        if let Some((held, Loadable::Loading)) = self.entries.get(uri)
            && *held == request
        {
            self.stages.insert(uri.to_owned(), stage);
        }
    }

    pub fn finish(&mut self, uri: &str, request: u64, result: Result<Reading, String>) {
        if let Some((held, state)) = self.entries.get_mut(uri)
            && *held == request
        {
            *state = match result {
                Ok(reading) => Loadable::Loaded(reading),
                Err(error) => Loadable::Failed(error),
            };
            self.stages.remove(uri);
        }
    }

    /// Keeps a reading the passive analyser made while the song played, so a
    /// row shows it without asking again.
    pub fn remember(&mut self, uri: &str, reading: Reading) {
        if matches!(self.get(uri), Some(Loadable::Loading | Loadable::Loaded(_))) {
            return;
        }
        self.stages.remove(uri);
        self.entries
            .insert(uri.to_owned(), (0, Loadable::Loaded(reading)));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.stages.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beat_analysis::Key;

    fn reading(bpm: u16) -> Reading {
        Reading {
            bpm,
            key: Some((
                Key {
                    tonic: 9,
                    minor: true,
                },
                0.8,
            )),
        }
    }

    #[test]
    fn one_request_runs_at_a_time_and_a_late_answer_is_dropped() {
        let mut state = State::default();
        let first = state.begin("soundcloud:track:1").unwrap();
        assert!(state.begin("soundcloud:track:1").is_none());
        state.progress("soundcloud:track:1", first, Stage::Listening);
        assert_eq!(state.stage("soundcloud:track:1"), Some(Stage::Listening));
        // A stale answer for an older request changes nothing.
        state.finish("soundcloud:track:1", first + 1, Ok(reading(120)));
        assert!(matches!(
            state.get("soundcloud:track:1"),
            Some(Loadable::Loading)
        ));
        state.finish("soundcloud:track:1", first, Ok(reading(140)));
        assert_eq!(
            state.reading("soundcloud:track:1").map(|r| r.bpm),
            Some(140)
        );
        assert_eq!(state.stage("soundcloud:track:1"), None);
        // Answered songs are not asked for again.
        assert!(state.begin("soundcloud:track:1").is_none());
    }

    #[test]
    fn a_failed_reading_can_be_asked_for_again() {
        let mut state = State::default();
        let first = state.begin("spotify:track:1").unwrap();
        state.finish("spotify:track:1", first, Err("no audio".into()));
        assert!(matches!(
            state.get("spotify:track:1"),
            Some(Loadable::Failed(message)) if message == "no audio"
        ));
        let second = state.begin("spotify:track:1").unwrap();
        assert_ne!(first, second);
        assert!(matches!(
            state.get("spotify:track:1"),
            Some(Loadable::Loading)
        ));
    }

    #[test]
    fn a_reading_heard_while_playing_is_remembered() {
        let mut state = State::default();
        state.remember("soundcloud:track:2", reading(87));
        assert_eq!(state.reading("soundcloud:track:2").map(|r| r.bpm), Some(87));
        assert!(state.begin("soundcloud:track:2").is_none());
        // A request in flight is not overwritten by the passive one.
        let request = state.begin("soundcloud:track:3").unwrap();
        state.remember("soundcloud:track:3", reading(70));
        assert!(matches!(
            state.get("soundcloud:track:3"),
            Some(Loadable::Loading)
        ));
        state.finish("soundcloud:track:3", request, Ok(reading(140)));
        assert_eq!(
            state.reading("soundcloud:track:3").map(|r| r.bpm),
            Some(140)
        );
    }

    #[test]
    fn spotify_without_a_file_says_why_it_cannot_be_heard() {
        let reason = unhearable("spotify:track:4").expect("Spotify is not heard");
        assert!(reason.contains("never hears its audio"), "{reason}");
        assert_eq!(unhearable("soundcloud:track:4"), None);
        assert_eq!(unhearable("youtube:video:dQw4w9WgXcQ"), None);
        assert!(
            unhearable("spotify:local:A:B:C:1")
                .unwrap()
                .contains("folder")
        );
    }
}
