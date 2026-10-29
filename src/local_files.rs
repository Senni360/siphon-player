//! The UI's local-file index. Call on a blocking worker, never the UI thread.
//!
//! The pinned player's separate startup index still needs the corresponding
//! librespot fix. Keep URI matching compatible with it until that lands.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use symphonia::core::{
    io::MediaSourceStream,
    meta::{MetadataOptions, StandardTagKey},
    probe::Hint,
};

pub(crate) fn scan(folder: &Path) -> HashMap<String, PathBuf> {
    let mut found = HashMap::new();
    visit(folder, &mut |path| match local_uri(path) {
        Ok(uri) => {
            found.insert(uri, path.to_path_buf());
        }
        // Downloads often holds incomplete, video-only or misnamed files.
        // These are not playback errors and must not flood normal logs.
        Err(error) => log::debug!("Skipping local file {}: {error:#}", path.display()),
    });
    found
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
        let root = std::env::temp_dir().join(format!(
            "spotifast-scan-test-{:016x}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
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
}
