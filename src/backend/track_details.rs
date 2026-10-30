//! On-demand credits and source-aware lyrics. No work runs on the UI thread.

use std::time::Duration;

use super::{Event, LyricsRequest, Worker, spotify_lyrics};
use crate::api::gateway::Operation;

impl Worker {
    pub(super) fn fetch_credits(&self, uri: String, request: u64) {
        let events = self.events.clone();
        let waker = self.waker.clone();
        let engine = self.engine.clone();
        let web = self.api.prerelease_http_client();
        let soundcloud = self.soundcloud_client();
        let mut cancelled = self.session.subscribe();
        tokio::spawn(async move {
            let read = async {
                if let Some(id) = crate::soundcloud::track_id(&uri) {
                    let client = soundcloud.ok_or("SoundCloud is unavailable.")?;
                    let metadata = client
                        .credits_metadata(id)
                        .await
                        .map_err(|_| "SoundCloud credits are unavailable.")?;
                    if metadata.id != id {
                        return Err("SoundCloud returned a different track.");
                    }
                    return Ok(metadata.credits());
                }
                if !uri.starts_with("spotify:track:") {
                    // YouTube and other unsupported sources have no credits.
                    return Ok(None);
                }
                let engine = engine.ok_or("Credits need Spotify playback enabled.")?;
                let web = web.ok_or("Credits need a signed-in Spotify account.")?;
                crate::session_reads::track_credits(engine.session(), &web, &uri)
                    .await
                    .map_err(|_| "Spotify credits are unavailable. Try again later.")
            };
            let result = tokio::select! {
                biased;
                _ = cancelled.changed() => Err("The session ended.".into()),
                result = tokio::time::timeout(Duration::from_secs(15), read) => {
                    match result {
                        Ok(result) => result.map_err(str::to_owned),
                        Err(_) => Err("Credits took too long to load.".into()),
                    }
                }
            };
            let _ = events.send(Event::Credits {
                uri,
                request,
                result,
            });
            waker.wake();
        });
    }

    pub(super) fn fetch_lyrics(&self, request: LyricsRequest) {
        if !crate::lyrics::supports_uri(&request.uri) {
            let _ = self.events.send(Event::Lyrics {
                uri: request.uri,
                result: Ok(None),
            });
            self.waker.wake();
            return;
        }
        let http = self.http.client();
        let events = self.events.clone();
        let waker = self.waker.clone();
        let cache_dir = self.dirs.lyrics_cache_dir();
        let match_dir = self.dirs.cache.join("isrc-matches-v1");
        let engine = self.engine.clone();
        let api = self.api.clone();
        let soundcloud = self.soundcloud_client();
        let mut cancelled = self.session.subscribe();
        tokio::spawn(async move {
            let read = async {
                let strict = !request.uri.starts_with("spotify:");
                let own = if request.uri.starts_with("spotify:local:") {
                    match request.local_file {
                        Some(file) => {
                            tokio::task::spawn_blocking(move || crate::local_tags::lyrics(&file))
                                .await
                                .ok()
                                .flatten()
                        }
                        None => None,
                    }
                } else {
                    None
                };
                let mapping = async {
                    let id = crate::soundcloud::track_id(&request.uri)?;
                    let metadata = soundcloud?.credits_metadata(id).await.ok()?;
                    if metadata.id != id {
                        return None;
                    }
                    let code = metadata.publisher_metadata.as_ref()?.isrc.as_deref()?;
                    let web = api.client_for(Operation::CatalogSearch).await.ok()?;
                    crate::isrc::lookup(
                        &web,
                        &match_dir,
                        &request.uri,
                        code,
                        metadata.duration_ms(),
                    )
                    .await
                    .ok()
                    .flatten()
                };
                // An unavailable mapping must not hold the strict public lookup indefinitely.
                let matched = tokio::time::timeout(Duration::from_secs(10), mapping)
                    .await
                    .ok()
                    .flatten();
                let spotify_uri = crate::lyrics::spotify_uri(
                    &request.uri,
                    matched.as_ref().map(|m| m.spotify_uri.as_str()),
                );
                let exact = match (own, spotify_uri) {
                    (Some(found), _) => Some(found),
                    (None, Some(uri)) => spotify_lyrics(engine, uri, &cache_dir).await,
                    (None, None) => None,
                };
                if let Some(found) = exact {
                    return Ok(Some(found));
                }
                let http = http?;
                let result = if strict {
                    crate::lyrics::fetch_strict(&http, &cache_dir, &request.query).await
                } else {
                    crate::lyrics::fetch(&http, &cache_dir, &request.query).await
                };
                result.map_err(|error| format!("{error:#}"))
            };
            let result = tokio::select! {
                biased;
                _ = cancelled.changed() => Err("The session ended.".to_string()),
                result = read => result,
            };
            let _ = events.send(Event::Lyrics {
                uri: request.uri,
                result,
            });
            waker.wake();
        });
    }
}
