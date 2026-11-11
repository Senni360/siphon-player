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
    /// The album's name, which the matcher looks the song up by on YouTube Music.
    pub album: Option<String>,
    /// The upload key of a playlist local file, so the phone can ask the edge
    /// for its audio. Nothing else sets it.
    pub upload: Option<String>,
    /// A SoundCloud song's own public page, which the edge plays it from.
    /// Never a private link: see [`public_soundcloud_link`].
    pub link: Option<String>,
}

/// A SoundCloud song's page when anyone may open it: `soundcloud.com/artist/song`
/// and nothing more. A private link carries a secret third part or a query, and
/// is not something to leave on a server.
pub fn public_soundcloud_link(link: Option<&str>) -> Option<String> {
    let link = crate::soundcloud::permalink(link)?;
    let url = reqwest::Url::parse(link).ok()?;
    let plain = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    };
    let parts: Vec<&str> = url.path().trim_matches('/').split('/').collect();
    (url.query().is_none()
        && url.fragment().is_none()
        && parts.len() == 2
        && parts.iter().all(|part| plain(part)))
    .then(|| link.to_owned())
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
                            "album": album_doc(track),
                        },
                    });
                    if let Some(link) = &track.link {
                        entry["track"]["link"] = json!(link);
                    }
                    if let Some(upload) = &track.upload {
                        entry["upload"] = json!(upload);
                        entry["track"]["upload"] = json!(upload);
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

/// A mixed song's album: its cover and its name, whichever are known.
fn album_doc(track: &MixedTrack) -> Value {
    let mut album = serde_json::Map::new();
    if let Some(url) = &track.image {
        album.insert("images".into(), json!([{ "url": url }]));
    }
    if let Some(name) = &track.album {
        album.insert("name".into(), json!(name));
    }
    if album.is_empty() {
        Value::Null
    } else {
        Value::Object(album)
    }
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
    match_source(&saved.uri).or_else(|| match_source(&saved.source))
}

/// The edge's canonical source for a saved match, or `None` when the match
/// names something it may not be given: a private SoundCloud path, a numeric
/// track URI, another host, or a YouTube link that is not a plain video.
pub(crate) fn match_source(text: &str) -> Option<(&'static str, String)> {
    // `youtube::parse` canonicalises a track URI and a watch link, and refuses
    // everything else. Only the canonical track URI is ever sent.
    if let Some(uri) = crate::youtube::parse(text).filter(|uri| crate::youtube::is_track_uri(uri)) {
        return Some(("youtube", uri));
    }
    soundcloud_path(text).map(|path| ("soundcloud", format!("soundcloud:{path}")))
}

/// The public path of a SoundCloud permalink or `soundcloud:` URI. A private
/// link's `s-` path segment, a query or fragment, or anything that is not a
/// SoundCloud permalink is refused: the edge must only see what is public.
fn soundcloud_path(url: &str) -> Option<String> {
    let text = url.trim();
    let rest = text
        .strip_prefix("https://soundcloud.com/")
        .or_else(|| text.strip_prefix("http://soundcloud.com/"))
        .or_else(|| text.strip_prefix("soundcloud:"))?;
    let path = rest.split(['?', '#']).next()?.trim_end_matches('/');
    if path.is_empty()
        || path.contains(char::is_whitespace)
        || path.contains('\\')
        || path.contains("..")
    {
        return None;
    }
    let segments: Vec<&str> = path.split('/').collect();
    // The edge's resolver accepts a track permalink, never a bare user page.
    if !(2..=5).contains(&segments.len()) {
        return None;
    }
    for segment in &segments {
        if !segment.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        }) {
            return None;
        }
        // `s-…` is SoundCloud's private share path; it must never leave here.
        if segment.to_ascii_lowercase().starts_with("s-") {
            return None;
        }
    }
    Some(path.to_owned())
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
                // A hand-edited file must not make the edge refuse the whole
                // document as an invalid date.
                "date": found.at.clamp(0, 4_000_000_000).saturating_mul(1000),
                "manual": false,
            }),
        );
    }
    Value::Object(entries)
}

/// What the local match store keeps for an edge source, so a match taken back
/// from the edge is remembered the same way a search would have. A SoundCloud
/// match keeps its public URL only: the local store's playable form needs a
/// numeric track id, which the edge's answer does not carry.
fn local_match(source_uri: &str) -> Option<matching::Match> {
    let (provider, canonical) = match_source(source_uri)?;
    if provider == "youtube" {
        return Some(matching::Match {
            uri: canonical.clone(),
            source: crate::youtube::permalink(&canonical)?,
        });
    }
    let path = canonical.strip_prefix("soundcloud:")?;
    Some(matching::Match {
        uri: String::new(),
        source: format!("https://soundcloud.com/{path}"),
    })
}

pub(crate) struct Sync {
    source: Arc<Source>,
    revisions: tokio::sync::Mutex<std::collections::HashMap<&'static str, u64>>,
}

/// Why an offer was refused. A snapshot the service can never accept, such as
/// one over its size limit, is dropped instead of being offered forever.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Failure {
    retry: bool,
}

impl Failure {
    fn retry() -> Self {
        Self { retry: true }
    }

    fn permanent() -> Self {
        Self { retry: false }
    }
}

impl Sync {
    pub(crate) fn new(source: Arc<Source>) -> Self {
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

    pub(crate) async fn get(&self, doc: &'static str) -> Result<Value, String> {
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
    pub(crate) async fn put(&self, doc: &'static str, data: &Value) -> Result<(), Failure> {
        let mut attempt = 0u8;
        loop {
            attempt = attempt.saturating_add(1);
            let (client, tokens, token) = self.http().await.map_err(|_| Failure::retry())?;
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
                .map_err(|_| Failure::retry())?;
            let status = response.status().as_u16();
            if status == 409 && attempt < 3 {
                self.get(doc).await.map_err(|_| Failure::retry())?;
                continue;
            }
            // A token that ran out between the check and the request is
            // replaced once, then the same snapshot is offered again.
            if status == 401 && attempt < 3 {
                self.source.forget_tokens().await;
                continue;
            }
            if !response.status().is_success() {
                // The size and shape of a snapshot are the caller's, not a
                // passing condition: offering it again cannot make it fit.
                return Err(if matches!(status, 400 | 413 | 415 | 422) {
                    Failure::permanent()
                } else {
                    Failure::retry()
                });
            }
            if let Ok(record) = response.json::<Value>().await
                && let Some(revision) = record.get("revision").and_then(Value::as_u64)
            {
                self.revisions.lock().await.insert(doc, revision);
            }
            return Ok(());
        }
    }

    /// Sends every saved match, and keeps the edge's automatic answers. A
    /// hand-chosen edge match is never overwritten or copied over a local one.
    pub(crate) async fn matches(&self, saved: &Saved) -> Result<(), Failure> {
        let remote = self.get("matches").await.map_err(|_| Failure::retry())?;
        take_matches(remote.get("data"), saved).await;
        self.put("matches", &matches_doc(&saved.all().await)).await
    }
}

/// The edge is not always reachable, and a snapshot offered while it is down
/// would otherwise be lost until the next change. This keeps the newest one
/// per document and sends it again until the edge accepts it. Only the newest
/// matters: each document is a whole picture.
struct Pending {
    playlists: Option<Value>,
    queue: Option<Value>,
    matches: bool,
}

enum Next {
    Playlists(Value),
    Queue(Value),
    Matches,
    Wait,
}

/// How long a refused or unreachable push waits before it is offered again.
const RETRY: [u64; 6] = [1, 2, 4, 8, 16, 30];

/// Holds the snapshots the edge has not accepted yet. Dropping it stops the
/// task, so turning the connection off leaves nothing running.
pub(crate) struct Writer {
    pending: std::sync::Arc<tokio::sync::Mutex<Pending>>,
    wake: std::sync::Arc<tokio::sync::Notify>,
    task: tokio::task::JoinHandle<()>,
}

impl Writer {
    pub(crate) fn spawn(source: std::sync::Arc<Source>, saved: Saved) -> Self {
        let pending = std::sync::Arc::new(tokio::sync::Mutex::new(Pending {
            playlists: None,
            queue: None,
            matches: false,
        }));
        let wake = std::sync::Arc::new(tokio::sync::Notify::new());
        let task = tokio::spawn(run(
            Sync::new(source),
            saved,
            std::sync::Arc::clone(&pending),
            std::sync::Arc::clone(&wake),
        ));
        Self {
            pending,
            wake,
            task,
        }
    }

    /// The playlists snapshot, replacing one still waiting to be sent.
    pub async fn playlists(&self, doc: Value) {
        self.pending.lock().await.playlists = Some(doc);
        self.wake.notify_one();
    }

    /// The queue snapshot, replacing one still waiting to be sent.
    pub async fn queue(&self, doc: Value) {
        self.pending.lock().await.queue = Some(doc);
        self.wake.notify_one();
    }

    /// Push the saved matches, and take the edge's back, when the edge allows.
    pub async fn matches(&self) {
        self.pending.lock().await.matches = true;
        self.wake.notify_one();
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run(
    sync: Sync,
    saved: Saved,
    pending: std::sync::Arc<tokio::sync::Mutex<Pending>>,
    wake: std::sync::Arc<tokio::sync::Notify>,
) {
    let mut retry = 0usize;
    loop {
        let next = {
            let mut held = pending.lock().await;
            if let Some(doc) = held.playlists.take() {
                Next::Playlists(doc)
            } else if let Some(doc) = held.queue.take() {
                Next::Queue(doc)
            } else if held.matches {
                held.matches = false;
                Next::Matches
            } else {
                Next::Wait
            }
        };
        let (next, done) = match next {
            Next::Wait => {
                wake.notified().await;
                continue;
            }
            Next::Playlists(doc) => {
                let done = sync.put("playlists", &doc).await;
                (Next::Playlists(doc), done)
            }
            Next::Queue(doc) => {
                let done = sync.put("queue", &doc).await;
                (Next::Queue(doc), done)
            }
            Next::Matches => {
                let done = sync.matches(&saved).await;
                (Next::Matches, done)
            }
        };
        if let Err(failure) = done {
            if !failure.retry {
                // Offering it again cannot help; the next change starts over.
                retry = 0;
                continue;
            }
        } else {
            retry = 0;
            continue;
        }
        // Put it back, unless something newer arrived while it was in flight.
        {
            let mut held = pending.lock().await;
            match next {
                Next::Playlists(doc) => {
                    if held.playlists.is_none() {
                        held.playlists = Some(doc);
                    }
                }
                Next::Queue(doc) => {
                    if held.queue.is_none() {
                        held.queue = Some(doc);
                    }
                }
                Next::Matches => held.matches = true,
                Next::Wait => {}
            }
        }
        let wait = RETRY[retry.min(RETRY.len() - 1)];
        retry = retry.saturating_add(1);
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
    }
}

/// Takes the edge's automatic matches back into the local file, so a later
/// play still finds them when the edge is unreachable.
async fn take_matches(data: Option<&Value>, saved: &Saved) {
    let Some(entries) = data.and_then(Value::as_object) else {
        return;
    };
    let local = saved.all().await;
    let mut found = Vec::new();
    for (uri, entry) in entries {
        if !uri.starts_with("spotify:track:") || local.contains_key(uri) {
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
        if let Some(matched) = local_match(source_uri) {
            found.push((uri.clone(), matched));
        }
    }
    saved.remember_all(found).await;
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
                        upload: Some("local-abc".into()),
                        link: None,
                        album: None,
                    },
                    MixedTrack {
                        id: "e2".into(),
                        uri: "soundcloud:track:7".into(),
                        name: "Cloud".into(),
                        artists: vec!["B".into()],
                        duration_ms: 1_000,
                        image: Some("https://i.example/b".into()),
                        upload: None,
                        link: Some("https://soundcloud.com/b/cloud".into()),
                        album: Some("Clouds".into()),
                    },
                ],
            }],
        );
        assert_eq!(
            doc["mixed"][0]["tracks"][1]["track"]["link"],
            "https://soundcloud.com/b/cloud"
        );
        assert!(doc["mixed"][0]["tracks"][0]["track"].get("link").is_none());
        assert_eq!(
            doc["mixed"][0]["tracks"][1]["track"]["album"]["name"],
            "Clouds"
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
        assert_eq!(mixed["tracks"][0]["upload"], "local-abc");
        assert_eq!(mixed["tracks"][0]["track"]["upload"], "local-abc");
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
            ..Default::default()
        };
        assert_eq!(
            saved_entry(&saved),
            Some(("youtube", "youtube:video:abcdefghijk".into()))
        );
        // A real SoundCloud match keeps a numeric track URI and a public
        // permalink. The edge resolver accepts only the permalink.
        let cloud = SavedMatch {
            source: "https://soundcloud.com/artist/song".into(),
            uri: "soundcloud:track:12345".into(),
            at: 0,
            ..Default::default()
        };
        assert_eq!(
            saved_entry(&cloud),
            Some(("soundcloud", "soundcloud:artist/song".into()))
        );
        let private = SavedMatch {
            source: "https://soundcloud.com/artist/song/s-AbCdEf?si=secret".into(),
            uri: String::new(),
            at: 0,
            ..Default::default()
        };
        // A private `s-` path never leaves this computer, and the query is
        // never sent either.
        assert_eq!(saved_entry(&private), None);
        let private_uri = SavedMatch {
            source: String::new(),
            uri: "soundcloud:artist/song/s-AbCdEf".into(),
            at: 0,
            ..Default::default()
        };
        assert_eq!(saved_entry(&private_uri), None);
        let queried = SavedMatch {
            source: "https://soundcloud.com/artist/song?si=public".into(),
            uri: String::new(),
            at: 0,
            ..Default::default()
        };
        assert_eq!(
            saved_entry(&queried),
            Some(("soundcloud", "soundcloud:artist/song".into()))
        );
        let numeric = SavedMatch {
            source: String::new(),
            uri: "soundcloud:track:12345".into(),
            at: 0,
            ..Default::default()
        };
        assert_eq!(saved_entry(&numeric), None);
        assert_eq!(
            saved_entry(&SavedMatch {
                source: "https://example.com/x".into(),
                uri: "https://example.com/x".into(),
                at: 0,
                ..Default::default()
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
                ..Default::default()
            },
        );
        let doc = matches_doc(&saved);
        assert_eq!(doc["spotify:track:a"]["source"], "youtube");
        assert_eq!(doc["spotify:track:a"]["confidence"], 0.9);
        assert_eq!(doc["spotify:track:a"]["date"], 20_000);
        assert_eq!(doc["spotify:track:a"]["manual"], false);
        // A hand-edited date cannot make the edge refuse the document.
        saved.insert(
            "spotify:track:b".into(),
            SavedMatch {
                source: "https://www.youtube.com/watch?v=abcdefghijk".into(),
                uri: "youtube:video:abcdefghijk".into(),
                at: i64::MAX,
                ..Default::default()
            },
        );
        assert_eq!(
            matches_doc(&saved)["spotify:track:b"]["date"],
            4_000_000_000_000i64
        );
    }

    /// A match taken back from the edge stays usable here: a YouTube answer is
    /// playable as it is, and a SoundCloud answer keeps its public URL without
    /// pretending that a permalink is a numeric track URI.
    #[tokio::test]
    async fn a_match_taken_from_the_edge_is_remembered_in_a_form_that_plays() {
        let dir = crate::test_dirs::temp_dir()
            .join(format!("siphon-edge-take-matches-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let saved = Saved::new(dir.join("matches.json"));
        let data = serde_json::json!({
            "spotify:track:a": {
                "source": "youtube", "sourceUri": "youtube:video:abcdefghijk",
                "confidence": 0.9, "date": "2026-01-01T00:00:00.000Z", "manual": false,
            },
            "spotify:track:b": {
                "source": "soundcloud", "sourceUri": "soundcloud:artist/song",
                "confidence": 0.9, "date": "2026-01-01T00:00:00.000Z", "manual": false,
            },
            "spotify:track:c": {
                "source": "soundcloud", "sourceUri": "soundcloud:artist/song/s-Secret",
                "confidence": 0.9, "date": "2026-01-01T00:00:00.000Z", "manual": false,
            },
            "spotify:track:d": {
                "source": "youtube", "sourceUri": "youtube:video:abcdefghijk",
                "confidence": 1, "date": "2026-01-01T00:00:00.000Z", "manual": true,
            },
        });
        take_matches(Some(&data), &saved).await;
        let youtube = saved.get("spotify:track:a").await.unwrap();
        assert_eq!(youtube.uri, "youtube:video:abcdefghijk");
        assert_eq!(
            youtube.source,
            "https://www.youtube.com/watch?v=abcdefghijk"
        );
        let soundcloud = saved.get("spotify:track:b").await.unwrap();
        assert_eq!(soundcloud.uri, "");
        assert_eq!(soundcloud.source, "https://soundcloud.com/artist/song");
        assert!(saved.get("spotify:track:c").await.is_none());
        // A hand-chosen edge match is never copied over a local one.
        assert!(saved.get("spotify:track:d").await.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A snapshot offered while the edge is down is kept and offered again,
    /// in the body shape the service accepts: `{ data, revision }`.
    #[tokio::test]
    async fn a_snapshot_waits_for_an_unreachable_edge_and_is_sent_again() {
        use crate::edge::test_support::{Server, group_source};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let puts = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&puts);
        let server = Server::new(move |request| {
            if request.path == "/edge/token" {
                return (
                    200,
                    serde_json::json!({
                        "url": "SELF", "live": "live", "media": "media", "sync": "sync"
                    }),
                );
            }
            if request.path == "/v1/sync/playlists" && request.method == "PUT" {
                let attempt = counted.fetch_add(1, Ordering::SeqCst);
                return if attempt == 0 {
                    (500, serde_json::json!({ "v": 1, "error": "internal" }))
                } else {
                    (200, serde_json::json!({ "v": 1, "revision": 1 }))
                };
            }
            (404, serde_json::json!({ "v": 1, "error": "not_found" }))
        });
        let source = group_source(&server).await;
        let dir =
            crate::test_dirs::temp_dir().join(format!("siphon-edge-writer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let writer = Writer::spawn(source, Saved::new(dir.join("matches.json")));
        writer
            .playlists(serde_json::json!({ "playlists": [], "mixed": [] }))
            .await;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while puts.load(Ordering::SeqCst) < 2 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the snapshot was not offered again"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let puts: Vec<_> = server
            .hits()
            .into_iter()
            .filter(|hit| hit.path == "/v1/sync/playlists" && hit.method == "PUT")
            .collect();
        assert_eq!(puts.len(), 2);
        // The second offer is the same picture, now accepted.
        assert_eq!(puts[1].json()["data"]["playlists"], serde_json::json!([]));
        drop(writer);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A snapshot the service can never accept is dropped instead of being
    /// offered every few seconds forever.
    #[tokio::test]
    async fn a_snapshot_the_edge_can_never_accept_is_not_retried() {
        use crate::edge::test_support::{Server, group_source};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let puts = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&puts);
        let server = Server::new(move |request| {
            if request.path == "/edge/token" {
                return (
                    200,
                    serde_json::json!({
                        "url": "SELF", "live": "live", "media": "media", "sync": "sync"
                    }),
                );
            }
            if request.path == "/v1/sync/playlists" && request.method == "PUT" {
                counted.fetch_add(1, Ordering::SeqCst);
                return (
                    413,
                    serde_json::json!({ "v": 1, "error": "user_limit_reached" }),
                );
            }
            (404, serde_json::json!({ "v": 1, "error": "not_found" }))
        });
        let source = group_source(&server).await;
        let dir = crate::test_dirs::temp_dir()
            .join(format!("siphon-edge-too-large-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let writer = Writer::spawn(source, Saved::new(dir.join("matches.json")));
        writer
            .playlists(serde_json::json!({ "playlists": [], "mixed": [] }))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert_eq!(puts.load(Ordering::SeqCst), 1);
        drop(writer);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A phone writing first makes the offer a conflict: the stored revision
    /// is read, and the same snapshot is offered once more with it.
    #[tokio::test]
    async fn a_conflict_reads_the_revision_and_offers_the_snapshot_again() {
        use crate::edge::test_support::{Server, group_source};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let puts = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&puts);
        let server = Server::new(move |request| {
            if request.path == "/edge/token" {
                return (
                    200,
                    serde_json::json!({
                        "url": "SELF", "live": "live", "media": "media", "sync": "sync"
                    }),
                );
            }
            if request.path == "/v1/sync/queue" && request.method == "GET" {
                return (
                    200,
                    serde_json::json!({
                        "v": 1, "doc": "queue", "revision": 4,
                        "updatedAt": "2026-11-10T00:00:00.000Z",
                        "data": { "playing": null, "position_ms": 0, "queue": [] },
                    }),
                );
            }
            if request.path == "/v1/sync/queue" && request.method == "PUT" {
                let attempt = counted.fetch_add(1, Ordering::SeqCst);
                return if attempt == 0 {
                    (
                        409,
                        serde_json::json!({ "v": 1, "error": "revision_conflict" }),
                    )
                } else {
                    (200, serde_json::json!({ "v": 1, "revision": 5 }))
                };
            }
            (404, serde_json::json!({ "v": 1, "error": "not_found" }))
        });
        let source = group_source(&server).await;
        let dir = crate::test_dirs::temp_dir()
            .join(format!("siphon-edge-conflict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let writer = Writer::spawn(source, Saved::new(dir.join("matches.json")));
        writer
            .queue(serde_json::json!({
                "playing": { "uri": "spotify:track:a", "name": "A" },
                "position_ms": 10_000,
                "playing_state": true,
                "queue": [],
            }))
            .await;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while puts.load(Ordering::SeqCst) < 2 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the snapshot was not offered again"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let puts: Vec<_> = server
            .hits()
            .into_iter()
            .filter(|hit| hit.path == "/v1/sync/queue" && hit.method == "PUT")
            .collect();
        assert_eq!(puts.len(), 2);
        assert!(puts[0].json().get("revision").is_none());
        assert_eq!(puts[1].json()["revision"], 4);
        assert_eq!(puts[1].json()["data"]["position_ms"], 10_000);
        drop(writer);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_public_soundcloud_page_is_left_on_the_edge() {
        let public = |link| public_soundcloud_link(Some(link));
        assert_eq!(
            public("https://soundcloud.com/lilgunnr/somuchmore").as_deref(),
            Some("https://soundcloud.com/lilgunnr/somuchmore")
        );
        assert_eq!(public("https://soundcloud.com/artist/song/s-AbCdEf"), None);
        assert_eq!(
            public("https://soundcloud.com/artist/song?secret_token=s-x"),
            None
        );
        assert_eq!(public("https://soundcloud.com/artist"), None);
        assert_eq!(public("https://evil.example/artist/song"), None);
        assert_eq!(public_soundcloud_link(None), None);
    }
}
