//! Songs people share with their group from their own files. The group service
//! holds them, and limits what fits. See `services/friends/src/audio.ts`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::api::models::{ArtistRef, Track};

/// The whole list, played as one context.
pub const LIBRARY: &str = "encore:songs";
const SONG_PREFIX: &str = "encore:song:";

/// Longest song the service takes; checked here first to say why.
pub const MAX_MS: u32 = 15 * 60 * 1000;
pub const MAX_BYTES: u64 = 40 * 1024 * 1024;
/// What the service accepts, which is also what the player decodes.
pub const EXTENSIONS: [&str; 4] = ["mp3", "flac", "m4a", "aac"];

/// One shared song. Its file's address comes only when it is played, so that
/// plays can be counted against the month's downloads.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedSong {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub duration_ms: u32,
    #[serde(default)]
    pub size: u64,
    pub by: String,
    #[serde(default)]
    pub by_name: String,
    #[serde(default)]
    pub created_at: u64,
}

impl SharedSong {
    pub fn uri(&self) -> String {
        format!("{SONG_PREFIX}{}", self.id)
    }

    pub fn to_track(&self) -> Track {
        Track {
            uri: self.uri(),
            id: Some(self.id.clone()),
            name: self.title.clone(),
            duration_ms: self.duration_ms,
            artists: self
                .artists
                .iter()
                .map(|name| ArtistRef {
                    name: name.clone(),
                    ..ArtistRef::default()
                })
                .collect(),
            ..Track::default()
        }
    }
}

pub fn is_song(uri: &str) -> bool {
    song_id(uri).is_some()
}

/// The song's id, which goes into a request path, so only its own shape passes.
pub fn song_id(uri: &str) -> Option<&str> {
    uri.strip_prefix(SONG_PREFIX)
        .filter(|id| id.len() == 36 && id.bytes().all(|b| b == b'-' || b.is_ascii_hexdigit()))
}

/// What a file says about itself, for the request that reserves its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub title: String,
    pub artists: Vec<String>,
    pub duration_ms: u32,
    pub size: u64,
    pub ext: String,
}

/// Reads a chosen file on a blocking worker, and says plainly if it can't be shared.
pub fn inspect(path: &Path) -> Result<FileInfo, String> {
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let ext = path
        .extension()
        .map_or_else(String::new, |ext| ext.to_string_lossy().to_lowercase());
    if !EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "{name}: only MP3, FLAC, M4A and AAC files can be shared."
        ));
    }
    let size = std::fs::metadata(path)
        .map_err(|_| format!("{name} could not be read."))?
        .len();
    if size > MAX_BYTES {
        return Err(format!(
            "{name} is too big to share. The limit is {} MB.",
            MAX_BYTES / (1024 * 1024)
        ));
    }
    let tags = read_tags(path).ok_or_else(|| format!("{name} could not be read as a song."))?;
    let duration_ms = tags.duration_ms;
    if duration_ms > MAX_MS {
        return Err(format!(
            "{name} is too long to share. The limit is {} minutes.",
            MAX_MS / 60_000
        ));
    }
    let stem = path
        .file_stem()
        .map_or_else(|| name.clone(), |stem| stem.to_string_lossy().into_owned());
    Ok(FileInfo {
        title: tags.title.unwrap_or(stem),
        artists: tags.artist.into_iter().collect(),
        duration_ms,
        size,
        ext,
    })
}

struct Tags {
    title: Option<String>,
    artist: Option<String>,
    duration_ms: u32,
}

fn read_tags(path: &Path) -> Option<Tags> {
    use symphonia::core::{
        io::MediaSourceStream,
        meta::{MetadataOptions, StandardTagKey},
        probe::Hint,
    };
    let source = MediaSourceStream::new(
        Box::new(std::fs::File::open(path).ok()?),
        Default::default(),
    );
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(extension);
    }
    let mut probe = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &Default::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let params = &probe.format.default_track()?.codec_params;
    let seconds = params.time_base?.calc_time(params.n_frames?);
    let duration_ms = u32::try_from(seconds.seconds.saturating_mul(1000))
        .ok()?
        .saturating_add((seconds.frac * 1000.0) as u32);
    let mut metadata = probe.format.metadata();
    if metadata.current().is_none()
        && let Some(found) = probe.metadata.get()
    {
        metadata = found;
    }
    let read = |key| {
        metadata.current().and_then(|revision| {
            revision
                .tags()
                .iter()
                .rev()
                .find(|tag| tag.std_key == Some(key))
                .map(|tag| tag.value.to_string().trim().to_owned())
                .filter(|value| !value.is_empty())
        })
    };
    let title = read(StandardTagKey::TrackTitle);
    let artist = read(StandardTagKey::Artist);
    (duration_ms > 0).then_some(Tags {
        title,
        artist,
        duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

    #[test]
    fn a_song_uri_round_trips_and_nothing_else_passes() {
        let song = SharedSong {
            id: ID.into(),
            ..SharedSong::default()
        };
        assert_eq!(song_id(&song.uri()), Some(ID));
        for uri in [
            LIBRARY,
            "encore:song:",
            "encore:song:../../admin",
            "encore:song:0f8fad5b-d9cb-469f-a165-70867728950e/play",
            "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
        ] {
            assert!(!is_song(uri), "{uri}");
        }
    }

    #[test]
    fn a_song_becomes_a_track_the_player_understands() {
        let track = SharedSong {
            id: ID.into(),
            title: "Midnight".into(),
            artists: vec!["Ana".into()],
            duration_ms: 1000,
            ..SharedSong::default()
        }
        .to_track();
        assert_eq!(track.uri, format!("encore:song:{ID}"));
        assert_eq!(track.artists[0].name, "Ana");
        assert!(crate::youtube::is_external(&track.uri));
    }

    #[test]
    fn a_file_that_cannot_be_shared_says_why() {
        let error = inspect(Path::new("notes.txt")).unwrap_err();
        assert!(error.contains("MP3"), "{error}");
    }
}
