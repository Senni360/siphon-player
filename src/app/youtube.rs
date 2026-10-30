//! Local YouTube library state and stale-result guards for the existing views.
use super::*;
use crate::youtube::likes::{Likes, Restored, Writes};

#[derive(Default)]
pub(super) struct State {
    pub generation: u64,
    account: Option<String>,
    likes: Loadable<Likes>,
    writes: std::sync::Arc<Writes>,
    pub(super) write_revision: u64,
    save_error: bool,
    restore_warning: bool,
}

impl App {
    /// Install explicitly invented library states without disk or helper work.
    #[cfg(any(test, feature = "demo"))]
    pub fn set_demo_youtube_likes(&mut self, likes: Loadable<Likes>) {
        self.youtube.generation = self.youtube.generation.wrapping_add(1);
        self.youtube.account = self.user_id().map(str::to_owned);
        self.youtube.likes = likes;
        self.youtube.save_error = false;
        self.youtube.restore_warning = false;
        self.refresh_youtube_likes_page();
    }

    pub fn youtube_likes(&self) -> Option<&[Track]> {
        if self.youtube.account.as_deref() != self.user_id() {
            return None;
        }
        self.youtube
            .likes
            .get()
            .map(|likes| likes.tracks.as_slice())
    }

    pub(super) fn reset_youtube(&mut self) {
        self.youtube.generation = self.youtube.generation.wrapping_add(1);
        self.youtube.account = None;
        self.youtube.likes = Loadable::NotLoaded;
        self.youtube.write_revision = 0;
        self.youtube.save_error = false;
        self.youtube.restore_warning = false;
        self.soundcloud_pages.remove("youtube:likes");
        self.table_rows
            .remove(&Page::SoundCloud("youtube:likes".into()));
        self.track_cache.retain(|uri, track| {
            !crate::youtube::is_track_uri(uri) && !crate::youtube::is_track_uri(&track.uri)
        });
        self.backend.send(Command::CancelYouTubeMetadata);
    }

    pub(super) fn request_youtube_likes(&mut self) {
        let Some(account) = self.user_id().map(str::to_owned) else {
            return;
        };
        if self.youtube.account.as_ref() != Some(&account) {
            self.reset_youtube();
            self.youtube.account = Some(account.clone());
        }
        if !self.youtube.likes.needs_load() {
            return;
        }
        self.youtube.likes = Loadable::Loading;
        self.backend.send(Command::YouTubeLikes {
            account,
            generation: self.youtube.generation,
            files: self.files.clone(),
            writes: self.youtube.writes.clone(),
        });
    }

    pub(super) fn youtube_likes_received(
        &mut self,
        account: &str,
        generation: u64,
        result: Result<Restored, String>,
    ) {
        if generation != self.youtube.generation
            || self.user_id() != Some(account)
            || self.youtube.account.as_deref() != Some(account)
            || !self.youtube.likes.is_loading()
        {
            return;
        }
        self.youtube.restore_warning = result.as_ref().is_ok_and(|restored| restored.missed_save);
        self.youtube.likes = Loadable::from_result(result.map(|restored| restored.likes));
        self.refresh_youtube_likes_page();
    }

    pub(super) fn load_youtube_likes_page(&mut self) {
        if self.youtube.save_error && self.youtube.likes.get().is_some() {
            self.save_youtube_likes();
        }
        self.request_youtube_likes();
        self.refresh_youtube_likes_page();
    }

    fn refresh_youtube_likes_page(&mut self) {
        let tracks = self.youtube_likes().unwrap_or_default().to_vec();
        let page = self
            .soundcloud_pages
            .entry("youtube:likes".into())
            .or_default();
        page.generation = page.generation.wrapping_add(1);
        page.loading = self.youtube.likes.is_loading();
        page.error = match &self.youtube.likes {
            Loadable::Failed(error) => Some(error.clone()),
            _ if self.user.is_none() => Some("Sign in to see your local YouTube likes.".into()),
            _ if self.youtube.save_error => Some(crate::youtube::likes::SAVE_ERROR.into()),
            _ if self.youtube.restore_warning => {
                Some(crate::youtube::likes::RESTORE_WARNING.into())
            }
            _ => None,
        };
        page.content = Loadable::Loaded(SoundCloudCollection {
            title: "YouTube Likes".into(),
            tracks,
            next: None,
        });
        self.table_rows
            .remove(&Page::SoundCloud("youtube:likes".into()));
    }

    pub(super) fn toggle_youtube_like(&mut self, uri: &str) {
        let Some(account) = self.user_id().map(str::to_owned) else {
            return;
        };
        if self.youtube.account.as_ref() != Some(&account) {
            return;
        }
        let track = self.hybrid_cached_track(uri);
        let Some(likes) = self.youtube.likes.get_mut() else {
            self.toast_error("YouTube likes are not ready. Open YouTube Likes and try again.");
            return;
        };
        if let Some(index) = likes.tracks.iter().position(|track| track.uri == uri) {
            likes.tracks.remove(index);
        } else if let Some(track) = track {
            likes.tracks.insert(0, track);
        } else {
            return;
        }
        self.save_youtube_likes();
        self.refresh_youtube_likes_page();
    }

    fn save_youtube_likes(&mut self) {
        let Some(account) = self.user_id().map(str::to_owned) else {
            return;
        };
        if self.youtube.account.as_deref() != Some(account.as_str()) {
            return;
        }
        let Some(likes) = self.youtube.likes.get() else {
            return;
        };
        self.youtube.save_error = false;
        self.youtube.restore_warning = false;
        if self.offline {
            return;
        }
        let revision = self.youtube.writes.begin(&account);
        self.youtube.write_revision = revision;
        let Ok(bytes) = likes.encode() else {
            self.youtube.writes.failed(&account, revision);
            self.youtube.save_error = true;
            self.toast_error(crate::youtube::likes::SAVE_ERROR);
            return;
        };
        self.files.write_or(
            &crate::youtube::likes::path(&self.dirs.state, &account),
            bytes,
            fastframe_file::Access::Private,
            self.backend.youtube_likes_write_failure(
                account,
                self.youtube.generation,
                revision,
                self.youtube.writes.clone(),
            ),
        );
    }

    pub(super) fn youtube_likes_write_failed(
        &mut self,
        account: &str,
        generation: u64,
        revision: u64,
    ) {
        if generation != self.youtube.generation
            || self.user_id() != Some(account)
            || self.youtube.account.as_deref() != Some(account)
            || revision != self.youtube.write_revision
        {
            return;
        }
        // Keep the listener's edits visible and retry the same snapshot when
        // they reopen Likes. Never turn a failed save into an empty library.
        self.youtube.save_error = true;
        self.toast_error(crate::youtube::likes::SAVE_ERROR);
        self.refresh_youtube_likes_page();
    }

    pub(super) fn youtube_track_received(
        &mut self,
        generation: u64,
        uri: &str,
        result: Result<Box<Track>, String>,
    ) {
        if generation != self.youtube.generation {
            return;
        }
        if !self.youtube_playlist_link_pending(uri)
            && !self
                .pending_pastes
                .iter()
                .any(|paste| paste.uris.iter().any(|pasted| pasted == uri))
        {
            return;
        }
        self.youtube_playlist_link_received(uri, &result);
        match result {
            Ok(track) => {
                self.cache_soundcloud_tracks(std::slice::from_ref(track.as_ref()));
                self.resolve_pasted_song(uri, Some(PlayableItem::Track(*track)));
            }
            Err(error) => {
                self.resolve_pasted_song(uri, None);
                self.toast_error(error);
            }
        }
    }
}
