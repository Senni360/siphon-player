//! Playlist local files, offered to the phone through the edge.
//!
//! One upload at a time, slowly, on the backend runtime. A file already on the
//! edge is skipped, so a restart resumes where it stopped instead of sending
//! every file again. DRM-protected files and M4P never leave the computer, and
//! a file over the service's cap is left alone rather than half sent.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use super::{Scope, Source, UploadJob};

/// The edge's default per-file cap (`EDGE_UPLOAD_FILE_MAX_BYTES`, 512 MiB).
/// A smaller cap it is configured with is reported as `413` and skipped.
pub const MAX_UPLOAD_BYTES: u64 = 512 * 1024 * 1024;
/// Read in small pieces and pause between them: an upload yields to playback
/// and to everything else on the runtime.
const CHUNK: usize = 128 * 1024;
/// How much of a file is enough to recognise its container.
const HEAD: usize = 64 * 1024;

/// The key a `spotify:local:` URI is uploaded under. Stable across restarts, so
/// the sync record can name the same key before the upload has finished.
pub fn key_for_uri(uri: &str) -> String {
    let digest = Sha256::digest(uri.as_bytes());
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest[..18]);
    format!("local-{token}")
}

/// `M4P` is Apple's protected MP4 audio; an `m4p` extension is enough to
/// refuse it without opening the file.
pub fn m4p(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("m4p"))
}

/// Whether the first bytes name a protected container: an `M4P` brand or the
/// DRM sample-entry boxes (`sinf`, `drms`).
async fn protected(path: &Path) -> bool {
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return true;
    };
    let mut head = vec![0u8; HEAD];
    let mut filled = 0;
    while filled < head.len() {
        match file.read(&mut head[filled..]).await {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(_) => return true,
        }
    }
    head.truncate(filled);
    let text = String::from_utf8_lossy(&head);
    text.contains("M4P ") || text.contains("sinf") || text.contains("drms")
}

pub struct Uploads {
    source: Arc<Source>,
    known: tokio::sync::Mutex<HashSet<String>>,
    enabled: Arc<AtomicBool>,
}

impl Uploads {
    pub fn new(source: Arc<Source>, enabled: Arc<AtomicBool>) -> Self {
        Self {
            source,
            known: tokio::sync::Mutex::new(HashSet::new()),
            enabled,
        }
    }

    pub async fn upload(&self, job: UploadJob) -> Result<(), String> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Err("Local files are not being offered.".into());
        }
        if self.known.lock().await.contains(&job.key) {
            return Ok(());
        }
        let metadata = tokio::fs::metadata(&job.path)
            .await
            .map_err(|_| "That local file is gone.".to_owned())?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err("That local file is empty.".into());
        }
        if m4p(&job.path) || protected(&job.path).await {
            return Err("That file is protected and stays here.".into());
        }
        if metadata.len() > MAX_UPLOAD_BYTES {
            return Err("That file is too large for the edge.".into());
        }

        let (tokens, token) = self.source.token(Scope::Media).await?;
        let client = self.source.http().client()?;
        if self.exists(&client, &tokens.url, &token, &job.key).await {
            self.remember(&job.key).await;
            return Ok(());
        }

        let file = tokio::fs::File::open(&job.path)
            .await
            .map_err(|_| "That local file could not be read.".to_owned())?;
        let body = reqwest::Body::wrap_stream(stream(file, Arc::clone(&self.enabled)));
        let response = client
            .put(format!("{}/v1/upload/{}", tokens.url, job.key))
            .bearer_auth(&token)
            .header(reqwest::header::CONTENT_LENGTH, metadata.len())
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(body)
            .timeout(Duration::from_secs(60 * 30))
            .send()
            .await
            .map_err(|_| "The edge could not be reached.".to_owned())?;
        match response.status().as_u16() {
            201 => {
                self.remember(&job.key).await;
                Ok(())
            }
            401 => Err("The edge refused the upload token.".into()),
            413 => Err("That file is too large for the edge.".into()),
            _ => Err("The edge refused that file.".into()),
        }
    }

    async fn remember(&self, key: &str) {
        self.known.lock().await.insert(key.to_owned());
    }

    /// Whether the edge already holds this key. Accepting the answer spends a
    /// media token, which is cheaper than sending the whole file again.
    async fn exists(&self, client: &reqwest::Client, base: &str, token: &str, key: &str) -> bool {
        let response = client
            .post(format!("{base}/v1/media-token"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "key": key }))
            .timeout(Duration::from_secs(10))
            .send()
            .await;
        response.is_ok_and(|response| response.status().is_success())
    }
}

/// Whether the edge already holds `key`, asked the way an upload asks first.
/// Not knowing counts as no: the upload itself asks once more.
pub(super) async fn on_edge(source: &Source, key: &str) -> bool {
    let Ok((tokens, token)) = source.token(Scope::Media).await else {
        return false;
    };
    let Ok(client) = source.http().client() else {
        return false;
    };
    client
        .post(format!("{}/v1/media-token", tokens.url))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "key": key }))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

/// One file, a piece at a time, with a pause so the network is shared. Turning
/// the setting off ends the stream at the next piece; the edge then refuses the
/// short body and keeps nothing.
fn stream(
    file: tokio::fs::File,
    enabled: Arc<AtomicBool>,
) -> impl futures_util::Stream<Item = Result<Vec<u8>, std::io::Error>> {
    futures_util::stream::unfold(file, move |mut file| {
        let enabled = Arc::clone(&enabled);
        async move {
            if !enabled.load(Ordering::Relaxed) {
                return None;
            }
            let mut buffer = vec![0u8; CHUNK];
            match file.read(&mut buffer).await {
                Ok(0) => {
                    let _ = &mut file;
                    None
                }
                Ok(read) => {
                    buffer.truncate(read);
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    Some((Ok(buffer), file))
                }
                Err(error) => Some((Err(error), file)),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::test_support::{Server, group_source};

    fn file(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn m4a(dir: &Path, name: &str, fill: usize) -> std::path::PathBuf {
        let mut bytes = Vec::with_capacity(12 + fill);
        bytes.extend_from_slice(b"\0\0\0\x18ftypM4A ");
        bytes.resize(12 + fill, b'a');
        file(dir, name, &bytes)
    }

    #[tokio::test]
    async fn a_second_pass_skips_what_the_edge_already_holds() {
        let dir =
            crate::test_dirs::temp_dir().join(format!("siphon-edge-upload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let some = m4a(&dir, "one.m4a", 128);
        let other = m4a(&dir, "two.m4a", 64);
        let server = Server::new(|request| {
            if request.path == "/edge/token" {
                return (
                    200,
                    serde_json::json!({
                        "url": "SELF", "live": "live", "media": "media", "sync": "sync"
                    }),
                );
            }
            if request.path == "/v1/media-token" {
                // Only the first file exists before this run starts.
                let key = request.json()["key"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                return if key == key_for_uri("spotify:local:a") {
                    (200, serde_json::json!({ "url": "/v1/media/x" }))
                } else {
                    (404, serde_json::json!({ "error": "not_found" }))
                };
            }
            if request.path.starts_with("/v1/upload/") {
                return (201, serde_json::json!({ "key": "k" }));
            }
            (404, serde_json::json!({ "error": "not_found" }))
        });
        let source = group_source(&server).await;
        let uploads = Uploads::new(source, Arc::new(AtomicBool::new(true)));
        uploads
            .upload(UploadJob {
                key: key_for_uri("spotify:local:a"),
                path: some.clone(),
                fetched: false,
            })
            .await
            .unwrap();
        uploads
            .upload(UploadJob {
                key: key_for_uri("spotify:local:b"),
                path: other.clone(),
                fetched: false,
            })
            .await
            .unwrap();
        // The first never went: the edge already had it. The second did.
        assert!(
            server
                .hits()
                .iter()
                .all(|hit| !hit.path.ends_with(&key_for_uri("spotify:local:a")))
        );
        assert!(
            server
                .hits()
                .iter()
                .any(|hit| hit.path.ends_with(&key_for_uri("spotify:local:b")))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn protected_files_never_leave_and_the_key_is_stable() {
        let dir = crate::test_dirs::temp_dir()
            .join(format!("siphon-edge-upload-drm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let protected = file(&dir, "song.m4p", b"\0\0\0\x18ftypM4P \0\0\0\0");
        assert!(m4p(&protected));
        assert!(protected_file(&protected).await);
        assert_eq!(
            key_for_uri("spotify:local:A:B:C:180"),
            key_for_uri("spotify:local:A:B:C:180")
        );
        assert_ne!(
            key_for_uri("spotify:local:A:B:C:180"),
            key_for_uri("spotify:local:A:B:D:180")
        );
        assert!(key_for_uri("spotify:local:A:B:C:180").starts_with("local-"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    async fn protected_file(path: &Path) -> bool {
        protected(path).await
    }

    #[test]
    fn the_key_uses_only_characters_the_upload_route_accepts() {
        let key = key_for_uri("spotify:local:Some Artist:An Album:A Song:215");
        assert!(key.len() <= 128);
        assert!(
            key.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        );
    }

    #[test]
    fn the_key_import_is_used() {
        // Keep the `Sha256` and base64 imports honest for the test build too.
        let _ = Sha256::digest(b"x");
        let _ = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"x");
    }
}
