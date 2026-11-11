//! Diagnostic: which audio files Spotify lists for a track, in the track's own
//! metadata and in the audio-files extension, with the stored playback
//! credential, and whether Spotify grants the audio key for one. A new release
//! lists its files only in the extension, so librespot refuses it with "no
//! alternatives found"; read from there, Spotify still refuses its key, which
//! only Spotify's own player (Soloist) can get.
//!
//!   cargo run --example audio_files_probe -- spotify:track:<id> ...

use librespot_core::{Session, SessionConfig, SpotifyUri, cache::Cache};
use librespot_protocol::extension_kind::ExtensionKind;
use librespot_protocol::metadata::{AudioFile, Track, audio_file::Format};
use protobuf::Message as _;

fn main() -> anyhow::Result<()> {
    fastframe_log::Logging::new("spotifast", env!("CARGO_PKG_VERSION"))
        .filter("warn")
        .init()?;
    let uris: Vec<String> = std::env::args().skip(1).collect();
    let dirs = spotifast::paths::AppDirs::discover()?.dirs;
    let cache = Cache::new::<&std::path::Path>(None, None, None, None)?.with_memory_credentials();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let store = spotifast::credentials::Store::new(dirs);
        let loaded = store.lease(spotifast::credentials::Slot::Playback).load().await?;
        let Some(spotifast::credentials::Grant::Playback(credentials)) = loaded.grant else {
            anyhow::bail!("Enable playback in Siphon first");
        };
        let session = Session::new(SessionConfig::default(), Some(cache));
        session.connect(credentials, false).await?;
        println!("country {}", session.country());
        for uri in uris {
            let id = SpotifyUri::from_uri(&uri)?;
            let track = session.spclient().get_track_metadata(&id).await?;
            let track = Track::parse_from_bytes(&track)?;
            println!(
                "{uri}: {:?} files in metadata {:?}, alternatives {}, restrictions {}, availability {}",
                track.name(),
                track.file.iter().map(|file| file.format()).collect::<Vec<_>>(),
                track.alternative.len(),
                track.restriction.len(),
                track.availability.len(),
            );
            for restriction in &track.restriction {
                println!("  restriction {restriction:?}");
            }
            let listed = match session
                .spclient()
                .get_metadata(ExtensionKind::AUDIO_FILES, &id)
                .await
            {
                Ok(bytes) => fields(&bytes, 1)
                    .into_iter()
                    .flat_map(|entry| fields(entry, 1))
                    .filter_map(|file| AudioFile::parse_from_bytes(file).ok())
                    .collect(),
                Err(error) => {
                    println!("  audio-files extension: {error}");
                    Vec::new()
                }
            };
            println!(
                "  audio-files extension: {:?}",
                listed.iter().map(|file| file.format()).collect::<Vec<_>>()
            );
            // Whether Spotify grants the key for the first Vorbis file it lists.
            let vorbis = track.file.iter().chain(&listed).find(|file| {
                matches!(file.format(), Format::OGG_VORBIS_160 | Format::OGG_VORBIS_320)
            });
            if let Some(file) = vorbis {
                let track_id = librespot_core::SpotifyId::try_from(&id)?;
                let file_id = librespot_core::FileId::from(file.file_id());
                match session.audio_key().request(track_id, file_id).await {
                    Ok(_) => println!("  audio key granted"),
                    Err(error) => println!("  audio key refused: {error}"),
                }
            }
        }
        Ok(())
    })
}

/// The length-delimited fields numbered `wanted` in one protobuf message.
fn fields(mut bytes: &[u8], wanted: u64) -> Vec<&[u8]> {
    fn varint(bytes: &mut &[u8]) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = bytes.split_first()?;
            *bytes = rest;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }
    let mut out = Vec::new();
    while let Some(key) = varint(&mut bytes) {
        match key & 7 {
            0 => {
                varint(&mut bytes);
            }
            1 => bytes = bytes.get(8..).unwrap_or_default(),
            5 => bytes = bytes.get(4..).unwrap_or_default(),
            2 => {
                let Some(len) = varint(&mut bytes) else { break };
                let len = len as usize;
                if len > bytes.len() {
                    break;
                }
                if key >> 3 == wanted {
                    out.push(&bytes[..len]);
                }
                bytes = &bytes[len..];
            }
            _ => break,
        }
    }
    out
}
