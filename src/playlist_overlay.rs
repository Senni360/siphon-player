//! Local SoundCloud rows spliced into Spotify playlists, without Spotify edits.
//!
//! Spotify row indices always refer to the original Spotify list. Local edits
//! checkpoint that list's identities, allowing a missing anchor to fall back to
//! its nearest earlier surviving neighbour. Reading/merging never changes the
//! stored anchors, so a temporarily incomplete response cannot erase them.
//! Encode on the UI thread, then hand bytes to `fastframe_file::Writer` with
//! `Access::Private`. Loading is startup/worker I/O, including corrupt backups.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::api::models::Track;
use crate::mixed_playlists::{Entry, load_state};

const VERSION: u32 = 1;

/// Occurrences are zero based and counted in Spotify order, including repeats.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TrackAnchor {
    pub uri: String,
    pub occurrence: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Anchor {
    pub after: Option<TrackAnchor>,
    /// Original Spotify index of `after`. `None` with no `after` means top.
    pub position: Option<usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OverlayEntry {
    pub entry: Entry,
    pub anchor: Anchor,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Overlay {
    /// Ordered by the rows at the last local edit. This breaks ties between
    /// entries sharing an anchor, including entries whose anchors disappear.
    pub entries: Vec<OverlayEntry>,
    /// Spotify order at that edit. Shared by all entries, rather than storing
    /// a copy of all earlier anchors on every insertion.
    pub spotify_snapshot: Vec<TrackAnchor>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MergedRow {
    /// `spotify_index` addresses the unmodified source list, including repeats.
    Spotify { track: Track, spotify_index: usize },
    /// `entry.id` addresses this local row independently of its track URI.
    Overlay { entry: Entry },
}

impl MergedRow {
    pub fn track(&self) -> &Track {
        match self {
            Self::Spotify { track, .. } => track,
            Self::Overlay { entry } => &entry.track,
        }
    }

    pub fn overlay_entry_id(&self) -> Option<&str> {
        match self {
            Self::Overlay { entry } => Some(&entry.id),
            Self::Spotify { .. } => None,
        }
    }

    pub fn to_track(&self) -> Track {
        match self {
            Self::Spotify { track, .. } => track.clone(),
            Self::Overlay { entry } => entry.to_track(),
        }
    }
}

/// Stable, expected O(Spotify rows + snapshot rows + overlay rows) time and
/// space using URI indexes and insertion buckets. No scans per insertion.
/// A URI+occurrence match wins even after a reorder. A removed anchor uses
/// the nearest earlier survivor in the saved Spotify order, then its saved
/// index clamped to the current list. A top insertion always stays at top.
pub fn merge(spotify_tracks: &[Track], overlay: &Overlay) -> Vec<MergedRow> {
    let mut positions: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, track) in spotify_tracks.iter().enumerate() {
        positions.entry(&track.uri).or_default().push(index);
    }
    let resolve = |anchor: &TrackAnchor| {
        positions
            .get(anchor.uri.as_str())
            .and_then(|indices| indices.get(anchor.occurrence))
            .copied()
    };
    // earlier[i] excludes the anchor at i itself.
    let mut earlier = Vec::with_capacity(overlay.spotify_snapshot.len() + 1);
    let mut survivor = None;
    earlier.push(survivor);
    for anchor in &overlay.spotify_snapshot {
        if let Some(index) = resolve(anchor) {
            survivor = Some(index);
        }
        earlier.push(survivor);
    }
    let mut buckets: Vec<Vec<&Entry>> = vec![Vec::new(); spotify_tracks.len() + 1];
    for row in &overlay.entries {
        let anchor = &row.anchor;
        let matched = anchor.after.as_ref().and_then(resolve);
        let slot = if let Some(index) = matched {
            index + 1
        } else if let Some(position) = anchor.position {
            earlier[position.min(overlay.spotify_snapshot.len())]
                .map(|index| index + 1)
                .unwrap_or_else(|| position.saturating_add(1).min(spotify_tracks.len()))
        } else {
            0
        };
        buckets[slot].push(&row.entry);
    }
    let mut rows = Vec::with_capacity(spotify_tracks.len() + overlay.entries.len());
    for (slot, bucket) in buckets.into_iter().enumerate() {
        if slot > 0 {
            rows.push(MergedRow::Spotify {
                track: spotify_tracks[slot - 1].clone(),
                spotify_index: slot - 1,
            });
        }
        rows.extend(bucket.into_iter().map(|entry| MergedRow::Overlay {
            entry: entry.clone(),
        }));
    }
    rows
}

impl Overlay {
    pub fn has_overlay(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Inserts before the merged row at `index` (clamped). Only SoundCloud
    /// tracks are accepted. The preceding overlay row shares its Spotify
    /// anchor, with the new entry placed immediately after that row.
    /// Pass a complete Spotify list so local edits retain all earlier anchors.
    pub fn insert(
        &mut self,
        spotify_tracks: &[Track],
        index: usize,
        track: &Track,
    ) -> Option<String> {
        if !crate::soundcloud::is_track_uri(&track.uri) {
            return None;
        }
        let mut rows = merge(spotify_tracks, self);
        let entry = Entry::new(track);
        let id = entry.id.clone();
        rows.insert(index.min(rows.len()), MergedRow::Overlay { entry });
        self.checkpoint(spotify_tracks, rows);
        Some(id)
    }

    /// The destination is a gap in the original merged list, before removal
    /// of the moved row, matching drag/drop semantics in mixed playlists.
    pub fn move_entry(&mut self, spotify_tracks: &[Track], id: &str, index: usize) -> bool {
        let mut rows = merge(spotify_tracks, self);
        let Some(from) = rows
            .iter()
            .position(|row| row.overlay_entry_id() == Some(id))
        else {
            return false;
        };
        let index = index.min(rows.len());
        let row = rows.remove(from);
        rows.insert(index - usize::from(from < index), row);
        self.checkpoint(spotify_tracks, rows);
        true
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|row| row.entry.id != id);
        before != self.entries.len()
    }

    fn checkpoint(&mut self, spotify_tracks: &[Track], rows: Vec<MergedRow>) {
        let mut counts = HashMap::new();
        self.spotify_snapshot = spotify_tracks
            .iter()
            .map(|track| {
                let occurrence = counts.entry(track.uri.as_str()).or_insert(0);
                let anchor = TrackAnchor {
                    uri: track.uri.clone(),
                    occurrence: *occurrence,
                };
                *occurrence += 1;
                anchor
            })
            .collect();
        self.entries.clear();
        let mut anchor = Anchor::default();
        for row in rows {
            match row {
                MergedRow::Spotify { spotify_index, .. } => {
                    anchor = Anchor {
                        after: Some(self.spotify_snapshot[spotify_index].clone()),
                        position: Some(spotify_index),
                    };
                }
                MergedRow::Overlay { entry } => self.entries.push(OverlayEntry {
                    entry,
                    anchor: anchor.clone(),
                }),
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Store {
    pub version: u32,
    /// Keyed by the Spotify playlist ID, without the URI prefix.
    pub playlists: BTreeMap<String, Overlay>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: VERSION,
            playlists: BTreeMap::new(),
        }
    }
}

impl Store {
    /// Startup/worker I/O, with the same corrupt-file handling as mixed lists.
    /// Future versions or failed reads/backups return an error; do not save
    /// an empty replacement over that file.
    pub fn load(path: &Path) -> io::Result<Self> {
        load_state(path, VERSION)
    }

    pub fn encode(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec_pretty(self)
    }

    pub fn get(&self, spotify_playlist_id: &str) -> Option<&Overlay> {
        self.playlists.get(spotify_playlist_id)
    }

    pub fn get_mut(&mut self, spotify_playlist_id: &str) -> &mut Overlay {
        self.playlists
            .entry(spotify_playlist_id.into())
            .or_default()
    }

    /// False means playback can keep using Spotify Connect unchanged.
    pub fn has_overlay(&self, spotify_playlist_id: &str) -> bool {
        self.get(spotify_playlist_id)
            .is_some_and(Overlay::has_overlay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixed_playlists::new_id;
    use std::path::PathBuf;
    use std::time::Duration;

    fn spotify(ids: &[&str]) -> Vec<Track> {
        ids.iter()
            .map(|id| Track {
                uri: format!("spotify:track:{id}"),
                name: (*id).into(),
                ..Track::default()
            })
            .collect()
    }

    fn sc(id: u64) -> Track {
        Track {
            uri: crate::soundcloud::track_uri(id),
            name: id.to_string(),
            ..Track::default()
        }
    }

    fn order(tracks: &[Track], overlay: &Overlay) -> Vec<String> {
        merge(tracks, overlay)
            .iter()
            .map(|row| row.track().name.clone())
            .collect()
    }

    fn directory() -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".cache")
            .join(format!("playlist-overlay-test-{}", new_id()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn insert_top_middle_end_and_empty_with_original_spotify_indices() {
        let tracks = spotify(&["a", "b", "c"]);
        let mut overlay = Overlay::default();
        let top = overlay.insert(&tracks, 0, &sc(1)).unwrap();
        let middle = overlay.insert(&tracks, 2, &sc(2)).unwrap();
        let end = overlay.insert(&tracks, usize::MAX, &sc(3)).unwrap();
        assert_eq!(order(&tracks, &overlay), ["1", "a", "2", "b", "c", "3"]);
        assert_eq!(overlay.entries[0].anchor, Anchor::default());
        assert_eq!(overlay.entries[1].anchor.position, Some(0));
        assert_eq!(overlay.entries[2].anchor.position, Some(2));
        let rows = merge(&tracks, &overlay);
        assert_eq!(rows[0].overlay_entry_id(), Some(top.as_str()));
        assert_eq!(rows[2].overlay_entry_id(), Some(middle.as_str()));
        assert_eq!(rows[5].overlay_entry_id(), Some(end.as_str()));
        let indices: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                MergedRow::Spotify { spotify_index, .. } => Some(*spotify_index),
                MergedRow::Overlay { .. } => None,
            })
            .collect();
        assert_eq!(indices, [0, 1, 2]);
        assert_eq!(
            rows.iter().map(MergedRow::to_track).collect::<Vec<_>>(),
            [
                sc(1),
                tracks[0].clone(),
                sc(2),
                tracks[1].clone(),
                tracks[2].clone(),
                sc(3)
            ]
        );
        let mut empty = Overlay::default();
        empty.insert(&[], 100, &sc(1)).unwrap();
        empty.insert(&[], 1, &sc(2)).unwrap();
        assert_eq!(order(&[], &empty), ["1", "2"]);
        assert_eq!(order(&tracks, &empty), ["1", "2", "a", "b", "c"]);
    }

    #[test]
    fn same_anchor_insertions_keep_order_and_accept_insertion_between_them() {
        let tracks = spotify(&["a", "b"]);
        let mut overlay = Overlay::default();
        for (index, id) in [(1, 1), (2, 2), (2, 3)] {
            overlay.insert(&tracks, index, &sc(id)).unwrap();
        }
        assert_eq!(order(&tracks, &overlay), ["a", "1", "3", "2", "b"]);
        assert!(
            overlay
                .entries
                .iter()
                .all(|row| row.anchor == overlay.entries[0].anchor)
        );
        assert_eq!(
            order(&spotify(&["b", "a"]), &overlay),
            ["b", "a", "1", "3", "2"]
        );
    }

    #[test]
    fn added_and_reordered_spotify_tracks_follow_uri_not_old_index() {
        let original = spotify(&["a", "b", "c"]);
        let mut overlay = Overlay::default();
        overlay.insert(&original, 2, &sc(1)).unwrap();
        let saved = overlay.clone();
        assert_eq!(
            order(&spotify(&["new", "a", "b", "also-new", "c"]), &overlay),
            ["new", "a", "b", "1", "also-new", "c"]
        );
        assert_eq!(
            order(&spotify(&["c", "b", "a"]), &overlay),
            ["c", "b", "1", "a"]
        );
        assert_eq!(
            overlay, saved,
            "reading cannot re-anchor on a stale response"
        );
    }

    #[test]
    fn removed_anchor_uses_nearest_earlier_survivor_then_clamped_position() {
        let original = spotify(&["a", "b", "c", "d"]);
        let mut overlay = Overlay::default();
        overlay.insert(&original, 3, &sc(1)).unwrap();
        assert_eq!(
            order(&spotify(&["a", "b", "d"]), &overlay),
            ["a", "b", "1", "d"]
        );
        assert_eq!(order(&spotify(&["d", "a"]), &overlay), ["d", "a", "1"]);
        // b is nearer in the saved order even when b moves before a.
        assert_eq!(
            order(&spotify(&["b", "d", "a"]), &overlay),
            ["b", "1", "d", "a"]
        );
        assert_eq!(
            order(&spotify(&["new", "other", "last", "extra"]), &overlay),
            ["new", "other", "last", "1", "extra"]
        );
        assert_eq!(order(&spotify(&["new"]), &overlay), ["new", "1"]);
        assert_eq!(order(&[], &overlay), ["1"]);
    }

    #[test]
    fn duplicate_spotify_occurrences_and_local_copies_stay_distinct() {
        let tracks = spotify(&["a", "b", "a", "c"]);
        let mut overlay = Overlay::default();
        let first = overlay.insert(&tracks, 1, &sc(1)).unwrap();
        let second = overlay.insert(&tracks, 4, &sc(1)).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            overlay.entries[0].anchor.after.as_ref().unwrap().occurrence,
            0
        );
        assert_eq!(
            overlay.entries[1].anchor.after.as_ref().unwrap().occurrence,
            1
        );
        assert_eq!(order(&tracks, &overlay), ["a", "1", "b", "a", "1", "c"]);
        assert_eq!(
            order(&spotify(&["a", "c", "b", "a"]), &overlay),
            ["a", "1", "c", "b", "a", "1"]
        );
        assert_eq!(
            order(&spotify(&["a", "b", "c"]), &overlay),
            ["a", "1", "b", "1", "c"]
        );
        assert!(overlay.remove(&first));
        assert_eq!(overlay.entries[0].entry.id, second);
    }

    #[test]
    fn fallback_collisions_preserve_order_including_top_insertions() {
        let tracks = spotify(&["a", "b", "c"]);
        let mut overlay = Overlay::default();
        for (index, id) in [(0, 1), (2, 2), (4, 3), (6, 4)] {
            overlay.insert(&tracks, index, &sc(id)).unwrap();
        }
        assert_eq!(
            order(&tracks, &overlay),
            ["1", "a", "2", "b", "3", "c", "4"]
        );
        assert_eq!(order(&spotify(&["a"]), &overlay), ["1", "a", "2", "3", "4"]);
        assert_eq!(order(&[], &overlay), ["1", "2", "3", "4"]);
    }

    #[test]
    fn move_reanchors_only_local_rows_and_preserves_identity_and_tie_order() {
        let tracks = spotify(&["a", "b", "c"]);
        let mut overlay = Overlay::default();
        let one = overlay.insert(&tracks, 1, &sc(1)).unwrap();
        let two = overlay.insert(&tracks, 2, &sc(2)).unwrap();
        assert!(overlay.move_entry(&tracks, &two, 1));
        assert_eq!(order(&tracks, &overlay), ["a", "2", "1", "b", "c"]);
        assert!(overlay.move_entry(&tracks, &one, 0));
        assert_eq!(order(&tracks, &overlay), ["1", "a", "2", "b", "c"]);
        assert!(overlay.move_entry(&tracks, &one, usize::MAX));
        assert_eq!(order(&tracks, &overlay), ["a", "2", "b", "c", "1"]);
        assert_eq!(overlay.entries[1].entry.id, one);
        assert_eq!(overlay.entries[1].anchor.position, Some(2));
        let saved = overlay.clone();
        assert!(overlay.move_entry(&tracks, &two, 2));
        assert_eq!(overlay, saved, "drop immediately after itself is a no-op");
        assert!(!overlay.move_entry(&tracks, "missing", 0));
        assert!(overlay.insert(&tracks, 0, &tracks[0]).is_none());
        assert_eq!(overlay, saved);
        assert!(overlay.remove(&two));
        assert!(!overlay.remove(&two));
        assert!(overlay.remove(&one));
        assert!(!overlay.has_overlay());
    }

    #[test]
    fn editing_after_a_remote_change_checkpoints_the_new_order() {
        let mut overlay = Overlay::default();
        overlay
            .insert(&spotify(&["a", "b", "c"]), 2, &sc(1))
            .unwrap();
        let changed = spotify(&["c", "new", "a"]);
        assert_eq!(order(&changed, &overlay), ["c", "new", "a", "1"]);
        overlay.insert(&changed, 4, &sc(2)).unwrap();
        assert_eq!(order(&changed, &overlay), ["c", "new", "a", "1", "2"]);
        assert_eq!(overlay.entries[0].anchor.position, Some(2));
        assert_eq!(
            order(&spotify(&["c", "new"]), &overlay),
            ["c", "new", "1", "2"]
        );
    }

    #[test]
    fn moving_after_a_remote_reorder_keeps_source_indices_and_duplicate_ids() {
        let mut overlay = Overlay::default();
        let original = spotify(&["a", "b", "c"]);
        let first = overlay.insert(&original, 1, &sc(1)).unwrap();
        let second = overlay.insert(&original, 2, &sc(1)).unwrap();
        let changed = spotify(&["c", "a", "new", "b"]);
        assert!(overlay.move_entry(&changed, &second, 0));
        let rows = merge(&changed, &overlay);
        assert_eq!(order(&changed, &overlay), ["1", "c", "a", "1", "new", "b"]);
        assert_eq!(rows[0].overlay_entry_id(), Some(second.as_str()));
        assert_eq!(rows[3].overlay_entry_id(), Some(first.as_str()));
        for row in rows {
            if let MergedRow::Spotify {
                track,
                spotify_index,
            } = row
            {
                assert_eq!(track, changed[spotify_index]);
            }
        }
    }

    #[test]
    fn round_trip_and_atomic_replacement_keep_anchors_and_playback_flags() {
        let root = directory();
        let path = root.join("overlay.json");
        let mut store = Store::load(&path).unwrap();
        assert!(!store.has_overlay("p"));
        assert!(!store.get_mut("empty").has_overlay());
        let tracks = spotify(&["a", "a", "b"]);
        let id = store.get_mut("p").insert(&tracks, 2, &sc(1)).unwrap();
        assert!(store.has_overlay("p"));
        assert!(!store.has_overlay("empty"));
        let files = fastframe_file::Writer::new();
        files.write(
            &path,
            store.encode().unwrap(),
            fastframe_file::Access::Private,
        );
        assert!(files.flush(Duration::from_secs(10)));
        let loaded = Store::load(&path).unwrap();
        assert_eq!(loaded, store);
        assert_eq!(
            order(&tracks, loaded.get("p").unwrap()),
            ["a", "a", "1", "b"]
        );
        store.get_mut("p").remove(&id);
        files.write(
            &path,
            store.encode().unwrap(),
            fastframe_file::Access::Private,
        );
        assert!(files.flush(Duration::from_secs(10)));
        assert!(!Store::load(&path).unwrap().has_overlay("p"));
        assert!(crate::util::temporary_files(&root).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn v1_missing_fields_unknown_fields_and_position_fallback() {
        let root = directory();
        let path = root.join("overlay.json");
        std::fs::write(
            &path,
            r#"{"version":1,"future":[],"playlists":{
            "p":{"entries":[
                {"entry":{"id":"old","track":{"uri":"soundcloud:track:1","name":"1"}},
                 "anchor":{"after":{"uri":"spotify:track:b"},"position":1}},
                {"entry":{"track":{"uri":"soundcloud:track:2","name":"2"}}}
            ]}}}"#,
        )
        .unwrap();
        let store = Store::load(&path).unwrap();
        let overlay = store.get("p").unwrap();
        assert!(overlay.spotify_snapshot.is_empty());
        assert_eq!(
            order(&spotify(&["a", "b", "c"]), overlay),
            ["2", "a", "b", "1", "c"]
        );
        assert_eq!(
            order(&spotify(&["x", "y", "z"]), overlay),
            ["2", "x", "y", "1", "z"]
        );
        assert!(!overlay.entries[1].entry.id.is_empty());
        let restored: Store = serde_json::from_slice(&store.encode().unwrap()).unwrap();
        assert_eq!(restored, store);
        assert_eq!(
            serde_json::from_str::<Store>("{}").unwrap(),
            Store::default()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_file_is_preserved_and_unknown_version_is_an_error() {
        let root = directory();
        let path = root.join("overlay.json");
        std::fs::write(&path, "{broken").unwrap();
        assert_eq!(Store::load(&path).unwrap(), Store::default());
        assert!(!path.exists());
        let backups: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read_to_string(backups[0].path()).unwrap(),
            "{broken"
        );
        let future = r#"{"version":2,"playlists":{}}"#;
        std::fs::write(&path, future).unwrap();
        assert_eq!(
            Store::load(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), future);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unversioned_files_default_missing_anchors_to_top_and_keep_ids_on_save() {
        let root = directory();
        let path = root.join("overlay.json");
        std::fs::write(
            &path,
            r#"{"playlists":{"p":{"entries":[
                {"entry":{"track":{"uri":"soundcloud:track:1","name":"1"}}},
                {"entry":{"track":{"uri":"soundcloud:track:1","name":"1"}}}
            ]}}}"#,
        )
        .unwrap();
        let store = Store::load(&path).unwrap();
        let overlay = store.get("p").unwrap();
        assert_eq!(store.version, VERSION);
        assert_ne!(overlay.entries[0].entry.id, overlay.entries[1].entry.id);
        assert_eq!(order(&spotify(&["a"]), overlay), ["1", "1", "a"]);
        let restored: Store = serde_json::from_slice(&store.encode().unwrap()).unwrap();
        assert_eq!(restored, store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn large_merge_keeps_every_row_and_stable_original_indices() {
        let tracks: Vec<_> = (0..10_000)
            .map(|i| Track {
                uri: format!("spotify:track:{i}"),
                ..Track::default()
            })
            .collect();
        let overlay = Overlay {
            spotify_snapshot: tracks
                .iter()
                .map(|track| TrackAnchor {
                    uri: track.uri.clone(),
                    occurrence: 0,
                })
                .collect(),
            entries: (0..10_000)
                .map(|i| OverlayEntry {
                    entry: Entry::new(&sc(i as u64)),
                    anchor: Anchor {
                        after: Some(TrackAnchor {
                            uri: format!("spotify:track:{i}"),
                            occurrence: 0,
                        }),
                        position: Some(i),
                    },
                })
                .collect(),
        };
        let rows = merge(&tracks, &overlay);
        assert_eq!(rows.len(), 20_000);
        for (i, pair) in rows.as_chunks::<2>().0.iter().enumerate() {
            assert!(
                matches!(&pair[0], MergedRow::Spotify { spotify_index, .. } if *spotify_index == i)
            );
            assert_eq!(
                pair[1].overlay_entry_id(),
                Some(overlay.entries[i].entry.id.as_str())
            );
        }
    }
}
