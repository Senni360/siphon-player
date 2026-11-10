//! What this computer offers the always-on edge: the library and queue
//! snapshot, playlist local files, YouTube fetches and the sign-in vault.
//!
//! Snapshots are debounced: a change is copied once it has been quiet for a
//! moment, so dragging a playlist into order does not write once per frame.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::Instant;

use super::*;
use crate::backend::Command;
use crate::edge::{self, MixedPlaylist, MixedTrack, PlaylistSummary, UploadJob};

/// A change waits this long before its snapshot is written.
const SETTLE: Duration = Duration::from_secs(3);
/// How often a possibly changed snapshot is looked at.
const LOOK: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct Edge {
    /// The last vault setting handed to the backend.
    pub vault: Option<bool>,
    /// The last uploads setting handed to the backend.
    pub uploads: Option<bool>,
    /// A change waiting for quiet.
    dirty: Option<Instant>,
    /// The snapshot last written.
    signature: Option<u64>,
    /// When the snapshot was last looked at.
    looked: Option<Instant>,
    /// Upload keys already handed over, so the same file waits only once.
    queued: HashSet<String>,
    /// A reset from the edge makes the next look write both documents.
    pub resync: bool,
}

impl App {
    /// Keeps the edge's copy of the library and queue near this computer's,
    /// and offers new local files. Runs on a linked phone only.
    pub(super) fn edge_tick(&mut self) {
        if !self.settings.phone_linked || !self.phone.started {
            return;
        }
        let vault = self.vault_signins_enabled();
        if self.edge.vault != Some(vault) {
            self.edge.vault = Some(vault);
            self.backend
                .send(Command::Edge(edge::Request::Vault(vault)));
        }
        let uploads = self.settings.local_files_on_phone;
        if self.edge.uploads != Some(uploads) {
            self.edge.uploads = Some(uploads);
            self.backend
                .send(Command::Edge(edge::Request::UploadsEnabled(uploads)));
        }
        if uploads {
            self.edge_queue_uploads();
        }
        if self
            .edge
            .looked
            .is_some_and(|looked| looked.elapsed() < LOOK)
        {
            return;
        }
        self.edge.looked = Some(Instant::now());
        let (playlists, queue) = self.edge_documents();
        let signature = edge_signature(&playlists, &queue);
        if self.edge.signature != Some(signature) {
            self.edge.signature = Some(signature);
            self.edge.dirty = Some(Instant::now());
        }
        if self.edge.resync {
            self.edge.resync = false;
            self.edge.dirty = Some(Instant::now() - SETTLE);
        }
        if self
            .edge
            .dirty
            .is_some_and(|dirty| dirty.elapsed() >= SETTLE)
        {
            self.edge.dirty = None;
            self.backend
                .send(Command::Edge(edge::Request::SyncPlaylists(playlists)));
            self.backend
                .send(Command::Edge(edge::Request::SyncQueue(queue)));
        }
    }

    /// The two documents the edge keeps. Mixed playlists carry their songs
    /// because only this computer knows them.
    fn edge_documents(&self) -> (serde_json::Value, serde_json::Value) {
        let spotify: Vec<PlaylistSummary> = self
            .library
            .playlists
            .get()
            .map(|lists| {
                lists
                    .iter()
                    .map(|list| PlaylistSummary {
                        id: list.id.clone(),
                        uri: list.uri.clone(),
                        name: list.name.clone(),
                        image: crate::api::models::pick_image(&list.images, 300).map(str::to_owned),
                        track_count: list.tracks.as_ref().map_or(0, |tracks| tracks.total),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mixed: Vec<MixedPlaylist> = self
            .mixed_playlists
            .playlists
            .iter()
            .map(|list| {
                let image = list.entries.iter().find_map(|entry| {
                    entry
                        .track
                        .album
                        .as_ref()
                        .and_then(|album| crate::api::models::pick_image(&album.images, 300))
                        .map(str::to_owned)
                });
                let tracks = list
                    .entries
                    .iter()
                    .map(|entry| {
                        let uri = entry.track.uri.clone();
                        MixedTrack {
                            id: entry.id.clone(),
                            name: entry.track.name.clone(),
                            artists: entry
                                .track
                                .artists
                                .iter()
                                .map(|artist| artist.name.clone())
                                .collect(),
                            duration_ms: entry.track.duration_ms,
                            image: entry
                                .track
                                .album
                                .as_ref()
                                .and_then(|album| {
                                    crate::api::models::pick_image(&album.images, 300)
                                })
                                .map(str::to_owned),
                            source: uri
                                .starts_with("spotify:local:")
                                .then(|| format!("upload:{}", edge::local_key(&uri))),
                            uri,
                        }
                    })
                    .collect();
                MixedPlaylist {
                    id: list.id.clone(),
                    uri: list.uri(),
                    name: list.name.clone(),
                    image,
                    tracks,
                }
            })
            .collect();
        let snapshot = self.remote_snapshot();
        (
            edge::sync::playlists_doc(&spotify, &mixed),
            edge::sync::queue_doc(
                snapshot.song.as_ref(),
                snapshot.position_ms,
                snapshot.playing,
                &snapshot.next,
            ),
        )
    }

    /// Offers every playlist local file this computer can find, newest first,
    /// once. Old files wait at the back, exactly as the listener expects.
    fn edge_queue_uploads(&mut self) {
        if self.local_files.is_none() {
            return;
        }
        let mut uris: Vec<String> = Vec::new();
        for page in self.playlist_pages.values() {
            for row in &page.items.items {
                if let Some(item) = row.playable() {
                    uris.push(item.uri().to_owned());
                }
            }
        }
        for list in &self.mixed_playlists.playlists {
            for entry in &list.entries {
                uris.push(entry.track.uri.clone());
            }
        }
        let mut seen = HashSet::new();
        let mut files: Vec<(std::time::SystemTime, UploadJob)> = uris
            .into_iter()
            .filter(|uri| uri.starts_with("spotify:local:"))
            .filter(|uri| seen.insert(uri.clone()))
            .filter_map(|uri| {
                let path = self.local_file(&uri)?.to_owned();
                let key = edge::local_key(&uri);
                if self.edge.queued.contains(&key) {
                    return None;
                }
                let modified = std::fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                Some((modified, UploadJob { key, path }))
            })
            .collect();
        // Newest first: the most recently added file should reach the phone
        // first, and an old library finishes last.
        files.sort_by(|left, right| right.0.cmp(&left.0));
        let jobs: Vec<UploadJob> = files
            .into_iter()
            .map(|(_, job)| {
                self.edge.queued.insert(job.key.clone());
                job
            })
            .collect();
        if !jobs.is_empty() {
            self.backend
                .send(Command::Edge(edge::Request::Upload(jobs)));
        }
    }

    /// Whether the listener wants the sign-ins kept in the edge vault. An
    /// unset choice is on for the group's owner and off for everyone else.
    pub(crate) fn vault_signins_enabled(&self) -> bool {
        self.settings
            .vault_signins
            .unwrap_or_else(|| self.is_group_owner())
    }

    /// The owner of the group this listener is in, on their own account.
    fn is_group_owner(&self) -> bool {
        let Some(account) = self.friends_feed.account.as_ref() else {
            return false;
        };
        let Some(group) = account.chosen_group(self.settings.group_current.as_deref()) else {
            return false;
        };
        group.owner_id.as_deref() == Some(account.user_id.as_str())
    }

    pub(super) fn edge_event(&mut self, event: edge::Event) {
        match event {
            edge::Event::Commands(commands) => self.accept_commands(commands),
            edge::Event::Phone(phone) => self.heard_from_phone(phone.map(|phone| (phone, 0))),
            edge::Event::Reset => self.edge.resync = true,
        }
    }
}

fn edge_signature(playlists: &serde_json::Value, queue: &serde_json::Value) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(playlists)
        .unwrap_or_default()
        .hash(&mut hasher);
    serde_json::to_string(queue)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}
