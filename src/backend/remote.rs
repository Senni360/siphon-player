//! The connection a linked phone reaches this computer through. It only runs for
//! someone who has linked one, and only looks often while the phone is watching.
use super::*;
use crate::remote::{IDLE_POLL, Incoming, WATCHED_POLL};

/// What the app asks of the phone's connection.
#[derive(Debug)]
pub enum RemoteRequest {
    /// Start looking for commands, if nothing is already.
    Start,
    State(serde_json::Value),
    Reply {
        id: String,
        value: serde_json::Value,
    },
    Search {
        id: String,
        query: String,
    },
    /// A code for a phone to type, and this listener's own Spotify app for it to use.
    LinkCode {
        client_id: Option<String>,
    },
}

#[derive(Debug)]
pub enum RemoteEvent {
    Commands(Vec<Incoming>),
    Watched(bool),
    Code(Result<String, String>),
}

fn songs_json(tracks: &[Track]) -> serde_json::Value {
    serde_json::to_value(crate::remote::songs(tracks)).unwrap_or_default()
}

impl Worker {
    fn remote_client(&self) -> Result<crate::friends::Client, String> {
        let http = crate::http::build_collection_client(&self.engine_config.proxy)
            .map(Http::new)
            .unwrap_or_else(Http::unavailable);
        crate::friends::Client::configured(http)
    }

    pub(super) fn remote(&mut self, request: RemoteRequest) {
        let Ok(client) = self.remote_client() else {
            return;
        };
        let credentials = self.credentials.clone();
        let events = self.events.clone();
        let waker = self.waker.clone();
        let emit = move |event: RemoteEvent| {
            let _ = events.send(Event::Remote(event));
            waker.wake();
        };
        match request {
            RemoteRequest::Start => {
                if self.hybrid.remote_running {
                    return;
                }
                self.hybrid.remote_running = true;
                tokio::spawn(async move {
                    let mut watched = false;
                    loop {
                        let wait = match crate::friends::saved_identity(&credentials).await {
                            None => IDLE_POLL,
                            Some(identity) => match client.remote_poll(&identity).await {
                                Ok(polled) => {
                                    if polled.wanted != watched {
                                        watched = polled.wanted;
                                        emit(RemoteEvent::Watched(watched));
                                    }
                                    let commands: Vec<Incoming> = polled
                                        .commands
                                        .into_iter()
                                        .filter_map(|value| serde_json::from_value(value).ok())
                                        .collect();
                                    if !commands.is_empty() {
                                        emit(RemoteEvent::Commands(commands));
                                    }
                                    if watched { WATCHED_POLL } else { IDLE_POLL }
                                }
                                Err(_) => {
                                    if watched {
                                        watched = false;
                                        emit(RemoteEvent::Watched(false));
                                    }
                                    IDLE_POLL
                                }
                            },
                        };
                        tokio::time::sleep(wait).await;
                    }
                });
            }
            RemoteRequest::State(state) => {
                tokio::spawn(async move {
                    if let Some(identity) = crate::friends::saved_identity(&credentials).await {
                        let _ = client.remote_state(&identity, &state).await;
                    }
                });
            }
            RemoteRequest::Reply { id, value } => {
                tokio::spawn(async move {
                    if let Some(identity) = crate::friends::saved_identity(&credentials).await {
                        let _ = client.remote_reply(&identity, &id, &value).await;
                    }
                });
            }
            RemoteRequest::Search { id, query } => {
                let search = self.remote_search(query);
                tokio::spawn(async move {
                    let value = search.await;
                    if let Some(identity) = crate::friends::saved_identity(&credentials).await {
                        let _ = client.remote_reply(&identity, &id, &value).await;
                    }
                });
            }
            RemoteRequest::LinkCode { client_id } => {
                tokio::spawn(async move {
                    let answer = match crate::friends::saved_identity(&credentials).await {
                        None => Err("Connect to your group first.".to_owned()),
                        Some(identity) => client.link_start(&identity, client_id.as_deref()).await,
                    };
                    emit(RemoteEvent::Code(answer));
                });
            }
        }
    }

    /// All three sources at once. A source that can't answer just adds nothing.
    fn remote_search(
        &self,
        query: String,
    ) -> impl std::future::Future<Output = serde_json::Value> + Send + 'static {
        let api = Arc::clone(&self.api);
        let soundcloud = self.soundcloud_client();
        async move {
            let spotify = async {
                let client = api.client_for(Operation::CatalogSearch).await.ok()?;
                client.search(&query, &["track", "artist"]).await.ok()
            };
            let sound = async {
                let client = soundcloud?;
                client.search_tracks(&query, 20, 0).await.ok().map(|page| {
                    page.collection
                        .iter()
                        .map(|track| track.to_track(None))
                        .collect::<Vec<_>>()
                })
            };
            let resolver = crate::youtube::Resolver::default();
            let video = crate::youtube::browse::videos_for(&resolver, &query);
            let (spotify, sound, video) = tokio::join!(spotify, sound, video);
            let (tracks, artists): (Vec<Track>, Vec<serde_json::Value>) = spotify
                .map(|found| {
                    (
                        found.tracks.map(|page| page.items).unwrap_or_default(),
                        found
                            .artists
                            .map(|page| page.items)
                            .unwrap_or_default()
                            .iter()
                            .take(8)
                            .map(|artist| {
                                serde_json::json!({
                                    "id": artist.id,
                                    "name": artist.name,
                                    "image": crate::api::models::pick_image(&artist.images, 300),
                                })
                            })
                            .collect(),
                    )
                })
                .unwrap_or_default();
            serde_json::json!({
                "artists": artists,
                "spotify": songs_json(&tracks),
                "soundcloud": songs_json(&sound.unwrap_or_default()),
                "youtube": songs_json(&video.unwrap_or_default()),
            })
        }
    }
}
