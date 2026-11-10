//! A YouTube song's audio for the edge, which cannot produce it itself. When
//! the phone plays one, the existing yt-dlp path finds the M4A URL, this
//! downloads it, and the uploader offers it under `yt-<id>` so a later play
//! finds it there. No cookies are used and no source outside YouTube is
//! accepted.

use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;

use super::upload::MAX_UPLOAD_BYTES;

/// The upload key for a YouTube video, stable so a second ask skips it.
pub fn key_for(source: &str) -> Option<String> {
    let id = source.strip_prefix("youtube:video:")?;
    (id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'))
    .then(|| format!("yt-{id}"))
}

/// Downloads `source` as M4A into `dir`. The return is the upload key and the
/// file. A download over the service's cap, a missing tool or a refused source
/// leaves nothing to upload.
pub async fn download(
    http: &crate::http::Http,
    source: &str,
    dir: &Path,
) -> Result<(String, PathBuf), String> {
    let key = key_for(source).ok_or_else(|| "Only a YouTube video can be fetched.".to_owned())?;
    let id = key
        .strip_prefix("yt-")
        .ok_or_else(|| "Only a YouTube video can be fetched.".to_owned())?;
    let resolver = crate::youtube::Resolver::default();
    let url = resolver
        .audio_url(id)
        .await
        .map_err(|_| "YouTube did not answer.".to_owned())?;
    let client = http.client()?;
    let response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(60 * 5))
        .send()
        .await
        .map_err(|_| "The audio could not be downloaded.".to_owned())?;
    if !response.status().is_success() {
        return Err("YouTube did not offer the audio.".into());
    }
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|_| "The fetch folder could not be made.".to_owned())?;
    let path = dir.join(format!("{key}.m4a"));
    let mut file = tokio::fs::File::create(&path)
        .await
        .map_err(|_| "The fetched audio could not be saved.".to_owned())?;
    let mut response = response;
    let mut written = 0u64;
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => {
                drop(file);
                let _ = tokio::fs::remove_file(&path).await;
                return Err("The download stopped early.".into());
            }
        };
        written += chunk.len() as u64;
        if written > MAX_UPLOAD_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return Err("That song is too large for the edge.".into());
        }
        if file.write_all(&chunk).await.is_err() {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return Err("The fetched audio could not be saved.".into());
        }
    }
    if written == 0 {
        drop(file);
        let _ = tokio::fs::remove_file(&path).await;
        return Err("YouTube offered an empty file.".into());
    }
    if file.flush().await.is_err() {
        drop(file);
        let _ = tokio::fs::remove_file(&path).await;
        return Err("The fetched audio could not be saved.".into());
    }
    Ok((key, path))
}

/// Removes fetched files a crash or a stop left behind. They live in the cache
/// folder, but a half-written song should not wait for the next cache cleanup.
pub async fn sweep(dir: &Path, older_than: std::time::Duration) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    let now = std::time::SystemTime::now();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let old = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > older_than);
        if old {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_youtube_video_is_fetched() {
        assert_eq!(
            key_for("youtube:video:aB9_-012xyz").as_deref(),
            Some("yt-aB9_-012xyz")
        );
        assert!(key_for("youtube:video:short").is_none());
        assert!(key_for("spotify:track:a").is_none());
        assert!(key_for("https://www.youtube.com/watch?v=aB9_-012xyz").is_none());
        assert!(key_for("youtube:video:aB9_-012xy?").is_none());
    }

    /// A fetch cut off by a crash leaves a large file; a fresh one stays for
    /// the upload that may still be waiting.
    #[tokio::test]
    async fn a_stale_fetch_is_swept_and_a_fresh_one_is_kept() {
        let dir =
            crate::test_dirs::temp_dir().join(format!("siphon-edge-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("yt-old.m4a");
        let fresh = dir.join("yt-fresh.m4a");
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(&fresh, b"fresh").unwrap();
        let two_days = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 24 * 3600);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(two_days)
            .unwrap();
        sweep(&dir, std::time::Duration::from_secs(24 * 3600)).await;
        assert!(!old.exists());
        assert!(fresh.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
