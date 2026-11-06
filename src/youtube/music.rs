//! Whether a video is music. YouTube files every upload under a category its
//! uploader chose, and Home recommends only those filed under Music, so a channel
//! followed for football or the news never fills a music app's Home. One small
//! read of YouTube's player endpoint per video, without a sign-in, remembered for
//! the rest of the run. Runs on the backend runtime.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use crate::api::models::Track;

const ENDPOINT: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";
const CLIENT_VERSION: &str = "2.20251101.00.00";
const MOST_AT_ONCE: usize = 6;
const ANSWER_BYTES: usize = 512 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

/// What each video was found to be, by id. Only clear answers are kept, so a
/// failed read is asked again next time.
static KNOWN: LazyLock<Mutex<HashMap<String, bool>>> = LazyLock::new(Mutex::default);

/// The category in a player answer, such as `Music` or `Sports`.
fn category(body: &serde_json::Value) -> Option<&str> {
    body.pointer("/microformat/playerMicroformatRenderer/category")?
        .as_str()
}

fn known(id: &str) -> Option<bool> {
    KNOWN
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(id)
        .copied()
}

async fn ask(http: &reqwest::Client, id: &str) -> Option<bool> {
    if let Some(answer) = known(id) {
        return Some(answer);
    }
    let body = serde_json::json!({
        "context": {"client": {"clientName": "WEB", "clientVersion": CLIENT_VERSION, "hl": "en"}},
        "videoId": id,
    });
    let read = async {
        let response = http
            .post(ENDPOINT)
            .json(&body)
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        let bytes = response.bytes().await.ok()?;
        (bytes.len() <= ANSWER_BYTES)
            .then(|| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .flatten()
    };
    let answer = tokio::time::timeout(TIMEOUT, read).await.ok()??;
    let music = category(&answer)? == "Music";
    KNOWN
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(id.to_owned(), music);
    Some(music)
}

/// The songs given, without the YouTube videos that aren't music, in the same
/// order. A video YouTube didn't say anything about is left out too: Home would
/// rather miss a song than show the news. Songs of other sources all stay.
pub async fn keep_music(http: &reqwest::Client, tracks: Vec<Track>) -> Vec<Track> {
    let ids: Vec<String> = tracks
        .iter()
        .filter_map(|track| crate::youtube::video_id(&track.uri))
        .map(str::to_owned)
        .collect();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(MOST_AT_ONCE));
    let mut reads = tokio::task::JoinSet::new();
    for id in ids {
        if known(&id).is_some() {
            continue;
        }
        let http = http.clone();
        let semaphore = std::sync::Arc::clone(&semaphore);
        reads.spawn(async move {
            let _permit = semaphore.acquire_owned().await.ok()?;
            ask(&http, &id).await
        });
    }
    while reads.join_next().await.is_some() {}
    tracks
        .into_iter()
        .filter(|track| match crate::youtube::video_id(&track.uri) {
            Some(id) => known(id) == Some(true),
            None => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_category_comes_from_the_player_answer() {
        let answer = serde_json::json!({
            "playabilityStatus": {"status": "UNPLAYABLE"},
            "microformat": {"playerMicroformatRenderer": {"category": "Sports"}}
        });
        assert_eq!(category(&answer), Some("Sports"));
        assert_eq!(category(&serde_json::json!({})), None);
    }

    #[tokio::test]
    async fn other_sources_stay_and_unanswered_videos_go() {
        KNOWN.lock().unwrap().extend([
            ("aaaaaaaaaaa".to_owned(), true),
            ("bbbbbbbbbbb".to_owned(), false),
        ]);
        let song = |uri: &str| Track {
            uri: uri.into(),
            ..Default::default()
        };
        // Nothing is asked: every video is known already.
        let kept = keep_music(
            &reqwest::Client::new(),
            vec![
                song("youtube:video:bbbbbbbbbbb"),
                song("soundcloud:track:1"),
                song("youtube:video:aaaaaaaaaaa"),
            ],
        )
        .await;
        let uris: Vec<&str> = kept.iter().map(|track| track.uri.as_str()).collect();
        assert_eq!(uris, ["soundcloud:track:1", "youtube:video:aaaaaaaaaaa"]);
    }

    /// Asks YouTube about a song and a video that isn't one:
    /// `cargo test --lib youtube_music_works_for_real -- --ignored`
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn youtube_music_works_for_real() {
        let http = reqwest::Client::new();
        assert_eq!(ask(&http, "dQw4w9WgXcQ").await, Some(true));
        assert_eq!(ask(&http, "jNQXAC9IVRw").await, Some(false));
    }
}
