//! Playlists owned by this computer, with ordinary tracks from either service.
//!
//! [`Store::encode`] does no I/O. Hand its bytes to the app's
//! `fastframe_file::Writer` with `Access::Private` for atomic background writes.
//! [`Store::load`] is startup I/O and must run before drawing or on a worker.

use std::collections::HashSet;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::api::models::Track;

pub const PLAYLIST_PREFIX: &str = "spotifast:playlist:";
const VERSION: u32 = 1;

/// A row identity is independent of its track URI, so duplicates are editable.
/// The complete metadata snapshot retains artwork, artists, availability and
/// service IDs without needing a network read. It never contains audio.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Entry {
    pub id: String,
    pub track: Track,
    /// Public track link. Kept explicitly so it survives changes to API models.
    pub permalink: Option<String>,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            id: new_id(),
            track: Track::default(),
            permalink: None,
        }
    }
}

impl Entry {
    pub fn new(track: &Track) -> Self {
        Self {
            track: track.clone(),
            permalink: track.external_urls.spotify.clone(),
            ..Self::default()
        }
    }

    /// Restores the snapshot, including a separately retained public link.
    pub fn to_track(&self) -> Track {
        let mut track = self.track.clone();
        if self.permalink.is_some() {
            track.external_urls.spotify.clone_from(&self.permalink);
        }
        track
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Unix seconds. Zero means absent in an older file.
    pub created: i64,
    pub modified: i64,
    pub entries: Vec<Entry>,
}

impl Default for Playlist {
    fn default() -> Self {
        Self {
            id: new_id(),
            name: String::new(),
            description: String::new(),
            created: 0,
            modified: 0,
            entries: Vec::new(),
        }
    }
}

impl Playlist {
    pub fn uri(&self) -> String {
        format!("{PLAYLIST_PREFIX}{}", self.id)
    }

    pub fn rename(&mut self, name: String) {
        self.name = name;
        self.modified = now();
    }

    pub fn set_description(&mut self, description: String) {
        self.description = description;
        self.modified = now();
    }

    /// Inserts before `index`, clamped to the end. `None` appends.
    /// Returns the new entry IDs in input order.
    pub fn add_tracks(&mut self, tracks: &[Track], index: Option<usize>) -> Vec<String> {
        let entries: Vec<_> = tracks.iter().map(Entry::new).collect();
        let ids = entries.iter().map(|entry| entry.id.clone()).collect();
        let index = index.unwrap_or(self.entries.len()).min(self.entries.len());
        if !entries.is_empty() {
            self.entries.splice(index..index, entries);
            self.modified = now();
        }
        ids
    }

    pub fn remove_entries(&mut self, ids: &[String]) -> usize {
        let ids: HashSet<_> = ids.iter().collect();
        let before = self.entries.len();
        self.entries.retain(|entry| !ids.contains(&entry.id));
        let removed = before - self.entries.len();
        if removed != 0 {
            self.modified = now();
        }
        removed
    }

    /// Moves the selected entries, in playlist order, to a gap in the
    /// *original* list. Unknown or repeated IDs are harmless. A drop within
    /// a contiguous selection leaves it where it was.
    pub fn move_entries(&mut self, ids: &[String], index: usize) {
        let ids: HashSet<_> = ids.iter().collect();
        let index = index.min(self.entries.len());
        let before = self.entries[..index]
            .iter()
            .filter(|entry| ids.contains(&entry.id))
            .count();
        let (selected, remaining): (Vec<_>, Vec<_>) = std::mem::take(&mut self.entries)
            .into_iter()
            .partition(|entry| ids.contains(&entry.id));
        self.entries = remaining;
        if !selected.is_empty() {
            self.entries
                .splice(index - before..index - before, selected);
            self.modified = now();
        }
    }

    pub fn to_tracks(&self) -> Vec<Track> {
        self.entries.iter().map(Entry::to_track).collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Store {
    pub version: u32,
    pub playlists: Vec<Playlist>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: VERSION,
            playlists: Vec::new(),
        }
    }
}

impl Store {
    /// Missing files start empty. Malformed JSON is moved aside before
    /// returning an empty store. Read/backup errors and future versions are
    /// returned to the caller, which must not overwrite the original file.
    pub fn load(path: &Path) -> io::Result<Self> {
        load_state(path, VERSION)
    }

    /// Readable JSON bytes, safe to encode on the UI thread.
    pub fn encode(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec_pretty(self)
    }

    pub fn get(&self, id: &str) -> Option<&Playlist> {
        self.playlists.iter().find(|playlist| playlist.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Playlist> {
        self.playlists.iter_mut().find(|playlist| playlist.id == id)
    }

    /// Returns the new playlist's local ID (not its URI).
    pub fn create(&mut self, name: String, description: String) -> String {
        let at = now();
        let playlist = Playlist {
            name,
            description,
            created: at,
            modified: at,
            ..Playlist::default()
        };
        let id = playlist.id.clone();
        self.playlists.push(playlist);
        id
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.playlists.len();
        self.playlists.retain(|playlist| playlist.id != id);
        self.playlists.len() != before
    }

    /// A duplicate has fresh playlist and entry identities.
    pub fn duplicate(&mut self, id: &str, name: String) -> Option<String> {
        let mut playlist = self.get(id)?.clone();
        playlist.id = new_id();
        playlist.name = name;
        playlist.created = now();
        playlist.modified = playlist.created;
        for entry in &mut playlist.entries {
            entry.id = new_id();
        }
        let id = playlist.id.clone();
        self.playlists.push(playlist);
        Some(id)
    }

    /// Imports snapshots only, without changing or fetching the source playlist.
    pub fn import(&mut self, name: String, description: String, tracks: Vec<Track>) -> String {
        let id = self.create(name, description);
        self.get_mut(&id).unwrap().add_tracks(&tracks, None);
        id
    }
}

pub(crate) fn new_id() -> String {
    format!("{:016x}", rand::random::<u64>())
}

fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

fn check_version(version: u32, supported: u32) -> io::Result<()> {
    if version != supported {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported playlist state version {version}"),
        ));
    }
    Ok(())
}

/// Shared startup-only reader. A unique backup name retains previous corrupt
/// files too. If the rename fails, propagate it instead of risking data loss.
pub(crate) fn load_state<T: DeserializeOwned + Default>(
    path: &Path,
    version: u32,
) -> io::Result<T> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            quarantine(path)?;
            return Ok(T::default());
        }
        Err(error) => return Err(error),
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    // Inspect the version before decoding this version's payload. A future
    // schema must not be treated as corrupt simply because its types changed.
    #[derive(Deserialize)]
    struct Header {
        version: Option<u32>,
    }
    if let Ok(header) = serde_json::from_str::<Header>(text) {
        check_version(header.version.unwrap_or(version), version)?;
    }
    match serde_json::from_str(text) {
        Ok(value) => Ok(value),
        Err(error) => {
            log::warn!(
                "playlist state at {} is unreadable: {error}",
                path.display()
            );
            quarantine(path)?;
            Ok(T::default())
        }
    }
}

fn quarantine(path: &Path) -> io::Result<()> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".corrupt-{}", new_id()));
    std::fs::rename(path, path.with_file_name(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{Album, ArtistRef, ExternalUrls, Image};
    use std::path::PathBuf;
    use std::time::Duration;

    pub(super) fn track(uri: &str) -> Track {
        Track {
            id: Some(uri.rsplit(':').next().unwrap().into()),
            uri: uri.into(),
            name: format!("Song {uri}"),
            duration_ms: 123_456,
            artists: vec![ArtistRef {
                name: "Artist".into(),
                ..ArtistRef::default()
            }],
            album: Some(Album {
                name: "Album".into(),
                images: vec![Image {
                    url: "https://example.test/art.jpg".into(),
                    ..Image::default()
                }],
                ..Album::default()
            }),
            external_urls: ExternalUrls {
                spotify: Some("https://example.test/song".into()),
            },
            is_playable: Some(false),
            ..Track::default()
        }
    }

    fn directory() -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".cache")
            .join(format!("mixed-playlists-test-{}", new_id()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn import_rename_duplicate_delete_and_metadata_round_trip() {
        let tracks = vec![track("spotify:track:a"), track("soundcloud:track:12")];
        let mut store = Store::default();
        let id = store.import("Imported".into(), "Both services".into(), tracks.clone());
        let playlist = store.get_mut(&id).unwrap();
        assert_eq!(playlist.uri(), format!("spotifast:playlist:{id}"));
        assert_eq!(id.len(), 16);
        assert!(playlist.created > 0);
        playlist.rename("Renamed".into());
        playlist.set_description("Description".into());
        assert!(playlist.modified >= playlist.created);
        assert_eq!(playlist.to_tracks(), tracks);
        assert_eq!(
            playlist.entries[1].permalink,
            tracks[1].external_urls.spotify
        );
        let duplicate = store.duplicate(&id, "Copy".into()).unwrap();
        assert_ne!(duplicate, id);
        let copied = store.get(&duplicate).unwrap();
        assert_eq!(copied.to_tracks(), tracks);
        assert_eq!(copied.description, "Description");
        for (original, copy) in store.get(&id).unwrap().entries.iter().zip(&copied.entries) {
            assert_ne!(original.id, copy.id);
        }
        let restored: Store = serde_json::from_slice(&store.encode().unwrap()).unwrap();
        assert_eq!(restored, store);
        assert_eq!(restored.get(&id).unwrap().name, "Renamed");
        assert!(store.delete(&id));
        assert!(!store.delete(&id));
        assert!(store.get(&duplicate).is_some());
        assert!(store.duplicate("missing", "No copy".into()).is_none());
    }

    #[test]
    fn duplicates_have_distinct_ids_and_remove_only_selected_copies() {
        let mut playlist = Playlist::default();
        let song = track("soundcloud:track:12");
        let ids = playlist.add_tracks(&[song.clone(), song.clone()], None);
        assert_ne!(ids[0], ids[1]);
        assert_eq!(playlist.to_tracks(), [song.clone(), song.clone()]);
        let inserted = playlist.add_tracks(&[track("spotify:track:b")], Some(1));
        assert_eq!(playlist.entries[1].id, inserted[0]);
        assert_eq!(
            playlist.remove_entries(&[ids[0].clone(), "missing".into()]),
            1
        );
        assert_eq!(playlist.entries[1].id, ids[1]);
        assert_eq!(playlist.entries[1].to_track(), song);
        assert_eq!(playlist.remove_entries(std::slice::from_ref(&ids[0])), 0);
        assert!(playlist.add_tracks(&[], Some(usize::MAX)).is_empty());
        playlist.add_tracks(&[track("spotify:track:last")], Some(usize::MAX));
        assert_eq!(
            playlist.entries.last().unwrap().track.uri,
            "spotify:track:last"
        );
    }

    #[test]
    fn multi_selection_reorder_preserves_playlist_order_and_entry_ids() {
        let tracks: Vec<_> = (0..6)
            .map(|i| track(&format!("spotify:track:{i}")))
            .collect();
        let mut playlist = Playlist::default();
        let ids = playlist.add_tracks(&tracks, None);
        // Selection input order and duplicate selection IDs cannot reorder rows.
        playlist.move_entries(&[ids[3].clone(), ids[1].clone(), ids[3].clone()], 6);
        let order = |playlist: &Playlist| -> Vec<_> {
            playlist.entries.iter().map(|row| row.id.clone()).collect()
        };
        let expected =
            |indices: &[usize]| -> Vec<_> { indices.iter().map(|&i| ids[i].clone()).collect() };
        assert_eq!(order(&playlist), expected(&[0, 2, 4, 5, 1, 3]));
        playlist.move_entries(&[ids[3].clone(), ids[1].clone()], 0);
        assert_eq!(order(&playlist), expected(&[1, 3, 0, 2, 4, 5]));
        playlist.move_entries(&[ids[1].clone(), ids[3].clone()], 1);
        assert_eq!(order(&playlist), expected(&[1, 3, 0, 2, 4, 5]));
        playlist.move_entries(&["unknown".into()], usize::MAX);
        assert_eq!(order(&playlist), expected(&[1, 3, 0, 2, 4, 5]));
        playlist.move_entries(&ids, 3);
        assert_eq!(order(&playlist), expected(&[1, 3, 0, 2, 4, 5]));
    }

    #[test]
    fn v1_missing_fields_unknown_fields_and_bom_remain_readable() {
        let root = directory();
        let path = root.join("mixed.json");
        std::fs::write(
            &path,
            concat!(
                "\u{feff}",
                r#"{"version":1,"future":true,"playlists":[{"id":"local","name":"Old","future":42,
            "entries":[{"track":{"uri":"soundcloud:track:1","name":"Song"}},
                        {"track":{"uri":"soundcloud:track:1"}}]}]}"#
            ),
        )
        .unwrap();
        let store = Store::load(&path).unwrap();
        let playlist = store.get("local").unwrap();
        assert_eq!(playlist.description, "");
        assert_eq!(playlist.created, 0);
        assert_eq!(playlist.modified, 0);
        assert_eq!(playlist.entries[0].track.name, "Song");
        assert_ne!(playlist.entries[0].id, playlist.entries[1].id);
        assert_eq!(playlist.entries[0].permalink, None);
        let restored: Store = serde_json::from_slice(&store.encode().unwrap()).unwrap();
        assert_eq!(restored, store, "generated IDs become stable on save");
        assert_eq!(
            serde_json::from_str::<Store>("{}").unwrap(),
            Store::default()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unversioned_files_gain_defaults_without_losing_snapshot_links() {
        let root = directory();
        let path = root.join("mixed.json");
        std::fs::write(
            &path,
            r#"{"playlists":[{"name":"Older playlist","entries":[
                {"permalink":"https://soundcloud.com/artist/song",
                 "track":{"uri":"soundcloud:track:1"}},
                {}
            ]}]}"#,
        )
        .unwrap();
        let store = Store::load(&path).unwrap();
        assert_eq!(store.version, VERSION);
        let playlist = &store.playlists[0];
        assert_eq!(playlist.id.len(), 16);
        assert!(playlist.id.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(playlist.entries[0].id, playlist.entries[1].id);
        let tracks = playlist.to_tracks();
        assert_eq!(
            tracks[0].external_urls.spotify.as_deref(),
            Some("https://soundcloud.com/artist/song")
        );
        assert_eq!(tracks[1], Track::default());
        let restored: Store = serde_json::from_slice(&store.encode().unwrap()).unwrap();
        assert_eq!(restored, store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn background_atomic_replacement_and_missing_file_load() {
        let root = directory();
        let path = root.join("mixed.json");
        assert_eq!(Store::load(&path).unwrap(), Store::default());
        let files = fastframe_file::Writer::new();
        let mut store = Store::default();
        let id = store.create("First".into(), String::new());
        files.write(
            &path,
            store.encode().unwrap(),
            fastframe_file::Access::Private,
        );
        store.get_mut(&id).unwrap().rename("Second".into());
        files.write(
            &path,
            store.encode().unwrap(),
            fastframe_file::Access::Private,
        );
        assert!(files.flush(Duration::from_secs(10)));
        assert_eq!(Store::load(&path).unwrap(), store);
        assert!(crate::util::temporary_files(&root).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_files_are_preserved_and_future_versions_are_not_replaced() {
        let root = directory();
        let path = root.join("mixed.json");
        let broken: &[&[u8]] = &[b"{\"playlists\": [", b"not json", b"\xff\xfe"];
        for (i, bytes) in broken.iter().enumerate() {
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(Store::load(&path).unwrap(), Store::default());
            assert!(!path.exists());
            let backups: Vec<_> = std::fs::read_dir(&root)
                .unwrap()
                .map(Result::unwrap)
                .collect();
            assert_eq!(backups.len(), i + 1);
            assert!(
                backups
                    .iter()
                    .any(|backup| std::fs::read(backup.path()).unwrap() == *bytes)
            );
        }
        let future = br#"{"version":999,"playlists":"future schema"}"#;
        std::fs::write(&path, future).unwrap();
        assert_eq!(
            Store::load(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(std::fs::read(&path).unwrap(), future);
        assert!(
            Store::load(&root).is_err(),
            "I/O errors are not empty stores"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
