//! The UI's local-file index. Call on a blocking worker, never the UI thread.
//!
//! The pinned player's separate startup index still needs the corresponding
//! librespot fix. Keep URI matching compatible with it until that lands.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use symphonia::core::{
    io::MediaSourceStream,
    meta::{MetadataOptions, StandardTagKey},
    probe::Hint,
};

/// The folder index on disk. A walk reuses a cached URI for a file whose size
/// and modification time are unchanged, so only new or edited files are
/// probed again.
pub(crate) const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Cache {
    /// Older files without a version read as the first version; a newer,
    /// unknown version is ignored and rebuilt.
    #[serde(default)]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) folder: Option<PathBuf>,
    #[serde(default)]
    files: Vec<CachedFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedFile {
    uri: String,
    path: PathBuf,
    size: u64,
    mtime_ms: u64,
}

impl Cache {
    /// The cache at `path`, or none when it is missing, unreadable or from a
    /// newer version. A cache is disposable: an unusable one only means the
    /// next walk probes every file.
    pub(crate) fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let cache: Self = serde_json::from_str(text).ok()?;
        (cache.version <= VERSION).then_some(cache)
    }

    pub(crate) fn encode(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec(self)
    }

    /// The `spotify:local:` URIs and the files that play them.
    pub(crate) fn uris(&self) -> HashMap<String, PathBuf> {
        self.files
            .iter()
            .map(|file| (file.uri.clone(), file.path.clone()))
            .collect()
    }

    fn by_path(&self) -> HashMap<&Path, &CachedFile> {
        self.files
            .iter()
            .map(|file| (file.path.as_path(), file))
            .collect()
    }
}

/// What a walk found, and the cache to keep for the next start.
pub(crate) struct Scan {
    pub(crate) uris: HashMap<String, PathBuf>,
    pub(crate) cache: Cache,
}

#[cfg(test)]
pub(crate) fn scan(folder: &Path) -> HashMap<String, PathBuf> {
    scan_with(folder, None).uris
}

/// Walks `folder`, reusing `cached` entries whose file is still the same size
/// and modification time. Only changed or new files are opened and probed.
pub(crate) fn scan_with(folder: &Path, cached: Option<&Cache>) -> Scan {
    let previous = cached
        .filter(|cache| cache.folder.as_deref() == Some(folder))
        .map(Cache::by_path);
    let mut found: HashMap<String, CachedFile> = HashMap::new();
    visit(folder, &mut |path| {
        let (size, mtime_ms) = file_stamp(path);
        let uri = previous
            .as_ref()
            .and_then(|files| files.get(path))
            .filter(|file| file.size == size && file.mtime_ms == mtime_ms)
            .map(|file| file.uri.clone())
            .or_else(|| match local_uri(path) {
                Ok(uri) => Some(uri),
                // Downloads often holds incomplete, video-only or misnamed
                // files. These are not playback errors and must not flood
                // normal logs.
                Err(error) => {
                    log::debug!("Skipping local file {}: {error:#}", path.display());
                    None
                }
            });
        if let Some(uri) = uri {
            found.insert(
                uri.clone(),
                CachedFile {
                    uri,
                    path: path.to_path_buf(),
                    size,
                    mtime_ms,
                },
            );
        }
    });
    let mut files: Vec<_> = found.into_values().collect();
    // A stable order makes an unchanged folder write the same bytes again.
    files.sort_by(|left, right| left.uri.cmp(&right.uri));
    let uris = files
        .iter()
        .map(|file| (file.uri.clone(), file.path.clone()))
        .collect();
    Scan {
        uris,
        cache: Cache {
            version: VERSION,
            folder: Some(folder.to_path_buf()),
            files,
        },
    }
}

fn file_stamp(path: &Path) -> (u64, u64) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return (0, 0);
    };
    let mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_millis() as u64);
    (metadata.len(), mtime_ms)
}

fn visit(folder: &Path, file: &mut impl FnMut(&Path)) {
    // Check every component too: the selected root may itself be junk.
    if folder
        .components()
        .any(|part| part.as_os_str() == "__MACOSX")
    {
        return;
    }
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) => {
            log::warn!("Cannot scan local folder {}: {error}", folder.display());
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log::debug!("Skipping unreadable local folder entry: {error}");
                continue;
            }
        };
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with("._") {
            continue;
        }
        if path.is_dir() {
            visit(&path, file);
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ["mp3", "mp4", "m4p", "flac"]
                    .iter()
                    .any(|supported| ext.eq_ignore_ascii_case(supported))
            })
        {
            file(&path);
        }
    }
}

fn local_uri(path: &Path) -> Result<String> {
    let source = MediaSourceStream::new(Box::new(std::fs::File::open(path)?), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(extension);
    }
    let mut probe = symphonia::default::get_probe().format(
        &hint,
        source,
        &Default::default(),
        &MetadataOptions::default(),
    )?;
    let duration = &probe
        .format
        .default_track()
        .context("missing audio track")?
        .codec_params;
    let seconds = duration
        .time_base
        .context("missing time base")?
        .calc_time(duration.n_frames.context("missing frame count")?)
        .seconds;

    // Container tags take precedence, just as in the playback lookup.
    let mut metadata = probe.format.metadata();
    if metadata.current().is_none()
        && let Some(tags) = probe.metadata.get()
    {
        metadata = tags;
    }
    let tags = metadata.skip_to_latest().context("missing audio tags")?;
    let part = |key| {
        let value = tags
            .tags()
            .iter()
            .rev()
            .find(|tag| tag.std_key == Some(key));
        let value = value.map(|tag| tag.value.to_string()).unwrap_or_default();
        // Spotify's local URI components use form encoding, including + for
        // spaces and %2B for a literal plus. Reuse our existing encoder.
        const FORM: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
            .remove(b'*')
            .remove(b'-')
            .remove(b'.')
            .remove(b'_');
        percent_encoding::utf8_percent_encode(&value, FORM)
            .to_string()
            .replace("%20", "+")
    };
    Ok(format!(
        "spotify:local:{}:{}:{}:{seconds}",
        part(StandardTagKey::Artist),
        part(StandardTagKey::Album),
        part(StandardTagKey::TrackTitle),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> PathBuf {
        let root = crate::test_dirs::temp_dir().join(format!(
            "spotifast-scan-test-{:016x}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// A playable MP3 with the tags the first fixture promises.
    fn playable_song(root: &Path) -> PathBuf {
        let text = |id: &[u8; 4], value: &str| {
            let mut body = vec![3];
            body.extend_from_slice(value.as_bytes());
            let mut frame = id.to_vec();
            frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
            frame.extend_from_slice(&[0, 0]);
            frame.extend(body);
            frame
        };
        let song = root.join("song.mp3");
        let mut audio = crate::local_tags::tests::mp3_with(&[
            text(b"TPE1", "Tan Biónica + *-_ ."),
            text(b"TALB", "An album"),
            text(b"TIT2", "Teléfonos: #1"),
        ]);
        // The tag fixtures have four MPEG frames. The player's duration
        // estimator needs more than sixteen, so provide a full second.
        let frame = audio[audio.len() - 417..].to_vec();
        for _ in 0..40 {
            audio.extend_from_slice(&frame);
        }
        std::fs::write(&song, audio).unwrap();
        song
    }

    #[test]
    fn the_walk_skips_mac_metadata_before_probing() {
        let root = folder();
        for name in ["album", "__MACOSX/nested", "album/__MACOSX", "._directory"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
        }
        for name in [
            "album/song.MP3",
            "album/other.flac",
            "album/._song.mp3",
            "__MACOSX/nested/song.mp3",
            "album/__MACOSX/song.mp3",
            "._directory/song.mp3",
            "notes.txt",
        ] {
            std::fs::write(root.join(name), []).unwrap();
        }
        let mut paths = Vec::new();
        visit(&root, &mut |path| paths.push(path.to_path_buf()));
        paths.sort();
        assert_eq!(
            paths,
            [root.join("album/other.flac"), root.join("album/song.MP3")]
        );
        visit(&root.join("__MACOSX/nested"), &mut |_| panic!("junk root"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_index_matches_the_player_and_ignores_invalid_files() {
        let root = folder();
        playable_song(&root);
        let expected: HashMap<_, _> =
            librespot_playback::local_file::create_local_file_lookup(std::slice::from_ref(&root))
                .iter()
                .map(|(uri, path)| (uri.to_uri(), path.to_path_buf()))
                .collect();
        assert_eq!(expected.len(), 1, "the fixture must be playable");
        std::fs::write(root.join("broken.mp3"), b"not audio").unwrap();
        assert_eq!(scan(&root), expected);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_saved_index_round_trips_and_keeps_a_known_file() {
        let root = folder();
        let song = playable_song(&root);
        let scan = scan_with(&root, None);
        assert_eq!(scan.uris.len(), 1);
        let cache_path = root.join("cache.json");
        std::fs::write(&cache_path, scan.cache.encode().unwrap()).unwrap();
        let loaded = Cache::load(&cache_path).expect("the saved index reads back");
        assert_eq!(loaded.folder.as_deref(), Some(root.as_path()));
        assert_eq!(loaded.uris(), scan.uris);

        // Same size, same timestamp: the file is trusted without probing it.
        let metadata = std::fs::metadata(&song).unwrap();
        let bytes = std::fs::read(&song).unwrap();
        std::fs::write(&song, vec![0; bytes.len()]).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&song)
            .unwrap()
            .set_modified(metadata.modified().unwrap())
            .unwrap();
        let again = scan_with(&root, Some(&loaded));
        assert_eq!(again.uris, scan.uris, "an unchanged file is not re-probed");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_changed_file_is_probed_again() {
        let root = folder();
        let song = playable_song(&root);
        let cache = scan_with(&root, None).cache;

        // A changed modification time (and contents) means the old URI is
        // stale. The file no longer probes, so it leaves the index.
        std::fs::write(&song, b"not audio").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&song)
            .unwrap()
            .set_modified(UNIX_EPOCH + std::time::Duration::from_secs(10))
            .unwrap();
        assert!(scan_with(&root, Some(&cache)).uris.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_old_or_future_cache_only_means_a_fresh_walk() {
        let root = folder();
        playable_song(&root);
        let cache = scan_with(&root, None).cache;
        let cache_path = root.join("cache.json");

        assert!(Cache::load(&cache_path).is_none(), "missing");
        let mut value: serde_json::Value =
            serde_json::from_slice(&cache.encode().unwrap()).unwrap();
        value["version"] = 0.into();
        std::fs::write(&cache_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            Cache::load(&cache_path).map(|cache| cache.uris()),
            Some(cache.uris()),
            "an older index stays readable"
        );
        value["version"] = (VERSION + 1).into();
        std::fs::write(&cache_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            Cache::load(&cache_path).is_none(),
            "a future index is ignored"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
