//! Runtime ownership of helper processes and Spotify collection reads.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use super::{ApiRequest, ApiResponse, Command, Event, Worker};
use crate::api::models::PlayableItem;
use crate::downloads::{self, Request, Source, Summary, Target};

#[derive(Default)]
pub(super) struct Jobs {
    cancels: BTreeMap<u64, watch::Sender<bool>>,
}

impl Jobs {
    pub fn cancel(&self, id: u64) {
        if let Some(cancel) = self.cancels.get(&id) {
            let _ = cancel.send(true);
        }
    }
    pub fn cancel_all(&self) {
        for cancel in self.cancels.values() {
            let _ = cancel.send(true);
        }
    }
    pub fn finish(&mut self, id: u64) {
        self.cancels.remove(&id);
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

impl Worker {
    pub(super) fn discover_download_helper(&self) {
        // Ordinary app fixtures must not inspect the machine's real helper
        // registration or receive nondeterministic availability events.
        #[cfg(not(test))]
        {
            let events = self.events.clone();
            let waker = self.waker.clone();
            tokio::spawn(async move {
                let available = downloads::available().await;
                let _ = events.send(Event::Download(downloads::Event::Available(available)));
                waker.wake();
            });
        }
    }

    pub(super) fn start_download(&mut self, request: Request) {
        let id = request.id;
        if self.download_jobs.cancels.len() >= downloads::MAX_BATCHES {
            self.emit(Event::Download(downloads::Event::Finished {
                id,
                result: Err("Wait for a download to finish before starting another.".into()),
            }));
            return;
        }
        let (cancel, mut cancelled) = watch::channel(false);
        self.download_jobs.cancels.insert(id, cancel);
        let reader = CollectionReader {
            api: self.api.clone(),
            made_for_you: self.made_for_you.clone(),
            new_releases: self.new_releases.clone(),
            connect: self.connect.clone(),
            engine: self.engine.clone(),
            background: self.background_session.clone(),
            soundcloud: self.soundcloud_client(),
        };
        let events = self.events.clone();
        let commands = self.commands.clone();
        let waker = self.waker.clone();
        let mut session = self.session.subscribe();
        drop(session.borrow_and_update());
        tokio::spawn(async move {
            let sources = tokio::select! {
                biased;
                _ = cancelled.changed() => None,
                _ = session.changed() => None,
                result = reader.sources(request.target) => Some(result),
            };
            drop(reader);
            drop(session);
            let result = match sources {
                None => Ok(Summary {
                    cancelled: true,
                    ..Default::default()
                }),
                Some(Err(error)) => Err(error),
                Some(Ok(sources)) => {
                    downloads::run(id, sources, cancelled, |event| {
                        let _ = events.send(Event::Download(event));
                        waker.wake();
                    })
                    .await
                }
            };
            let _ = commands.send(Command::DownloadFinished { id, result });
        });
    }
}

struct CollectionReader {
    api: Arc<crate::api::ApiGateway>,
    made_for_you: Arc<crate::made_for_you::MadeForYou>,
    new_releases: Arc<crate::new_releases::NewReleases>,
    connect: Arc<crate::sonos::Controller>,
    engine: Option<Arc<crate::player::Engine>>,
    background: Arc<tokio::sync::Semaphore>,
    soundcloud: Option<crate::soundcloud::api::Client>,
}

impl CollectionReader {
    async fn sources(&self, target: Target) -> Result<Vec<Source>, String> {
        let sources = match target {
            Target::Items(items) => items,
            Target::SpotifyContext(uri) => self.collection(&uri).await?,
            Target::SoundCloudSet { uri, title } => {
                let id = crate::soundcloud::playlist_id(&uri)
                    .ok_or("Choose a SoundCloud set to download.")?;
                let client = self
                    .soundcloud
                    .as_ref()
                    .ok_or("SoundCloud is not ready. Try again shortly.")?;
                let playlist = tokio::time::timeout(Duration::from_secs(60), client.playlist(id))
                    .await
                    .map_err(|_| "SoundCloud took too long to load the set.".to_string())?
                    .map_err(|error| error.to_string())?;
                let url = crate::soundcloud::permalink(playlist.permalink_url.as_deref())
                    .ok_or("SoundCloud did not provide a public link for this set.")?;
                vec![Source::SoundCloud {
                    url: url.into(),
                    title,
                    all: true,
                }]
            }
        };
        if sources.is_empty() {
            return Err("This collection has no downloadable tracks.".into());
        }
        Ok(sources)
    }

    async fn collection(&self, uri: &str) -> Result<Vec<Source>, String> {
        let mut parts = uri.split(':');
        let (Some("spotify"), Some(kind @ ("album" | "playlist")), Some(id), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err("Choose a Spotify album or playlist to download.".into());
        };
        let mut sources = Vec::new();
        let mut seen = HashSet::new();
        let mut offset = 0;
        loop {
            let request = if kind == "album" {
                ApiRequest::AlbumTracks {
                    id: id.into(),
                    offset,
                    generation: 0,
                }
            } else {
                ApiRequest::PlaylistItems {
                    id: id.into(),
                    offset,
                    generation: 0,
                }
            };
            // Use the same-account session/Web API router and pagination as
            // the collection page. Never turn a partial page into Download all.
            let (response, _) = tokio::time::timeout(
                Duration::from_secs(120),
                super::handle(
                    &self.api,
                    &self.made_for_you,
                    &self.new_releases,
                    &self.connect,
                    self.engine.as_deref(),
                    &self.background,
                    request,
                ),
            )
            .await
            .map_err(|_| "Spotify took too long to load the collection.".to_string())?;
            let next = match response {
                ApiResponse::AlbumTracks { result, .. } => {
                    let page = result.map_err(|error| error.to_string())?;
                    let next = page.next_offset();
                    for track in page.items {
                        push_source(&mut sources, &mut seen, PlayableItem::Track(track));
                    }
                    next
                }
                ApiResponse::PlaylistItems { result, .. } => {
                    let page = result.map_err(|error| error.to_string())?;
                    let next = page.next_offset();
                    for item in page
                        .items
                        .into_iter()
                        .filter_map(|item| item.playable().cloned())
                    {
                        push_source(&mut sources, &mut seen, item);
                    }
                    next
                }
                _ => return Err("Could not read the Spotify collection.".into()),
            };
            match next {
                None => break,
                Some(next) if next > offset && next <= 10_000 => offset = next,
                Some(_) => {
                    return Err("Spotify returned incomplete collection pages. Try again.".into());
                }
            }
        }
        Ok(sources)
    }
}

fn push_source(sources: &mut Vec<Source>, seen: &mut HashSet<String>, item: PlayableItem) {
    if let Some(source) = Source::from_item(&item)
        && seen.insert(item.uri().into())
    {
        sources.push(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{Episode, Track};

    #[test]
    fn collection_sources_preserve_order_deduplicate_and_skip_local_files_and_episodes() {
        let mut sources = Vec::new();
        let mut seen = HashSet::new();
        for uri in [
            "spotify:track:first",
            "spotify:local:file",
            "spotify:track:second",
            "spotify:track:first",
        ] {
            push_source(
                &mut sources,
                &mut seen,
                PlayableItem::Track(Track {
                    uri: uri.into(),
                    name: uri.into(),
                    ..Default::default()
                }),
            );
        }
        push_source(
            &mut sources,
            &mut seen,
            PlayableItem::Episode(Episode::default()),
        );
        assert_eq!(
            sources.iter().map(Source::title).collect::<Vec<_>>(),
            ["spotify:track:first", "spotify:track:second"]
        );
    }
}
