//! Local YouTube library state and stale-result guards for the existing views.
use super::*;
use crate::youtube::likes::Likes;

#[derive(Default)]
pub(super) struct State {
    pub generation: u64,
    account: Option<String>,
    likes: Loadable<Likes>,
}

impl App {
    /// Install explicitly invented library states without disk or helper work.
    #[cfg(feature = "demo")]
    pub fn set_demo_youtube_likes(&mut self, likes: Loadable<Likes>) {
        self.youtube.generation = self.youtube.generation.wrapping_add(1);
        self.youtube.account = self.user_id().map(str::to_owned);
        self.youtube.likes = likes;
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
        self.soundcloud_pages.remove("youtube:likes");
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
        });
    }

    pub(super) fn youtube_likes_received(
        &mut self,
        account: &str,
        generation: u64,
        result: Result<Likes, String>,
    ) {
        if generation != self.youtube.generation || self.user_id() != Some(account) {
            return;
        }
        self.youtube.likes = Loadable::from_result(result);
        self.refresh_youtube_likes_page();
    }

    pub(super) fn load_youtube_likes_page(&mut self) {
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
        let bytes = likes.encode();
        if !self.offline {
            match bytes {
                Ok(bytes) => self.files.write(
                    &crate::youtube::likes::path(&self.dirs.state, &account),
                    bytes,
                    fastframe_file::Access::Private,
                ),
                Err(_) => self.toast_error("Could not save YouTube likes. Try again."),
            }
        }
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
