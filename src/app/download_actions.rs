use super::App;
use crate::api::models::PlayableItem;
use crate::backend::Command;
use crate::downloads::{Event, Source, Target};

impl App {
    pub(crate) fn download_source(&self, item: &PlayableItem) -> Option<Source> {
        Source::from_item(item).or_else(|| {
            if !crate::soundcloud::is_track_uri(item.uri()) {
                return None;
            }
            Some(Source::SoundCloud {
                url: self.soundcloud_link(item.uri())?,
                title: item.name().into(),
                all: false,
            })
        })
    }

    pub(super) fn download_items(&mut self, items: Vec<PlayableItem>) {
        let mut seen = std::collections::HashSet::new();
        let sources: Vec<_> = items
            .iter()
            .filter(|item| seen.insert(item.uri()))
            .filter_map(|item| self.download_source(item))
            .collect();
        let title = if sources.len() == 1 {
            sources[0].title().to_string()
        } else {
            format!("{} tracks", sources.len())
        };
        if sources.is_empty() {
            self.toast_error("These items have no downloadable track link.");
            return;
        }
        self.enqueue_download(title, Target::Items(sources));
    }

    pub(super) fn download_all(&mut self, uri: String, name: String) {
        let target = if crate::soundcloud::playlist_id(&uri).is_some() {
            if let Some(url) = self.soundcloud_link(&uri) {
                Target::Items(vec![Source::SoundCloud {
                    url,
                    title: name.clone(),
                    all: true,
                }])
            } else {
                Target::SoundCloudSet {
                    uri,
                    title: name.clone(),
                }
            }
        } else if uri.starts_with("spotify:")
            && matches!(crate::util::uri_kind(&uri), Some("album" | "playlist"))
        {
            Target::SpotifyContext(uri)
        } else {
            return;
        };
        self.enqueue_download(name, target);
    }

    /// Downloads the song a friend shared. A Spotify share is matched by
    /// its title and artists, like any Spotify song; SoundCloud and YouTube
    /// shares go by their link.
    pub(super) fn download_share(&mut self, id: &str) {
        let Some(post) = self.friends_feed.posts.iter().find(|p| p.id == id) else {
            return;
        };
        let item = &post.item;
        let source = match item.source {
            crate::friends::Source::Youtube => Source::YouTube {
                url: item.url.clone(),
                title: item.title.clone(),
            },
            crate::friends::Source::Soundcloud => Source::SoundCloud {
                url: item.url.clone(),
                title: item.title.clone(),
                all: false,
            },
            crate::friends::Source::Spotify => {
                let Some(track_id) = item
                    .url
                    .split("/track/")
                    .nth(1)
                    .map(|rest| rest.split(['?', '/']).next().unwrap_or(rest))
                    .filter(|id| !id.is_empty())
                else {
                    self.toast_error("Only songs can be downloaded, not albums or playlists.");
                    return;
                };
                let track = self.track_cache.get(track_id).cloned().unwrap_or_else(|| {
                    crate::api::models::Track {
                        id: Some(track_id.to_owned()),
                        uri: format!("spotify:track:{track_id}"),
                        name: item.title.clone(),
                        artists: item
                            .artists
                            .iter()
                            .map(|name| crate::api::models::ArtistRef {
                                name: name.clone(),
                                ..Default::default()
                            })
                            .collect(),
                        duration_ms: item.duration_ms,
                        ..Default::default()
                    }
                });
                Source::Spotify(Box::new(track))
            }
        };
        let key = item.url.clone();
        let title = item.title.clone();
        self.enqueue_download_for(title, Target::Items(vec![source]), Some(key));
    }

    fn enqueue_download(&mut self, title: String, target: Target) {
        // A single song remembers its file, so its row can offer it.
        let key = match &target {
            Target::Items(items) if items.len() == 1 => Some(match &items[0] {
                Source::SoundCloud { url, .. } | Source::YouTube { url, .. } => url.clone(),
                Source::Spotify(track) => track.uri.clone(),
            }),
            _ => None,
        };
        self.enqueue_download_for(title, target, key);
    }

    fn enqueue_download_for(&mut self, title: String, target: Target, key: Option<String>) {
        if !self.downloads.available {
            self.toast_error(crate::downloads::INSTALL_HELP);
            return;
        }
        if let Some(request) = self.downloads.enqueue(title, target) {
            if let Some(key) = key {
                self.download_keys.insert(request.id, key);
            }
            self.toast("Download queued");
            self.backend.send(Command::Download(request));
        } else {
            self.toast_error("Wait for a download to finish before starting another.");
        }
    }

    pub(super) fn handle_download_event(&mut self, event: Event) {
        match event {
            Event::Available(available) => self.downloads.available = available,
            Event::Progress { id, progress } => self.downloads.progress(id, progress),
            Event::ItemFailed { id, message } => {
                if self.downloads.active.contains_key(&id) {
                    self.download_error(message);
                }
            }
            Event::Finished { id, result } => {
                let key = self.download_keys.remove(&id);
                let Some(progress) = self.downloads.active.remove(&id) else {
                    return;
                };
                let file = result
                    .as_ref()
                    .ok()
                    .and_then(|summary| summary.last_file.clone())
                    .map(std::path::PathBuf::from);
                if let (Some(key), Some(file)) = (key, file.clone()) {
                    self.downloaded_files.insert(key, file);
                }
                // Leave room for the completion/cancellation notice even
                // when several tracks have just reported a failed match.
                let excess = self.toasts.len().saturating_sub(3);
                drop(self.toasts.drain(..excess));
                match result {
                    Err(error) => self.download_error(error),
                    Ok(summary) if summary.cancelled => {
                        if summary.downloaded == 0 {
                            self.toast("Download cancelled");
                        } else {
                            self.toast(format!(
                                "Download cancelled. Kept {} saved files.",
                                summary.downloaded
                            ));
                        }
                    }
                    Ok(summary) if summary.failed > 0 => self.download_error(format!(
                        "Downloaded {} files. {} downloads failed.",
                        summary.downloaded, summary.failed
                    )),
                    Ok(summary) if summary.downloaded == 1 => match file {
                        Some(file) => self.toast_with(
                            format!("Downloaded: {}", progress.title),
                            "Show in folder",
                            crate::model::Action::RevealFile(file),
                        ),
                        None => self.toast(format!("Downloaded: {}", progress.title)),
                    },
                    Ok(summary) => self.toast(format!("Downloaded {} files", summary.downloaded)),
                }
            }
        }
    }

    fn download_error(&mut self, message: String) {
        // Extractor errors can contain signed media URLs. Show their bounded
        // explanation to the listener without toast_error's log::warn path.
        self.toasts.push(crate::model::Toast {
            message,
            kind: crate::model::ToastKind::Error,
            created: std::time::Instant::now(),
            action: None,
        });
        let excess = self.toasts.len().saturating_sub(4);
        drop(self.toasts.drain(..excess));
    }
}
