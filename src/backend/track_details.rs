//! On-demand credits and source-aware lyrics. No work runs on the UI thread.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::{Event, LyricsRequest, Worker, spotify_lyrics};
use crate::api::gateway::Operation;

#[derive(Default)]
pub(super) struct Jobs {
    credits: HashMap<String, (u64, tokio::task::AbortHandle)>,
    lyrics: Option<tokio::task::AbortHandle>,
    matches: Arc<crate::isrc::Cache>,
}

impl Jobs {
    pub fn cancel_credits(&mut self, uri: &str, request: u64) {
        if self
            .credits
            .get(uri)
            .is_some_and(|(held, _)| *held == request)
            && let Some((_, task)) = self.credits.remove(uri)
        {
            task.abort();
        }
    }

    pub fn cancel_lyrics(&mut self) {
        if let Some(task) = self.lyrics.take() {
            task.abort();
        }
    }

    pub fn clear(&mut self) {
        self.cancel_lyrics();
        for (_, (_, task)) in self.credits.drain() {
            task.abort();
        }
        // A cancelled task can retain its old Arc until its next poll, but it
        // can never fill the next account's cache.
        self.matches = Arc::default();
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        self.clear();
    }
}

impl Worker {
    pub(super) fn fetch_credits(&mut self, uri: String, request: u64) {
        self.track_details
            .credits
            .retain(|_, (_, task)| !task.is_finished());
        if let Some((_, previous)) = self.track_details.credits.remove(&uri) {
            previous.abort();
        }
        let events = self.events.clone();
        let waker = self.waker.clone();
        let engine = self.engine.clone().filter(|engine| {
            super::same_account(&engine.session().username(), self.api.account().as_ref())
        });
        let web = self.api.prerelease_http_client();
        let soundcloud = self.soundcloud_client();
        let mut cancelled = self.session.subscribe();
        let task_uri = uri.clone();
        let background = self.background_session.clone();
        let task = tokio::spawn(async move {
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
                let _permit = background
                    .acquire()
                    .await
                    .map_err(|_| "Credits are unavailable.")?;
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
        self.track_details
            .credits
            .insert(task_uri, (request, task.abort_handle()));
    }

    pub(super) fn fetch_lyrics(&mut self, request: LyricsRequest) {
        self.track_details.cancel_lyrics();
        if !crate::lyrics::supports_uri(&request.uri) {
            let _ = self.events.send(Event::Lyrics {
                request: request.request,
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
        let matches = self.track_details.matches.clone();
        let engine = self.engine.clone().filter(|engine| {
            super::same_account(&engine.session().username(), self.api.account().as_ref())
        });
        let api = self.api.clone();
        let soundcloud = self.soundcloud_client();
        let mut cancelled = self.session.subscribe();
        let task = tokio::spawn(async move {
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
                    crate::isrc::lookup(&web, &matches, &request.uri, code, metadata.duration_ms())
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
                result.map_err(|_| "Lyrics are unavailable. Try again later.".to_owned())
            };
            let result = tokio::select! {
                biased;
                _ = cancelled.changed() => Err("The session ended.".to_string()),
                result = tokio::time::timeout(Duration::from_secs(30), read) => {
                    result.unwrap_or_else(|_| Err("Lyrics took too long to load.".into()))
                },
            };
            let _ = events.send(Event::Lyrics {
                request: request.request,
                uri: request.uri,
                result,
            });
            waker.wake();
        });
        self.track_details.lyrics = Some(task.abort_handle());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_does_not_stop_a_newer_request_for_the_same_track() {
        let mut jobs = Jobs::default();
        let task = tokio::spawn(std::future::pending::<()>());
        jobs.credits
            .insert("soundcloud:track:1".into(), (2, task.abort_handle()));
        jobs.cancel_credits("soundcloud:track:1", 1);
        assert!(jobs.credits.contains_key("soundcloud:track:1"));
        assert!(!task.is_finished());
        jobs.cancel_credits("soundcloud:track:1", 2);
        assert!(task.await.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn account_reset_aborts_reads_and_replaces_the_match_cache() {
        let mut jobs = Jobs::default();
        let cache = jobs.matches.clone();
        let credits = tokio::spawn(std::future::pending::<()>());
        let lyrics = tokio::spawn(std::future::pending::<()>());
        jobs.credits
            .insert("soundcloud:track:1".into(), (1, credits.abort_handle()));
        jobs.lyrics = Some(lyrics.abort_handle());
        jobs.clear();
        assert!(!Arc::ptr_eq(&cache, &jobs.matches));
        assert!(credits.await.unwrap_err().is_cancelled());
        assert!(lyrics.await.unwrap_err().is_cancelled());
    }
}
