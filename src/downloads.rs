//! Downloads delegated to the installed Siphon extension helper.
//!
//! No Spotify audio enters this module. Spotify tracks describe a recording
//! to find on YouTube; SoundCloud supplies its original public permalink.

mod client;
mod discovery;
mod framing;
mod matching;

use std::collections::BTreeMap;

use crate::api::models::{PlayableItem, Track};

pub const INSTALL_HELP: &str = "Install the Siphon extension helper to download";
pub const MAX_BATCHES: usize = 3;

#[derive(Clone, Debug)]
pub enum Source {
    SoundCloud {
        url: String,
        title: String,
        all: bool,
    },
    Spotify(Box<Track>),
}

impl Source {
    pub fn from_item(item: &PlayableItem) -> Option<Self> {
        let PlayableItem::Track(track) = item else {
            return None;
        };
        if crate::soundcloud::is_track_uri(&track.uri) {
            return Some(Self::SoundCloud {
                url: crate::soundcloud::permalink(track.external_urls.spotify.as_deref())?.into(),
                title: track.name.clone(),
                all: false,
            });
        }
        (crate::util::uri_kind(&track.uri) == Some("track") && !track.is_local)
            .then(|| Self::Spotify(Box::new(track.clone())))
    }

    pub fn title(&self) -> &str {
        match self {
            Self::SoundCloud { title, .. } => title,
            Self::Spotify(track) => &track.name,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Target {
    Items(Vec<Source>),
    SoundCloudSet {
        uri: String,
        title: String,
    },
    /// Expanded with the existing Spotify request router on the runtime.
    SpotifyContext(String),
}

#[derive(Clone, Debug)]
pub struct Request {
    pub id: u64,
    pub title: String,
    pub target: Target,
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub title: String,
    pub stage: String,
    pub item: usize,
    pub total: usize,
    pub fraction: Option<f32>,
    pub cancelling: bool,
}

impl Progress {
    pub fn queued(title: String) -> Self {
        Self {
            title,
            stage: "queued".into(),
            item: 0,
            total: 0,
            fraction: None,
            cancelling: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub downloaded: usize,
    pub failed: usize,
    pub cancelled: bool,
    pub last_file: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Event {
    Available(bool),
    Progress {
        id: u64,
        progress: Progress,
    },
    ItemFailed {
        id: u64,
        message: String,
    },
    Finished {
        id: u64,
        result: Result<Summary, String>,
    },
}

#[derive(Default)]
pub struct State {
    pub available: bool,
    pub active: BTreeMap<u64, Progress>,
    next_id: u64,
}

impl State {
    pub fn enqueue(&mut self, title: String, target: Target) -> Option<Request> {
        if !self.available || self.active.len() >= MAX_BATCHES {
            return None;
        }
        self.next_id = self.next_id.wrapping_add(1);
        self.active
            .insert(self.next_id, Progress::queued(title.clone()));
        Some(Request {
            id: self.next_id,
            title,
            target,
        })
    }

    pub fn progress(&mut self, id: u64, mut progress: Progress) {
        if let Some(current) = self.active.get_mut(&id) {
            progress.cancelling |= current.cancelling;
            *current = progress;
        }
    }
}

#[cfg(not(test))]
pub(crate) async fn available() -> bool {
    tokio::task::spawn_blocking(discovery::discover)
        .await
        .ok()
        .flatten()
        .is_some()
}

pub(crate) use client::run;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_late_progress_event_cannot_restore_a_finished_or_cancelled_batch() {
        let mut state = State {
            available: true,
            ..Default::default()
        };
        let request = state.enqueue("Song".into(), Target::Items(vec![])).unwrap();
        state.active.get_mut(&request.id).unwrap().cancelling = true;
        state.progress(request.id, Progress::queued("Song".into()));
        assert!(state.active[&request.id].cancelling);
        state.active.remove(&request.id);
        state.progress(request.id, Progress::queued("Song".into()));
        assert!(state.active.is_empty());
    }

    #[test]
    fn missing_helper_and_excess_batches_are_not_queued() {
        let mut state = State::default();
        assert!(
            state
                .enqueue("Song".into(), Target::Items(vec![]))
                .is_none()
        );
        state.available = true;
        for _ in 0..MAX_BATCHES {
            assert!(
                state
                    .enqueue("Song".into(), Target::Items(vec![]))
                    .is_some()
            );
        }
        assert!(
            state
                .enqueue("Song".into(), Target::Items(vec![]))
                .is_none()
        );
    }

    #[test]
    fn soundcloud_uses_its_real_permalink_and_spotify_never_supplies_an_audio_url() {
        let mut track = Track {
            uri: "soundcloud:track:12".into(),
            name: "Song".into(),
            ..Default::default()
        };
        assert!(Source::from_item(&PlayableItem::Track(track.clone())).is_none());
        track.external_urls.spotify = Some("https://soundcloud.com/artist/song".into());
        assert!(
            matches!(Source::from_item(&PlayableItem::Track(track.clone())),
            Some(Source::SoundCloud {url, all: false, ..}) if url == "https://soundcloud.com/artist/song")
        );
        track.uri = "spotify:track:abc".into();
        assert!(matches!(
            Source::from_item(&PlayableItem::Track(track)),
            Some(Source::Spotify(_))
        ));
    }
}
