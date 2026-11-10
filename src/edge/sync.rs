//! The edge's small per-listener sync store: playlists, the queue and the
//! saved Spotify matches. Revision numbers come from the service; a `409`
//! means a phone wrote first, so the stored revision is read again before the
//! snapshot is offered once more.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::downloads::matching;
use crate::downloads::saved::{Saved, SavedMatch};

use super::{Scope, Source};

/// A playlist as the phone lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistSummary {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub image: Option<String>,
    pub track_count: u32,
}

/// One song of a mixed playlist, which lives only on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixedTrack {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub artists: Vec<String>,
    pub duration_ms: u32,
    pub image: Option<String>,
    /// Where the phone finds this song's audio on the edge. A playlist local
    /// file names its upload key as `upload:<key>`; nothing else sets it.
    pub source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixedPlaylist {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub image: Option<String>,
    pub tracks: Vec<MixedTrack>,
}

/// The playlists document. `library.mjs` reads mixed rows from `mixed` and
/// their songs from `tracks`, so the shape here is the one it already accepts.
pub fn playlists_doc(spotify: &[PlaylistSummary], mixed: &[MixedPlaylist]) -> Value {
    let spotify: Vec<Value> = spotify
        .iter()
        .map(|list| {
            json!({
                "source": "spotify",
                "id": list.id,
                "uri": list.uri,
                "name": list.name,
                "image": list.image,
                "track_count": list.track_count,
            })
        })
        .collect();
    let mixed: Vec<Value> = mixed
        .iter()
        .map(|list| {
            let tracks: Vec<Value> = list
                .tracks
                .iter()
                .map(|track| {
                    let mut entry = json!({
                        "id": track.id,
                        "track": {
                            "uri": track.uri,
                            "name": track.name,
                            "artists": track.artists.iter().map(|name| json!({ "name": name })).collect::<Vec<_>>(),
                            "duration_ms": track.duration_ms,
                            "album": track.image.as_ref().map(|url| json!({ "images": [{ "url": url }] })),
                        },
                    });
                    if let Some(source) = &track.source {
                        entry["source"] = json!(source);
                        entry["track"]["source"] = json!(source);
                    }
                    entry
                })
                .collect();
            json!({
                "source": "mixed",
                "mixed": true,
                "id": list.id,
                "uri": list.uri,
                "name": list.name,
                "image": list.image,
                "track_count": tracks.len(),
                "tracks": tracks,
            })
        })
        .collect();
    json!({ "playlists": spotify, "mixed": mixed })
}

/// The queue and where it is, with what plays first.
pub fn queue_doc(
    playing: Option<&crate::remote::Song>,
    position_ms: u32,
    is_playing: bool,
    upcoming: &[crate::remote::Song],
) -> Value {
    json!({
        "playing": playing,
        "position_ms": position_ms,
        "playing_state": is_playing,
        "queue": upcoming,
    })
}

/// A saved match as the edge stores it: only public YouTube or SoundCloud
/// sources qualify, and an older file that kept only a URL is read too.
pub(crate) fn saved_entry(saved: &SavedMatch) -> Option<(&'static str, String)> {
    if let Some(id) = saved.uri.strip_prefix("youtube:video:") {
        if id.len() == 11 {
            return Some(("youtube", saved.uri.clone()));
        }
    }
    if saved.uri.starts_with("soundcloud:") && !saved.uri.contains(char::is_whitespace) {
        return Some(("soundcloud", saved.uri.clone()));
    }
    if let Some(permalink) = crate::youtube::parse(&saved.uri) {
        return Some(("youtube", permalink));
    }
    if let Some(rest) = soundcloud_path(&saved.source) {
        return Some(("soundcloud", format!("soundcloud:{rest}")));
    }
    if saved.source.starts_with("soundcloud:") {
        return Some(("soundcloud", saved.source.clone()));
    }
    None
}

fn soundcloud_path(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = url
        .strip_prefix("https://soundcloud.com/")
        .or_else(|| url.strip_prefix("http://soundcloud.com/"))?;
    let path = rest.split(['?', '#']).next()?.trim_end_matches('/');
    (!path.is_empty() && !path.contains(char::is_whitespace)).then(|| path.to_owned())
}

/// The matches document: every saved Spotify song as an automatic choice.
pub(crate) fn matches_doc(saved: &std::collections::BTreeMap<String, SavedMatch>) -> Value {
    let mut entries = serde_json::Map::new();
    for (uri, found) in saved {
        if !uri.starts_with("spotify:track:") {
            continue;
        }
        let Some((source, source_uri)) = saved_entry(found) else {
            continue;
        };
        entries.insert(
            uri.clone(),
            json!({
                "source": source,
                "sourceUri": source_uri,
                "confidence": 0.9,
                "date": found.at.max(0).saturating_mul(1000),
                "manual": false,
            }),
        );
    }
    Value::Object(entries)
}

/// A public URL for a source URI, so a match taken back from the edge can be
/// remembered locally the same way a search would have.
fn source_url(source_uri: &str) -> Option<String> {
    if let Some(id) = source_uri.strip_prefix("youtube:video:") {
        return (id.len() == 11).then(|| format!("https://www.youtube.com/watch?v={id}"));
    }
    let rest = source_uri.strip_prefix("soundcloud:")?;
    (!rest.is_empty() && !rest.contains(char::is_whitespace))
        .then(|| format!("https://soundcloud.com/{rest}"))
}

pub struct Sync {
    source: Arc<Source>,
    revisions: tokio::sync::Mutex<std::collections::HashMap<&'static str, u64>>,
}

impl Sync {
    pub fn new(source: Arc<Source>) -> Self {
        Self {
            source,
            revisions: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    async fn http(&self) -> Result<(reqwest::Client, super::Tokens, String), String> {
        let (tokens, token) = self.source.token(Scope::Sync).await?;
        let client = self.source.http().client()?;
        Ok((client, tokens, token))
    }

    pub async fn get(&self, doc: &'static str) -> Result<Value, String> {
        let (client, tokens, token) = self.http().await?;
        let response = client
            .get(format!("{}/v1/sync/{doc}", tokens.url))
            .bearer_auth(token)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
            .map_err(|_| "The edge could not be reached.".to_owned())?;
        if !response.status().is_success() {
            return Err("The edge refused that sync read.".into());
        }
        let record: Value = response
            .json()
            .await
            .map_err(|_| "The edge returned an unreadable sync record.".to_owned())?;
        if let Some(revision) = record.get("revision").and_then(Value::as_u64) {
            self.revisions.lock().await.insert(doc, revision);
        }
        Ok(record)
    }

    /// Offers `data` as the document's next revision. A conflict is not an
    /// error: the stored revision is read and the snapshot offered again.
    pub async fn put(&self, doc: &'static str, data: Value) -> Result<(), String> {
        for attempt in 0..2 {
            let (client, tokens, token) = self.http().await?;
            let revision = self.revisions.lock().await.get(doc).copied();
            let mut body = json!({ "data": data.clone() });
            if let Some(revision) = revision {
                body["revision"] = json!(revision);
            }
            let response = client
                .put(format!("{}/v1/sync/{doc}", tokens.url))
                .bearer_auth(token)
                .json(&body)
                .timeout(std::time::Duration::from_secs(20))
                .send()
                .await
                .map_err(|_| "The edge could not be reached.".to_owned())?;
            if response.status().as_u16() == 409 && attempt == 0 {
                self.get(doc).await?;
                continue;
            }
            if !response.status().is_success() {
                return Err("The edge refused that sync write.".into());
            }
            if let Ok(record) = response.json::<Value>().await
                && let Some(revision) = record.get("revision").and_then(Value::as_u64)
            {
                self.revisions.lock().await.insert(doc, revision);
            }
            return Ok(());
        }
        Err("The edge's sync revision kept moving.".into())
    }

    /// Sends every saved match, and keeps the edge's automatic answers. A
    /// hand-chosen edge match is never overwritten or copied over a local one.
    pub(crate) async fn matches(&self, saved: &Saved) -> Result<(), String> {
        let remote = self.get("matches").await?;
        take_matches(remote.get("data"), saved).await;
        self.put("matches", matches_doc(&saved.all().await)).await
    }
}

/// Takes the edge's automatic matches back into the local file, so a later
/// play still finds them when the edge is unreachable.
async fn take_matches(data: Option<&Value>, saved: &Saved) {
    let Some(entries) = data.and_then(Value::as_object) else {
        return;
    };
    for (uri, entry) in entries {
        if !uri.starts_with("spotify:track:") {
            continue;
        }
        if entry
            .get("manual")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let Some(source_uri) = entry.get("sourceUri").and_then(Value::as_str) else {
            continue;
        };
        let Some(url) = source_url(source_uri) else {
            continue;
        };
        if saved.get(uri).await.is_none() {
            saved
                .remember(
                    uri,
                    &matching::Match {
                        uri: source_uri.to_owned(),
                        source: url,
                    },
                )
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_playlist_snapshot_is_the_shape_the_edge_reads() {
        let doc = playlists_doc(
            &[PlaylistSummary {
                id: "abc".into(),
                uri: "spotify:playlist:abc".into(),
                name: "Saved".into(),
                image: Some("https://i.example/a".into()),
                track_count: 3,
            }],
            &[MixedPlaylist {
                id: "m1".into(),
                uri: "spotifast:playlist:m1".into(),
                name: "Across".into(),
                image: None,
                tracks: vec![
                    MixedTrack {
                        id: "e1".into(),
                        uri: "spotify:local:A:B:C:180".into(),
                        name: "Local".into(),
                        artists: vec!["A".into()],
                        duration_ms: 180_000,
                        image: None,
                        source: Some("upload:local-abc".into()),
                    },
                    MixedTrack {
                        id: "e2".into(),
                        uri: "soundcloud:track:7".into(),
                        name: "Cloud".into(),
                        artists: vec!["B".into()],
                        duration_ms: 1_000,
                        image: Some("https://i.example/b".into()),
                        source: None,
                    },
                ],
            }],
        );
        assert_eq!(doc["playlists"][0]["uri"], "spotify:playlist:abc");
        assert_eq!(doc["playlists"][0]["track_count"], 3);
        let mixed = &doc["mixed"][0];
        assert_eq!(mixed["source"], "mixed");
        assert_eq!(mixed["track_count"], 2);
        assert_eq!(
            mixed["tracks"][0]["track"]["uri"],
            "spotify:local:A:B:C:180"
        );
        assert_eq!(mixed["tracks"][0]["source"], "upload:local-abc");
        assert_eq!(mixed["tracks"][1]["track"]["uri"], "soundcloud:track:7");
        assert_eq!(
            mixed["tracks"][1]["track"]["album"]["images"][0]["url"],
            "https://i.example/b"
        );
    }

    #[test]
    fn the_queue_record_carries_what_plays_and_where() {
        let song = crate::remote::Song {
            uri: "spotify:track:a".into(),
            name: "A".into(),
            artists: vec!["X".into()],
            duration_ms: 200_000,
            ..Default::default()
        };
        let doc = queue_doc(Some(&song), 12_000, true, std::slice::from_ref(&song));
        assert_eq!(doc["playing"]["uri"], "spotify:track:a");
        assert_eq!(doc["position_ms"], 12_000);
        assert_eq!(doc["playing_state"], true);
        assert_eq!(doc["queue"][0]["name"], "A");
    }

    #[test]
    fn only_public_sources_are_offered_as_matches() {
        let saved = SavedMatch {
            source: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            uri: "youtube:video:abcdefghijk".into(),
            at: 10,
        };
        assert_eq!(
            saved_entry(&saved),
            Some(("youtube", "youtube:video:abcdefghijk".into()))
        );
        let cloud = SavedMatch {
            source: "https://soundcloud.com/artist/song".into(),
            uri: String::new(),
            at: 0,
        };
        assert_eq!(
            saved_entry(&cloud),
            Some(("soundcloud", "soundcloud:artist/song".into()))
        );
        let private = SavedMatch {
            source: "https://soundcloud.com/artist/song?token=secret".into(),
            uri: String::new(),
            at: 0,
        };
        // The trailing token is dropped with the query, and the public path
        // is still a usable source.
        assert_eq!(
            saved_entry(&private),
            Some(("soundcloud", "soundcloud:artist/song".into()))
        );
        assert_eq!(
            saved_entry(&SavedMatch {
                source: "https://example.com/x".into(),
                uri: "https://example.com/x".into(),
                at: 0,
            }),
            None
        );
    }

    #[test]
    fn a_match_document_names_the_date_in_milliseconds() {
        let mut saved = std::collections::BTreeMap::new();
        saved.insert(
            "spotify:track:a".into(),
            SavedMatch {
                source: "https://www.youtube.com/watch?v=abcdefghijk".into(),
                uri: "youtube:video:abcdefghijk".into(),
                at: 20,
            },
        );
        let doc = matches_doc(&saved);
        assert_eq!(doc["spotify:track:a"]["source"], "youtube");
        assert_eq!(doc["spotify:track:a"]["confidence"], 0.9);
        assert_eq!(doc["spotify:track:a"]["date"], 20_000);
        assert_eq!(doc["spotify:track:a"]["manual"], false);
    }
}
