//! Spotify songs already matched to a source, so a later play or download
//! searches no more. A small JSON file under the state folder; a missing,
//! corrupt, older or newer file simply means nothing is remembered yet.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::matching;

/// This file's shape. Older copies read a file without it as version 0.
const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedMatch {
    /// The public URL the Siphon helper downloads from.
    #[serde(default)]
    pub source: String,
    /// The URI the player uses for the same song.
    #[serde(default)]
    pub uri: String,
    /// Unix seconds, when the match was found.
    #[serde(default)]
    pub at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    matches: BTreeMap<String, SavedMatch>,
}

struct Inner {
    path: PathBuf,
    matches: Mutex<BTreeMap<String, SavedMatch>>,
    loaded: tokio::sync::OnceCell<()>,
    writing: tokio::sync::Mutex<()>,
}

/// The saved matches of this profile. Clones share one file and one map.
#[derive(Clone, Default)]
pub(crate) struct Saved {
    inner: Option<Arc<Inner>>,
}

impl Saved {
    pub fn new(path: PathBuf) -> Self {
        Self {
            inner: Some(Arc::new(Inner {
                path,
                matches: Mutex::new(BTreeMap::new()),
                loaded: tokio::sync::OnceCell::new(),
                writing: tokio::sync::Mutex::new(()),
            })),
        }
    }

    /// Reads the file once, on the runtime. A failed read leaves an empty set
    /// that the next match fills in again.
    async fn ready(&self) {
        let Some(inner) = &self.inner else { return };
        inner
            .loaded
            .get_or_init(|| async {
                let Ok(bytes) = tokio::fs::read(&inner.path).await else {
                    return;
                };
                let Ok(file) = serde_json::from_slice::<File>(&bytes) else {
                    return;
                };
                // Do not guess at a newer shape; keep it untouched.
                if file.version > VERSION {
                    return;
                }
                if let Ok(mut held) = inner.matches.lock() {
                    *held = file.matches;
                }
            })
            .await;
    }

    pub async fn get(&self, spotify: &str) -> Option<SavedMatch> {
        self.ready().await;
        let inner = self.inner.as_ref()?;
        inner
            .matches
            .lock()
            .ok()?
            .get(spotify)
            .cloned()
            .filter(|found| !found.source.trim().is_empty() || !found.uri.trim().is_empty())
    }

    /// The public URL the helper downloads. Older files that only kept a
    /// player URI still work when it names a YouTube video.
    pub async fn source(&self, spotify: &str) -> Option<String> {
        let found = self.get(spotify).await?;
        if !found.source.trim().is_empty() {
            return Some(found.source);
        }
        if crate::youtube::is_track_uri(&found.uri) {
            return crate::youtube::permalink(&found.uri);
        }
        found.uri.starts_with("https://").then_some(found.uri)
    }

    pub async fn remember(&self, spotify: &str, found: &matching::Match) {
        if !spotify.starts_with("spotify:track:") {
            return;
        }
        self.ready().await;
        let Some(inner) = &self.inner else { return };
        if let Ok(mut held) = inner.matches.lock() {
            held.insert(
                spotify.to_owned(),
                SavedMatch {
                    source: found.source.clone(),
                    uri: found.uri.clone(),
                    at: now(),
                },
            );
        }
        self.persist().await;
    }

    /// Every saved match, for the always-on edge's match document.
    pub async fn all(&self) -> BTreeMap<String, SavedMatch> {
        self.ready().await;
        self.inner
            .as_ref()
            .and_then(|inner| inner.matches.lock().ok().map(|held| held.clone()))
            .unwrap_or_default()
    }

    pub async fn forget(&self, spotify: &str) {
        self.ready().await;
        let Some(inner) = &self.inner else { return };
        let removed = inner
            .matches
            .lock()
            .map(|mut held| held.remove(spotify).is_some())
            .unwrap_or(false);
        if removed {
            self.persist().await;
        }
    }

    /// Replaces the whole file, so a failed write leaves the previous one
    /// readable. Writes are serialized, so two songs finishing together do
    /// not overwrite each other with a stale snapshot.
    async fn persist(&self) {
        let Some(inner) = &self.inner else { return };
        let _writing = inner.writing.lock().await;
        let file = {
            let Ok(held) = inner.matches.lock() else {
                return;
            };
            File {
                version: VERSION,
                matches: held.clone(),
            }
        };
        let Ok(bytes) = serde_json::to_vec(&file) else {
            return;
        };
        let path = inner.path.clone();
        let _ = tokio::task::spawn_blocking(move || fastframe_file::write(&path, bytes)).await;
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        crate::test_dirs::temp_dir().join(format!(
            "siphon-saved-matches-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    fn found() -> matching::Match {
        matching::Match {
            uri: "youtube:video:abcdefghijk".into(),
            source: "https://www.youtube.com/watch?v=abcdefghijk".into(),
        }
    }

    #[tokio::test]
    async fn a_match_round_trips_with_its_date_and_survives_a_restart() {
        let root = root("round-trip");
        let path = root.join("spotify-matches.json");
        let saved = Saved::new(path.clone());
        assert!(saved.get("spotify:track:one").await.is_none());
        saved.remember("spotify:track:one", &found()).await;
        saved.forget("spotify:track:one").await;
        assert!(saved.get("spotify:track:one").await.is_none());
        saved.remember("spotify:track:one", &found()).await;
        let stored = saved.get("spotify:track:one").await.unwrap();
        assert_eq!(stored.source, found().source);
        assert_eq!(stored.uri, found().uri);
        assert!(stored.at > 0);
        // A new copy reads the file rather than searching again.
        let reopened = Saved::new(path.clone());
        assert_eq!(
            reopened.get("spotify:track:one").await.unwrap().source,
            found().source
        );
        assert_eq!(
            reopened.source("spotify:track:one").await.as_deref(),
            Some(found().source.as_str())
        );
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn older_corrupt_and_newer_files_still_load_what_is_usable() {
        let root = root("older");
        std::fs::create_dir_all(&root).unwrap();
        // The first files had no version and only the player URI.
        let older = root.join("older.json");
        std::fs::write(
            &older,
            br#"{"matches":{"spotify:track:a":{"uri":"youtube:video:abcdefghijk"}}}"#,
        )
        .unwrap();
        let saved = Saved::new(older.clone());
        let stored = saved.get("spotify:track:a").await.unwrap();
        assert_eq!(stored.uri, "youtube:video:abcdefghijk");
        assert_eq!(stored.at, 0);
        assert_eq!(
            saved.source("spotify:track:a").await.as_deref(),
            Some("https://www.youtube.com/watch?v=abcdefghijk")
        );
        // Unknown fields and a partial entry are ignored, not fatal.
        let partial = root.join("partial.json");
        std::fs::write(
            &partial,
            br#"{"version":0,"matches":{"spotify:track:b":{"source":"https://soundcloud.com/a/b","extra":true}},"future":{}}"#,
        )
        .unwrap();
        let saved = Saved::new(partial);
        assert_eq!(
            saved.source("spotify:track:b").await.as_deref(),
            Some("https://soundcloud.com/a/b")
        );
        // Corrupt and newer files read as empty and are left untouched.
        for (name, body) in [
            ("corrupt.json", &br#"{"matches":{"spotify:track:c":"#[..]),
            (
                "newer.json",
                &br#"{"version":99,"matches":{"spotify:track:d":{"source":"https://example.com/d"}}}"#[..],
            ),
        ] {
            let path = root.join(name);
            std::fs::write(&path, body).unwrap();
            let saved = Saved::new(path.clone());
            assert!(saved.get("spotify:track:d").await.is_none());
            assert_eq!(std::fs::read(&path).unwrap(), body);
            if name == "corrupt.json" {
                // A corrupt file is replaced by the next saved match.
                saved.remember("spotify:track:e", &found()).await;
                assert_eq!(
                    Saved::new(path).get("spotify:track:e").await.unwrap().uri,
                    found().uri
                );
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn simultaneous_matches_all_reach_the_file() {
        let root = root("together");
        let path = root.join("spotify-matches.json");
        let saved = Saved::new(path.clone());
        let tasks: Vec<_> = (0..8)
            .map(|index| {
                let saved = saved.clone();
                tokio::spawn(async move {
                    saved
                        .remember(
                            &format!("spotify:track:{index}"),
                            &matching::Match {
                                uri: format!("youtube:video:abcdefghij{index}"),
                                source: "https://www.youtube.com/watch?v=abcdefghijk".into(),
                            },
                        )
                        .await;
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        let reopened = Saved::new(path);
        for index in 0..8 {
            assert!(
                reopened
                    .get(&format!("spotify:track:{index}"))
                    .await
                    .is_some(),
                "{index}"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
