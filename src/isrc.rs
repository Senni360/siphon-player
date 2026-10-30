//! Exact, one-way SoundCloud to Spotify recording matches. Never used for audio.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::api::{ApiClient, models::Track};

/// Owned by one account/session's track-detail jobs. Private SoundCloud track
/// identities never survive an account change or get written to shared disk.
#[derive(Default)]
pub struct Cache(Mutex<HashMap<(String, String, u32), Match>>);

impl Cache {
    fn get(&self, uri: &str, code: &str, duration_ms: u32) -> Option<Match> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&(uri.to_owned(), code.to_owned(), duration_ms))
            .cloned()
    }

    fn remember(&self, found: Match) {
        let mut entries = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Only positive matches are cached; bound long listening sessions.
        if entries.len() >= 512 {
            entries.clear();
        }
        entries.insert(
            (
                found.soundcloud_uri.clone(),
                found.isrc.clone(),
                found.soundcloud_duration_ms,
            ),
            found,
        );
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Match {
    pub soundcloud_uri: String,
    pub isrc: String,
    pub soundcloud_duration_ms: u32,
    pub spotify_uri: String,
    pub spotify_duration_ms: u32,
}

/// Reject malformed codes rather than turning arbitrary metadata into a query.
pub fn normalize(code: &str) -> Option<String> {
    let code = code.trim().replace('-', "").to_ascii_uppercase();
    let bytes = code.as_bytes();
    (bytes.len() == 12
        && bytes[..2].iter().all(u8::is_ascii_alphabetic)
        && bytes[2..5].iter().all(u8::is_ascii_alphanumeric)
        && bytes[5..].iter().all(u8::is_ascii_digit))
    .then_some(code)
}

fn same_recording(track: &Track, code: &str, duration_ms: u32) -> bool {
    matches!(
        librespot_core::SpotifyUri::from_uri(&track.uri),
        Ok(librespot_core::SpotifyUri::Track { .. })
    ) && track
        .external_ids
        .isrc
        .as_deref()
        .and_then(normalize)
        .as_deref()
        == Some(code)
        && duration_ms > 0
        && track.duration_ms > 0
        && track.duration_ms.abs_diff(duration_ms) <= 3_000
}

/// Runs on the backend runtime. Only verified positive matches are cached;
/// a missing grant, rate limit or empty search never poisons later lookups.
pub async fn lookup(
    web: &ApiClient,
    cache: &Cache,
    soundcloud_uri: &str,
    code: &str,
    duration_ms: u32,
) -> Result<Option<Match>> {
    if crate::soundcloud::track_id(soundcloud_uri).is_none() || duration_ms == 0 {
        return Ok(None);
    }
    let Some(code) = normalize(code) else {
        return Ok(None);
    };
    if let Some(found) = cache.get(soundcloud_uri, &code, duration_ms) {
        return Ok(Some(found));
    }
    // Use the same ISRC query representation as Track::recording_key().
    let query_track = Track {
        external_ids: crate::api::models::ExternalIds {
            isrc: Some(code.clone()),
        },
        ..Default::default()
    };
    let query = query_track.recording_key().expect("validated ISRC");
    let results = web.search(&query, &["track"]).await?;
    let found = results
        .tracks
        .into_iter()
        .flat_map(|page| page.items)
        .filter(|track| same_recording(track, &code, duration_ms))
        .min_by_key(|track| track.duration_ms.abs_diff(duration_ms))
        .map(|track| Match {
            soundcloud_uri: soundcloud_uri.to_owned(),
            isrc: code,
            soundcloud_duration_ms: duration_ms,
            spotify_uri: track.uri,
            spotify_duration_ms: track.duration_ms,
        });
    if let Some(found) = &found {
        cache.remember(found.clone());
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isrc_search_requires_the_same_code_and_recording_length() {
        let mut track = Track {
            uri: "spotify:track:4uLU6hMCjMI75M1A2tKUQC".into(),
            duration_ms: 203_000,
            external_ids: crate::api::models::ExternalIds {
                isrc: Some("GB-ABC-22-12345".into()),
            },
            ..Default::default()
        };
        assert!(same_recording(&track, "GBABC2212345", 200_000));
        assert!(!same_recording(&track, "GBABC2212345", 199_999));
        assert!(!same_recording(&track, "GBABC2212346", 200_000));
        assert!(!same_recording(&track, "GBABC2212345", 0));
        track.external_ids.isrc = None;
        assert!(!same_recording(&track, "GBABC2212345", 200_000));
        assert_eq!(
            normalize(" gb-abc-22-12345 ").as_deref(),
            Some("GBABC2212345")
        );
        assert_eq!(normalize("GBABC2212345 OR title:other"), None);
        assert_eq!(normalize(""), None);
    }

    #[test]
    fn cache_requires_the_source_identity_code_and_full_duration() {
        let cache = Cache::default();
        cache.remember(Match {
            soundcloud_uri: "soundcloud:track:123".into(),
            isrc: "GBABC2212345".into(),
            soundcloud_duration_ms: 200_000,
            spotify_uri: "spotify:track:4uLU6hMCjMI75M1A2tKUQC".into(),
            spotify_duration_ms: 200_001,
        });
        assert!(
            cache
                .get("soundcloud:track:123", "GBABC2212345", 200_000)
                .is_some()
        );
        assert!(
            cache
                .get("soundcloud:track:124", "GBABC2212345", 200_000)
                .is_none()
        );
        assert!(
            cache
                .get("soundcloud:track:123", "GBABC2212346", 200_000)
                .is_none()
        );
        assert!(
            cache
                .get("soundcloud:track:123", "GBABC2212345", 30_000)
                .is_none()
        );
        assert!(
            Cache::default()
                .get("soundcloud:track:123", "GBABC2212345", 200_000)
                .is_none()
        );
    }
}
