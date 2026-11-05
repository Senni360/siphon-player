//! Reads what SoundCloud and YouTube have of a Spotify artist, for their page. What
//! was found is saved, so a later visit shows it at once and refreshes it quietly.
use super::*;
use crate::artist_sources::{self as sources, Catalogue, Finding, Link, Links, Request, Side};

/// How long what's saved is shown without reading it again.
const FRESH_SECONDS: i64 = 24 * 60 * 60;
const SAVED_VERSION: u32 = 1;
/// How many releases have their songs read, to know what Spotify has.
const MOST_RELEASES: usize = 200;
/// How many further pages of an artist's releases are read, on top of what the page has.
const RELEASE_PAGES: u32 = 6;
const RELEASES_PER_PAGE: u32 = 50;
/// Releases are read this many at a time over the session.
const SESSION_BATCH: usize = 50;

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
    api: Arc<ApiGateway>,
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

    /// Every release of the artist's that can be read, beyond the page or two the
    /// page has loaded. The Web API has a budget, so a failure stops here quietly.
    async fn all_releases(&self, request: &Request, pages: u32) -> (Vec<Album>, Vec<Album>) {
        let mut own = request.albums.clone();
        let mut appears = Vec::new();
        let Ok(client) = self.api.client_for(Operation::Catalog).await else {
            return (own, appears);
        };
        let _permit = client.background_permit().await;
        let read = |groups: &'static str, list: &mut Vec<Album>, pages: u32| {
            let client = Arc::clone(&client);
            let id = request.id.clone();
            let mut found = std::mem::take(list);
            async move {
                for page in 0..pages {
                    let offset = page * RELEASES_PER_PAGE;
                    let Ok(answer) = client
                        .artist_albums(&id, groups, offset, RELEASES_PER_PAGE)
                        .await
                    else {
                        break;
                    };
                    let done = answer.next.is_none();
                    found.extend(answer.items);
                    if done {
                        break;
                    }
                }
                found
            }
        };
        own = read("album,single,compilation", &mut own, pages).await;
        if pages > 1 {
            appears = read("appears_on", &mut appears, 2).await;
        }
        let mut seen = std::collections::HashSet::new();
        own.retain(|album| seen.insert(album.id.clone()));
        (own, appears)
    }

    /// The titles Spotify has: what the page knows, the artist's releases, and their
    /// songs over the playback session. A release's own name stands in without one.
    /// On other people's releases only the songs they're credited on count.
    async fn catalogue(&self, request: &Request, pages: u32) -> Catalogue {
        let mut titles = request.titles.clone();
        let (own, appears) = self.all_releases(request, pages).await;
        titles.extend(own.iter().map(|album| album.name.clone()));
        if let Some(engine) = &self.engine {
            let read = async |albums: &[Album]| {
                let mut tracks = Vec::new();
                for batch in albums[..albums.len().min(MOST_RELEASES)].chunks(SESSION_BATCH) {
                    let answer = tokio::time::timeout(
                        Duration::from_secs(15),
                        session_reads::album_tracks(engine.session(), batch),
                    )
                    .await;
                    match answer {
                        Ok(Ok(found)) => tracks.extend(found),
                        _ => {
                            log::debug!("an artist's release songs could not be read");
                            break;
                        }
                    }
                }
                tracks
            };
            titles.extend(read(&own).await.into_iter().map(|track| track.name));
            let credited = read(&appears).await.into_iter().filter(|track| {
                track
                    .artists
                    .iter()
                    .any(|artist| artist.id.as_deref() == Some(request.id.as_str()))
            });
            titles.extend(credited.map(|track| track.name));
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
        let resolver = crate::youtube::Resolver::default();
        let mut finding = match side {
            Side::SoundCloud => {
                sources::soundcloud(&self.client, &request.name, catalogue, link.as_ref()).await
            }
            Side::YouTube => {
                sources::youtube(&resolver, &request.name, catalogue, link.as_ref()).await
            }
        };
        // The artist's own first, as soon as they're known.
        let show = |finding: &Finding| {
            self.emit(Event::ArtistSource {
                artist: request.id.clone(),
                ask: request.ask,
                side,
                finding: Box::new(finding.clone()),
            });
        };
        show(&finding);
        let beyond = match side {
            Side::SoundCloud => {
                sources::soundcloud_beyond(
                    &self.client,
                    &request.name,
                    catalogue,
                    finding.account.as_ref(),
                )
                .await
            }
            Side::YouTube => sources::youtube_beyond(&resolver, &request.name, catalogue).await,
        };
        finding.add(beyond);
        show(&finding);
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
        let catalogue = self.catalogue(&request, RELEASE_PAGES).await;
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
                    if side == Side::SoundCloud {
                        linked.alternates = finding.alternates.clone();
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
            api: Arc::clone(&self.api),
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

    /// Someone opened an account's page with no artist known for it. If a Spotify
    /// artist of that name finds this very account by the usual rules, it leads back
    /// from now on; if not, the page stays as it is rather than guess.
    pub(super) fn find_artist_for(&self, uri: String, name: String) {
        let Some(reader) = self.artist_reader() else {
            return;
        };
        tokio::spawn(async move {
            let side = if uri.starts_with("youtube:") {
                Side::YouTube
            } else {
                Side::SoundCloud
            };
            let Ok(client) = reader.api.client_for(Operation::CatalogSearch).await else {
                return;
            };
            let Ok(found) = client.search(&name, &["artist"]).await else {
                return;
            };
            let artists = found.artists.map(|page| page.items).unwrap_or_default();
            let same = artists
                .into_iter()
                .filter(|artist| sources::name_match(&artist.name, &name) >= 1.0)
                .take(2);
            for artist in same {
                let request = Request {
                    id: artist.id.clone(),
                    name: artist.name.clone(),
                    ask: 0,
                    titles: Vec::new(),
                    albums: Vec::new(),
                    fresh: false,
                };
                // Enough to recognise an account: opening profiles shouldn't spend
                // the listener's Spotify budget reading whole catalogues.
                let catalogue = reader.catalogue(&request, 1).await;
                let finding = match side {
                    Side::SoundCloud => {
                        sources::soundcloud(&reader.client, &request.name, &catalogue, None).await
                    }
                    Side::YouTube => {
                        let resolver = crate::youtube::Resolver::default();
                        sources::youtube(&resolver, &request.name, &catalogue, None).await
                    }
                };
                if finding
                    .account
                    .as_ref()
                    .is_none_or(|account| account.uri != uri)
                {
                    continue;
                }
                let (_, links) = reader
                    .with_links(|links| {
                        let linked = links.artists.entry(artist.id.clone()).or_default();
                        linked.name = artist.name.clone();
                        let slot = linked.side_mut(side);
                        if slot.is_none() {
                            *slot = finding.account.clone().map(Link::Found);
                        }
                    })
                    .await;
                reader.write_links(links.clone()).await;
                reader.emit(Event::ArtistLinks(links.back()));
                return;
            }
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
