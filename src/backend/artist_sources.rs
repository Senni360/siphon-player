//! Reads what SoundCloud and YouTube have of a Spotify artist, for their page. What
//! was found is saved, so a later visit shows it at once and refreshes it quietly.
use super::*;
use crate::artist_sources::{self as sources, Catalogue, Finding, Link, Links, Request, Side};

/// How long what's saved is shown without reading it again.
const FRESH_SECONDS: i64 = 24 * 60 * 60;
const SAVED_VERSION: u32 = 1;
/// How many releases have their songs read, to know what Spotify has.
const MOST_RELEASES: usize = 100;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Saved {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    saved_at: i64,
    #[serde(default)]
    soundcloud: Option<Finding>,
    #[serde(default)]
    youtube: Option<Finding>,
}

/// Everything the reads need, cheap to hand to a task.
#[derive(Clone)]
struct Reader {
    client: crate::soundcloud::api::Client,
    engine: Option<Arc<Engine>>,
    links: Arc<tokio::sync::Mutex<Option<Links>>>,
    links_path: std::path::PathBuf,
    saved_dir: std::path::PathBuf,
    events: std::sync::mpsc::Sender<Event>,
    waker: Waker,
}

impl Reader {
    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        self.waker.wake();
    }

    fn saved_path(&self, id: &str) -> Option<std::path::PathBuf> {
        id.chars()
            .all(|character| character.is_ascii_alphanumeric())
            .then(|| self.saved_dir.join(format!("{id}.json")))
    }

    async fn read_saved(&self, id: &str) -> Option<Saved> {
        let path = self.saved_path(id)?;
        tokio::task::spawn_blocking(move || {
            let bytes = std::fs::read(path).ok()?;
            serde_json::from_slice::<Saved>(&bytes).ok()
        })
        .await
        .ok()
        .flatten()
        .filter(|saved| saved.version == SAVED_VERSION)
    }

    async fn write_saved(&self, id: &str, saved: Saved) {
        let Some(path) = self.saved_path(id) else {
            return;
        };
        let _ = tokio::task::spawn_blocking(move || {
            let bytes = serde_json::to_vec(&saved).ok()?;
            let parent = path.parent()?;
            std::fs::create_dir_all(parent).ok()?;
            if fastframe_file::write_private(&path, &bytes).is_err() {
                log::warn!("Could not save what an artist has elsewhere");
            }
            Some(())
        })
        .await;
    }

    /// The links, read from disk the first time.
    async fn with_links<R>(&self, change: impl FnOnce(&mut Links) -> R) -> (R, Links) {
        let mut links = self.links.lock().await;
        if links.is_none() {
            let path = self.links_path.clone();
            let read = tokio::task::spawn_blocking(move || {
                let bytes = std::fs::read(path).ok()?;
                serde_json::from_slice::<Links>(&bytes).ok()
            })
            .await
            .ok()
            .flatten();
            *links = Some(read.unwrap_or_default());
        }
        let links = links.as_mut().expect("just filled");
        let answer = change(links);
        (answer, links.clone())
    }

    async fn write_links(&self, links: Links) {
        let path = self.links_path.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let bytes = serde_json::to_vec_pretty(&links).ok()?;
            if fastframe_file::write_private(&path, &bytes).is_err() {
                log::warn!("Could not save artist links");
            }
            Some(())
        })
        .await;
    }

    /// The titles Spotify has: what the page knows, and the songs of its releases
    /// over the playback session. A release's own name stands in without a session.
    async fn catalogue(&self, request: &Request) -> Catalogue {
        let mut titles = request.titles.clone();
        titles.extend(request.albums.iter().map(|album| album.name.clone()));
        if let Some(engine) = &self.engine {
            let albums = &request.albums[..request.albums.len().min(MOST_RELEASES)];
            let read = tokio::time::timeout(
                Duration::from_secs(15),
                session_reads::album_tracks(engine.session(), albums),
            );
            match read.await {
                Ok(Ok(tracks)) => titles.extend(tracks.into_iter().map(|track| track.name)),
                _ => log::debug!("an artist's release songs could not be read"),
            }
        }
        Catalogue::new(&request.name, titles)
    }

    async fn read_side(
        &self,
        request: &Request,
        side: Side,
        catalogue: &Catalogue,
        link: Option<Link>,
    ) -> Finding {
        let finding = match side {
            Side::SoundCloud => {
                sources::soundcloud(&self.client, &request.name, catalogue, link.as_ref()).await
            }
            Side::YouTube => {
                let resolver = crate::youtube::Resolver::default();
                sources::youtube(&resolver, &request.name, catalogue, link.as_ref()).await
            }
        };
        self.emit(Event::ArtistSource {
            artist: request.id.clone(),
            ask: request.ask,
            side,
            finding: Box::new(finding.clone()),
        });
        finding
    }

    async fn gather(&self, request: Request, only: Option<Side>) {
        let saved = self.read_saved(&request.id).await;
        let saved = saved.unwrap_or_default();
        let now = jiff::Timestamp::now().as_second();
        if only.is_none() {
            for (side, finding) in [
                (Side::SoundCloud, &saved.soundcloud),
                (Side::YouTube, &saved.youtube),
            ] {
                if let Some(finding) = finding {
                    self.emit(Event::ArtistSource {
                        artist: request.id.clone(),
                        ask: request.ask,
                        side,
                        finding: Box::new(finding.clone()),
                    });
                }
            }
            let complete = saved.soundcloud.is_some() && saved.youtube.is_some();
            if complete && !request.fresh && now - saved.saved_at < FRESH_SECONDS {
                let ((), links) = self.with_links(|_| ()).await;
                self.emit(Event::ArtistLinks(links.back()));
                return;
            }
        }
        let catalogue = self.catalogue(&request).await;
        let ((sc_link, yt_link), _) = self
            .with_links(|links| {
                let linked = links.artists.entry(request.id.clone()).or_default();
                linked.name = request.name.clone();
                (linked.soundcloud.clone(), linked.youtube.clone())
            })
            .await;
        let wanted = |side| only.is_none_or(|only| only == side);
        let (soundcloud, youtube) = tokio::join!(
            async {
                if wanted(Side::SoundCloud) {
                    Some(
                        self.read_side(&request, Side::SoundCloud, &catalogue, sc_link)
                            .await,
                    )
                } else {
                    None
                }
            },
            async {
                if wanted(Side::YouTube) {
                    Some(
                        self.read_side(&request, Side::YouTube, &catalogue, yt_link)
                            .await,
                    )
                } else {
                    None
                }
            }
        );
        let (_, links) = self
            .with_links(|links| {
                let linked = links.artists.entry(request.id.clone()).or_default();
                for (side, finding) in [(Side::SoundCloud, &soundcloud), (Side::YouTube, &youtube)]
                {
                    let Some(finding) = finding else { continue };
                    if finding.error.is_some() {
                        continue;
                    }
                    let slot = linked.side_mut(side);
                    if matches!(slot, Some(Link::Chosen(_) | Link::NotThere)) {
                        continue;
                    }
                    *slot = finding.account.clone().map(Link::Found);
                }
            })
            .await;
        self.write_links(links.clone()).await;
        self.emit(Event::ArtistLinks(links.back()));
        // An answer that failed leaves what was saved in its place.
        let keep = |new: Option<Finding>, old: Option<Finding>| match new {
            Some(new) if new.error.is_none() => Some(new),
            _ => old,
        };
        self.write_saved(
            &request.id,
            Saved {
                version: SAVED_VERSION,
                saved_at: now,
                soundcloud: keep(soundcloud, saved.soundcloud),
                youtube: keep(youtube, saved.youtube),
            },
        )
        .await;
    }
}

impl Worker {
    fn artist_reader(&self) -> Option<Reader> {
        Some(Reader {
            client: self.soundcloud_client()?,
            engine: self.engine.clone(),
            links: Arc::clone(&self.hybrid.artist_links),
            links_path: self.dirs.state.join("artist-links.json"),
            saved_dir: self.dirs.cache.join("artist-sources"),
            events: self.events.clone(),
            waker: self.waker.clone(),
        })
    }

    pub(super) fn artist_sources(&self, request: Request) {
        if let Some(reader) = self.artist_reader() {
            tokio::spawn(async move { reader.gather(request, None).await });
        }
    }

    /// The listener says which account is the artist's, or that none is, or takes
    /// that back with `None`. Read again at once, for that source only.
    pub(super) fn choose_artist_account(&self, request: Request, side: Side, link: Option<Link>) {
        let Some(reader) = self.artist_reader() else {
            return;
        };
        tokio::spawn(async move {
            let (_, links) = reader
                .with_links(|links| {
                    let linked = links.artists.entry(request.id.clone()).or_default();
                    linked.name = request.name.clone();
                    *linked.side_mut(side) = link;
                })
                .await;
            reader.write_links(links).await;
            reader.gather(request, Some(side)).await;
        });
    }

    /// Which artist page each account leads back to.
    pub(super) fn artist_links(&self) {
        let Some(reader) = self.artist_reader() else {
            return;
        };
        tokio::spawn(async move {
            let ((), links) = reader.with_links(|_| ()).await;
            reader.emit(Event::ArtistLinks(links.back()));
        });
    }
}
