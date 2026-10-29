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

    fn enqueue_download(&mut self, title: String, target: Target) {
        if !self.downloads.available {
            self.toast_error(crate::downloads::INSTALL_HELP);
            return;
        }
        if let Some(request) = self.downloads.enqueue(title, target) {
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
                let Some(progress) = self.downloads.active.remove(&id) else {
                    return;
                };
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
                    Ok(summary) if summary.downloaded == 1 => {
                        self.toast(format!("Downloaded: {}", progress.title))
                    }
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
        });
        let excess = self.toasts.len().saturating_sub(4);
        drop(self.toasts.drain(..excess));
    }
}
