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
/// How often the playlist rows are searched for local files to offer.
const UPLOAD_LOOK: Duration = Duration::from_secs(10);
/// The queue's position is stored to this many seconds. An exact position
/// reaches the phone over the socket; a rounded one keeps a playing song from
/// writing the document every few seconds.
const POSITION_BUCKET_MS: u32 = 10_000;

#[derive(Default)]
pub struct Edge {
    /// The last vault setting handed to the backend.
    pub vault: Option<bool>,
    /// The last uploads setting handed to the backend.
    pub uploads: Option<bool>,
    /// The last connection setting handed to the backend.
    pub enabled: Option<bool>,
    /// A change waiting for quiet.
    dirty: Option<Instant>,
    /// The snapshot signatures last looked at.
    looked_signature: Option<(u64, u64)>,
    /// The snapshot signatures last handed to the backend. Each document is
    /// offered on its own, so a moving queue never rewrites the playlists.
    sent: Option<(u64, u64)>,
    /// When the snapshot was last looked at.
    looked: Option<Instant>,
    /// When local files were last searched for.
    scanned: Option<Instant>,
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
            self.edge_off();
            return;
        }
        let enabled = self.settings.edge_enabled;
        if self.edge.enabled != Some(enabled) {
            self.edge.enabled = Some(enabled);
            self.backend
                .send(Command::Edge(edge::Request::Enabled(enabled)));
            if enabled {
                // Switched back on: offer everything again, since the edge may
                // have missed what changed while this computer was away.
                self.edge.sent = None;
                self.edge.looked_signature = None;
                self.edge.dirty = Some(Instant::now() - SETTLE);
            } else {
                self.edge_off();
                return;
            }
        }
        if !enabled {
            return;
        }
        if let Some(vault) = self.vault_signins_enabled()
            && self.edge.vault != Some(vault)
        {
            self.edge.vault = Some(vault);
            self.backend
                .send(Command::Edge(edge::Request::Vault(vault)));
        }
        let uploads = self.settings.local_files_on_phone;
        if self.edge.uploads != Some(uploads) {
            self.edge.uploads = Some(uploads);
            self.backend
                .send(Command::Edge(edge::Request::UploadsEnabled(uploads)));
            if uploads {
                // Offer the files again: one skipped while the setting was off
                // must not wait for a restart.
                self.edge.scanned = None;
            } else {
                self.edge.queued.clear();
            }
        }
        if uploads
            && self
                .edge
                .scanned
                .is_none_or(|scanned| scanned.elapsed() >= UPLOAD_LOOK)
        {
            self.edge.scanned = Some(Instant::now());
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
        let signature = (edge_signature(&playlists), edge_signature(&queue));
        if self.edge.looked_signature != Some(signature) {
            self.edge.looked_signature = Some(signature);
            self.edge.dirty = Some(Instant::now());
        }
        if self.edge.resync {
            self.edge.resync = false;
            self.edge.sent = None;
            self.edge.dirty = Some(Instant::now() - SETTLE);
        }
        if self
            .edge
            .dirty
            .is_some_and(|dirty| dirty.elapsed() >= SETTLE)
        {
            self.edge.dirty = None;
            let sent = self.edge.sent;
            if sent.is_none_or(|(playlists, _)| playlists != signature.0) {
                self.backend
                    .send(Command::Edge(edge::Request::SyncPlaylists(playlists)));
            }
            if sent.is_none_or(|(_, queue)| queue != signature.1) {
                self.backend
                    .send(Command::Edge(edge::Request::SyncQueue(queue)));
            }
            self.edge.sent = Some(signature);
        }
    }

    /// Forgets everything the backend held for a connection that is now off,
    /// so turning it on again offers the vault, the uploads and both
    /// documents from scratch.
    fn edge_off(&mut self) {
        let was_on = self.edge.enabled != Some(false);
        self.edge.enabled = Some(false);
        self.edge.vault = None;
        self.edge.uploads = None;
        self.edge.queued.clear();
        self.edge.scanned = None;
        self.edge.looked = None;
        self.edge.looked_signature = None;
        self.edge.sent = None;
        self.edge.dirty = Some(Instant::now() - SETTLE);
        if was_on {
            self.backend
                .send(Command::Edge(edge::Request::Enabled(false)));
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
                            album: entry
                                .track
                                .album
                                .as_ref()
                                .map(|album| album.name.clone())
                                .filter(|name| !name.trim().is_empty()),
                            upload: (self.settings.local_files_on_phone
                                && uri.starts_with("spotify:local:"))
                            .then(|| edge::local_key(&uri)),
                            link: crate::soundcloud::is_track_uri(&uri)
                                .then(|| {
                                    edge::public_soundcloud_link(
                                        entry.track.external_urls.spotify.as_deref(),
                                    )
                                })
                                .flatten(),
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
                stored_position(snapshot.position_ms),
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
        let jobs: Vec<UploadJob> = uris
            .into_iter()
            .filter(|uri| uri.starts_with("spotify:local:"))
            .filter(|uri| seen.insert(uri.clone()))
            .filter_map(|uri| {
                // The key comes from the URI alone, so a file already offered
                // is passed over before its path is looked for.
                let key = edge::local_key(&uri);
                if self.edge.queued.contains(&key) {
                    return None;
                }
                // A stat here would wait for a network folder on the UI
                // thread; the uploader reads the file itself.
                Some(UploadJob {
                    key,
                    path: self.local_file(&uri)?.to_owned(),
                    fetched: false,
                })
            })
            .collect();
        for job in &jobs {
            self.edge.queued.insert(job.key.clone());
        }
        if !jobs.is_empty() {
            self.backend
                .send(Command::Edge(edge::Request::Upload(jobs)));
        }
    }

    /// Whether the listener wants the sign-ins kept in the edge vault. An
    /// unset choice is on for the group's owner and off for everyone else.
    /// `None` until the account is known: deciding earlier would delete a
    /// vault the owner set up before this computer read its groups.
    pub(crate) fn vault_signins_enabled(&self) -> Option<bool> {
        if let Some(choice) = self.settings.vault_signins {
            return Some(choice);
        }
        let account = self.friends_feed.account.as_ref()?;
        Some(
            account
                .chosen_group(self.settings.group_current.as_deref())
                .and_then(|group| group.owner_id.as_deref())
                .is_some_and(|owner| owner == account.user_id),
        )
    }

    pub(super) fn edge_event(&mut self, event: edge::Event) {
        match event {
            edge::Event::Commands(commands) => self.accept_commands(commands),
            edge::Event::Phone(phone) => {
                // The edge stamps a frame with its own clock, so a replay after
                // a reconnect is judged as old instead of fresh.
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_millis().min(i64::MAX as u128) as i64)
                    .unwrap_or_default();
                self.heard_from_phone(phone.map(|phone| {
                    let age = if phone.at > 0 {
                        now.saturating_sub(phone.at)
                    } else {
                        0
                    };
                    (phone, age)
                }));
            }
            edge::Event::Reset => {
                // The edge restarted or this computer was away long enough for
                // its replay buffer to run out: every document is offered again.
                self.edge.resync = true;
                self.backend.send(Command::Edge(edge::Request::Matches));
            }
        }
    }
}

fn edge_signature(document: &serde_json::Value) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(document)
        .unwrap_or_default()
        .hash(&mut hasher);
    hasher.finish()
}

/// The position the queue document keeps: rounded down to ten seconds, so a
/// playing song does not make the document a new one every few seconds.
fn stored_position(position_ms: u32) -> u32 {
    position_ms - position_ms % POSITION_BUCKET_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A playing song's position moves every frame; the document is only a new
    /// one every ten seconds, which is what keeps writes rare.
    #[test]
    fn only_a_whole_ten_seconds_of_position_changes_the_document() {
        assert_eq!(stored_position(0), 0);
        assert_eq!(stored_position(9_999), 0);
        assert_eq!(stored_position(10_000), 10_000);
        assert_eq!(stored_position(19_999), 10_000);
        assert_eq!(stored_position(20_001), 20_000);
        // A few seconds of playback never moves it within one bucket.
        assert_eq!(stored_position(63_400), stored_position(69_999));
        assert_ne!(stored_position(69_999), stored_position(70_000));
    }

    /// The store is filled again after a reset, without waiting for a change.
    #[test]
    fn a_reset_from_the_edge_asks_for_every_document_again() {
        let mut app = crate::app::tests::test_app("edge-reset");
        assert!(!app.edge.resync);
        app.edge_event(edge::Event::Reset);
        assert!(app.edge.resync);
        app.backend.shutdown();
    }

    /// Turning the connection off forgets what the stopped backend held, so
    /// turning it back on offers the vault and the uploads again.
    #[test]
    fn switching_the_edge_off_and_on_offers_everything_again() {
        let mut app = crate::app::tests::test_app("edge-off-on");
        app.settings.phone_linked = true;
        app.phone.started = true;
        app.settings.vault_signins = Some(true);
        app.settings.edge_enabled = true;
        app.edge_tick();
        assert_eq!(app.edge.enabled, Some(true));
        assert_eq!(app.edge.vault, Some(true));
        assert_eq!(app.edge.uploads, Some(true));
        app.settings.edge_enabled = false;
        app.edge_tick();
        assert_eq!(app.edge.enabled, Some(false));
        assert_eq!(app.edge.vault, None);
        assert_eq!(app.edge.uploads, None);
        app.settings.edge_enabled = true;
        app.edge_tick();
        assert_eq!(app.edge.enabled, Some(true));
        assert_eq!(app.edge.vault, Some(true));
        assert_eq!(app.edge.uploads, Some(true));
        app.backend.shutdown();
    }

    /// The vault default follows the account's owner, which is unknown until
    /// the group loads. Guessing before that would delete the owner's vault.
    #[test]
    fn the_vault_choice_waits_for_the_group_account() {
        let mut app = crate::app::tests::test_app("edge-vault-account");
        app.settings.phone_linked = true;
        app.phone.started = true;
        app.settings.edge_enabled = true;
        app.edge_tick();
        assert_eq!(app.edge.vault, None);
        app.settings.vault_signins = Some(false);
        app.edge_tick();
        assert_eq!(app.edge.vault, Some(false));
        app.backend.shutdown();
    }
}
