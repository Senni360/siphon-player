//! YouTube audio the edge cannot produce itself. The edge asks for one source,
//! or the phone asks while a computer is in the room; the existing yt-dlp path
//! finds the M4A URL, this downloads it, and the uploader offers it to the edge.
//! No cookies are used and no source outside YouTube is accepted.

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
        let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "The download stopped early.".to_owned())?
        else {
            break;
        };
        written += chunk.len() as u64;
        if written > MAX_UPLOAD_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return Err("That song is too large for the edge.".into());
        }
        file.write_all(&chunk)
            .await
            .map_err(|_| "The fetched audio could not be saved.".to_owned())?;
    }
    if written == 0 {
        drop(file);
        let _ = tokio::fs::remove_file(&path).await;
        return Err("YouTube offered an empty file.".into());
    }
    file.flush()
        .await
        .map_err(|_| "The fetched audio could not be saved.".to_owned())?;
    Ok((key, path))
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
}
