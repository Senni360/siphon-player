//! Account-scoped catalogue snapshots, separate from Spotify's library.
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::api::models::{Artist, Playlist, Track};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub following: super::following::Snapshot,
    pub reposts: super::reposts::Snapshot,
    pub likes: Vec<Track>,
    pub next_likes: Option<String>,
    pub likes_loaded: bool,
    pub playlists: Vec<Playlist>,
    pub artists: Vec<Artist>,
}

#[derive(Default)]
pub struct Library {
    pub following: super::following::State,
    pub repost_edits: super::reposts::Edits,
    pub snapshot: Snapshot,
    pub generation: u64,
    pub revision: u64,
    pub loading: bool,
    pub refreshing: bool,
    pub error: Option<String>,
    pub serial: u64,
    /// Local writes overlay lagging paged reads for this connection.
    pub edits: HashMap<u64, bool>,
    pub pending: HashMap<u64, PendingLike>,
    liked_ids: HashSet<u64>,
}

pub struct PendingLike {
    pub serial: u64,
    pub before: bool,
    pub track: Option<Track>,
    pub index: Option<usize>,
}

impl Library {
    pub fn rebuild_membership(&mut self) {
        self.liked_ids = self
            .snapshot
            .likes
            .iter()
            .filter_map(|track| super::track_id(&track.uri))
            .collect();
    }

    pub fn reset(&mut self) {
        let generation = self.generation.wrapping_add(1);
        let revision = self.revision.wrapping_add(1);
        *self = Self {
            generation,
            revision,
            ..Default::default()
        };
    }

    pub fn is_liked(&self, id: u64) -> bool {
        self.edits
            .get(&id)
            .copied()
            .unwrap_or_else(|| self.liked_ids.contains(&id))
    }

    pub fn absorb_likes(&mut self, tracks: Vec<Track>, next: Option<String>, more: bool) {
        let previous = self.snapshot.likes.clone();
        if !more {
            self.snapshot.likes.clear();
        }
        let mut seen: HashSet<_> = self
            .snapshot
            .likes
            .iter()
            .map(|track| track.uri.clone())
            .collect();
        self.snapshot
            .likes
            .extend(tracks.into_iter().filter(|track| {
                super::track_id(&track.uri).is_some_and(|id| self.edits.get(&id) != Some(&false))
                    && seen.insert(track.uri.clone())
            }));
        for track in previous.into_iter().rev() {
            if super::track_id(&track.uri).is_some_and(|id| self.edits.get(&id) == Some(&true))
                && seen.insert(track.uri.clone())
            {
                self.snapshot.likes.insert(0, track);
            }
        }
        self.snapshot.next_likes = next;
        self.snapshot.likes_loaded = true;
        self.rebuild_membership();
        self.loading = false;
        self.error = None;
        self.revision = self.revision.wrapping_add(1);
    }
}

pub fn cache_key(account: &super::account::Account) -> Option<String> {
    account.user().map(|user| {
        format!(
            "{}-{}",
            user.user_id,
            if account.token().is_some() {
                "session"
            } else {
                "profile"
            }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track(id: u64) -> Track {
        Track {
            uri: super::super::track_uri(id),
            ..Default::default()
        }
    }

    #[test]
    fn likes_pages_deduplicate_and_hold_local_edits() {
        let mut library = Library::default();
        library.absorb_likes(vec![track(1), track(2)], Some("cursor".into()), false);
        assert_eq!(library.snapshot.next_likes.as_deref(), Some("cursor"));
        library.edits.insert(2, false);
        library
            .snapshot
            .likes
            .retain(|track| track.uri != super::super::track_uri(2));
        library.absorb_likes(vec![track(1), track(2), track(3)], None, true);
        assert_eq!(library.snapshot.likes, [track(1), track(3)]);
        assert!(library.snapshot.next_likes.is_none());
        library.edits.insert(3, true);
        library.absorb_likes(vec![track(1)], None, false);
        assert!(library.is_liked(3));
        library.reset();
        assert_eq!(library.generation, 1);
        assert!(library.snapshot.likes.is_empty());
    }

    #[tokio::test]
    async fn library_cache_round_trip_is_account_scoped() {
        let root = std::env::current_dir()
            .unwrap()
            .join(".cache")
            .join(format!("sc-library-{}", std::process::id()));
        let path = root.join("library.json");
        let snapshot = Snapshot {
            reposts: super::super::reposts::Snapshot {
                tracks: [1].into(),
                playlists: [2].into(),
                loaded: true,
            },
            following: super::super::following::Snapshot {
                items: vec![super::super::following::Item {
                    kind: super::super::following::Kind::Reposted,
                    by: Artist {
                        uri: "soundcloud:user:42".into(),
                        ..Default::default()
                    },
                    created_at: "2026-10-30T12:00:00Z".into(),
                    content: super::super::following::Content::Track(track(1).into()),
                }],
                next: Some("stream?cursor=next".into()),
                loaded: true,
            },
            likes: vec![track(1)],
            likes_loaded: true,
            playlists: vec![Playlist {
                uri: "soundcloud:playlist:2".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        crate::account_cache::write(
            &path,
            &crate::account_cache::Cache::new("42-profile".into(), snapshot.clone()),
        )
        .await
        .unwrap();
        assert_eq!(
            crate::account_cache::read::<Snapshot>(&path, "42-profile").await,
            Some(snapshot)
        );
        assert!(
            crate::account_cache::read::<Snapshot>(&path, "42-session")
                .await
                .is_none()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_library_snapshots_default_to_unloaded_social_data() {
        let snapshot: Snapshot =
            serde_json::from_str(r#"{"likes":[],"likes_loaded":true}"#).unwrap();
        assert!(!snapshot.reposts.loaded);
        assert!(!snapshot.following.loaded);
    }
}
