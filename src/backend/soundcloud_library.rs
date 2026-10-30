//! SoundCloud account library work stays on the backend runtime.
use super::*;
use crate::soundcloud::{
    api::Client,
    library::{Snapshot, cache_key},
    models as wire,
};

pub enum Request {
    Following {
        generation: u64,
        serial: u64,
        next: Option<String>,
        cached: usize,
    },
    Repost {
        generation: u64,
        target: crate::soundcloud::reposts::Target,
        reposted: bool,
        serial: u64,
    },
    Refresh {
        generation: u64,
    },
    More {
        generation: u64,
        next: String,
    },
    Like {
        generation: u64,
        id: u64,
        liked: bool,
        serial: u64,
    },
}

pub enum Response {
    Following {
        generation: u64,
        serial: u64,
        more: bool,
        result: Result<wire::Page<crate::soundcloud::following::Item>, String>,
    },
    Reposts {
        generation: u64,
        result: Result<crate::soundcloud::reposts::Snapshot, String>,
    },
    Reposted {
        generation: u64,
        target: crate::soundcloud::reposts::Target,
        serial: u64,
        result: Result<(), String>,
    },
    Cached {
        generation: u64,
        snapshot: Option<Snapshot>,
    },
    Likes {
        generation: u64,
        more: bool,
        result: Result<wire::Page<Track>, String>,
    },
    Playlists {
        generation: u64,
        result: Result<Vec<Playlist>, String>,
    },
    Artists {
        generation: u64,
        result: Result<Vec<Artist>, String>,
    },
    Refreshed {
        generation: u64,
    },
    Liked {
        generation: u64,
        id: u64,
        serial: u64,
        result: Result<(), String>,
    },
}

impl Worker {
    pub(super) fn soundcloud_library(&self, request: Request) {
        let Some(user) = self.hybrid.account.user() else {
            if let Request::Following {
                generation,
                serial,
                next,
                ..
            } = request
            {
                self.emit(Event::SoundCloudLibrary(Response::Following {
                    generation,
                    serial,
                    more: next.is_some(),
                    result: Err(
                        "Connect SoundCloud with a session to see the following feed.".into(),
                    ),
                }));
            }
            return;
        };
        let user_id = user.user_id;
        let key = cache_key(&self.hybrid.account).expect("connected account");
        let session = self.hybrid.account.token().is_some();
        let path = self
            .dirs
            .cache
            .join("soundcloud-library")
            .join(format!("{key}.json"));
        let client = self.soundcloud_client().expect("SoundCloud initialized");
        let client_path = self.dirs.state.join("soundcloud-client.json");
        let lease = self.credentials.lease(CredentialSlot::SoundCloud);
        let events = self.events.clone();
        let waker = self.waker.clone();
        let write_client = crate::http::build_collection_client(&self.engine_config.proxy);
        tokio::spawn(async move {
            let emit = |response| {
                let _ = events.send(Event::SoundCloudLibrary(response));
                waker.wake();
            };
            match request {
                Request::Following {
                    generation,
                    serial,
                    next,
                    cached,
                } => {
                    let result = if session && lease.current() {
                        following(&client, next.as_deref(), cached)
                            .await
                            .map_err(|error| error.to_string())
                    } else {
                        Err("Connect SoundCloud with a session to see the following feed.".into())
                    };
                    emit(Response::Following {
                        generation,
                        serial,
                        more: next.is_some(),
                        result,
                    });
                }
                Request::Refresh { generation } => {
                    let snapshot = crate::account_cache::read(&path, &key).await;
                    let cached_likes = snapshot
                        .as_ref()
                        .map_or(0, |snapshot: &Snapshot| snapshot.likes.len());
                    emit(Response::Cached {
                        generation,
                        snapshot,
                    });
                    let likes = async {
                        let result = refresh_likes(&client, user_id, cached_likes)
                            .await
                            .map_err(|error| error.to_string());
                        emit(Response::Likes {
                            generation,
                            more: false,
                            result,
                        });
                    };
                    let playlists = async {
                        let result = playlists(&client, user_id, session)
                            .await
                            .map_err(|error| error.to_string());
                        emit(Response::Playlists { generation, result });
                    };
                    let artists = async {
                        let result = followings(&client, user_id)
                            .await
                            .map_err(|error| error.to_string());
                        emit(Response::Artists { generation, result });
                    };
                    let reposts = async {
                        if session {
                            let result = client
                                .reposted_ids()
                                .await
                                .map_err(|error| error.to_string());
                            emit(Response::Reposts { generation, result });
                        }
                    };
                    tokio::join!(likes, playlists, artists, reposts);
                    emit(Response::Refreshed { generation });
                }
                Request::More { generation, next } => {
                    let result = client
                        .next_likes(&next)
                        .await
                        .map(|page| page.map(|track| track.to_track(None)))
                        .map_err(|error| error.to_string());
                    emit(Response::Likes {
                        generation,
                        more: true,
                        result,
                    });
                }
                Request::Like {
                    generation,
                    id,
                    liked,
                    serial,
                } => {
                    let result = if session && lease.current() {
                        match write_client {
                            Ok(http) => client
                                .set_liked(&http, user_id, id, liked)
                                .await
                                .map_err(|error| error.to_string()),
                            Err(error) => Err(error),
                        }
                    } else {
                        Err("Connect a SoundCloud session to change likes.".into())
                    };
                    emit(Response::Liked {
                        generation,
                        id,
                        serial,
                        result,
                    });
                }
                Request::Repost {
                    generation,
                    target,
                    reposted,
                    serial,
                } => {
                    let result = if session && lease.current() {
                        match write_client {
                            Ok(http) => client
                                .set_reposted(&http, target, reposted)
                                .await
                                .map_err(|error| error.to_string()),
                            Err(error) => Err(error),
                        }
                    } else {
                        Err("Connect a SoundCloud session to change reposts.".into())
                    };
                    emit(Response::Reposted {
                        generation,
                        target,
                        serial,
                        result,
                    });
                }
            }
            hybrid::persist_client_id(&client, client_path).await;
        });
    }
}

// Replace a cached prefix only after its refresh catches up, as for likes.
pub(crate) async fn following(
    client: &Client,
    next: Option<&str>,
    cached: usize,
) -> crate::soundcloud::api::Result<wire::Page<crate::soundcloud::following::Item>> {
    let mut page = client.following_feed(next).await?;
    if next.is_none() {
        let mut cursors = std::collections::HashSet::new();
        let mut items = std::collections::HashSet::new();
        page.collection.retain(|item| items.insert(item.key()));
        while page.collection.len() < cached {
            let Some(next) = page.next_href.take() else {
                break;
            };
            if !cursors.insert(next.clone()) {
                return Err(crate::soundcloud::api::Error::Decode);
            }
            let more = client.following_feed(Some(&next)).await?;
            page.collection.extend(
                more.collection
                    .into_iter()
                    .filter(|item| items.insert(item.key())),
            );
            page.next_href = more.next_href;
        }
    }
    Ok(page)
}

pub(crate) async fn playlists(
    client: &Client,
    id: u64,
    session: bool,
) -> crate::soundcloud::api::Result<Vec<Playlist>> {
    let mut playlists = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cursors = std::collections::HashSet::new();
    // Read the owner's feed with OAuth explicitly. The collected feed alone
    // can omit private sets owned by this user.
    let mut page = client.user_playlists(id, 100, 0).await?;
    loop {
        playlists.extend(
            page.collection
                .into_iter()
                .filter(|list| seen.insert(list.id))
                .map(|list| list.to_playlist()),
        );
        let Some(next) = page.next_href else { break };
        if !cursors.insert(next.clone()) {
            return Err(crate::soundcloud::api::Error::Decode);
        }
        page = client.next_page(&next).await?;
    }
    if session {
        cursors.clear();
        let mut page = client.user_collection_playlists(id, 100, 0).await?;
        loop {
            playlists.extend(
                page.collection
                    .into_iter()
                    .filter(|list| seen.insert(list.id))
                    .map(|list| list.to_playlist()),
            );
            let Some(next) = page.next_href else { break };
            if !cursors.insert(next.clone()) {
                return Err(crate::soundcloud::api::Error::Decode);
            }
            page = client.next_page(&next).await?;
        }
    }
    {
        cursors.clear();
        let mut page = client.user_liked_playlists(id, 100, 0).await?;
        loop {
            playlists.extend(
                page.collection
                    .into_iter()
                    .filter(|list| seen.insert(list.id))
                    .map(|list| list.to_playlist()),
            );
            let Some(next) = page.next_href else { break };
            if !cursors.insert(next.clone()) {
                return Err(crate::soundcloud::api::Error::Decode);
            }
            page = client.next_liked_playlists(&next).await?;
        }
    }
    Ok(playlists)
}

async fn followings(client: &Client, id: u64) -> crate::soundcloud::api::Result<Vec<Artist>> {
    let mut artists = Vec::new();
    let mut cursors = std::collections::HashSet::new();
    let mut page = client.user_followings(id, 100, 0).await?;
    loop {
        artists.extend(page.collection.into_iter().map(|user| user.to_artist()));
        let Some(next) = page.next_href else { break };
        if !cursors.insert(next.clone()) {
            return Err(crate::soundcloud::api::Error::Decode);
        }
        page = client.next_page(&next).await?;
    }
    Ok(artists)
}

// Keep the cached prefix on screen until its replacement has caught up.
pub(super) async fn refresh_likes(
    client: &Client,
    id: u64,
    cached: usize,
) -> crate::soundcloud::api::Result<wire::Page<Track>> {
    let mut page = client.user_likes(id, 100, 0).await?;
    let mut cursors = std::collections::HashSet::new();
    while page.collection.len() < cached {
        let Some(next) = page.next_href.take() else {
            break;
        };
        if !cursors.insert(next.clone()) {
            return Err(crate::soundcloud::api::Error::Decode);
        }
        let more = client.next_likes(&next).await?;
        page.collection.extend(more.collection);
        page.next_href = more.next_href;
    }
    Ok(page.map(|track| track.to_track(None)))
}
