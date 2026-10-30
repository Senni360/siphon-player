use super::*;
use crate::backend::soundcloud_library::{Request, Response};
use crate::soundcloud::library::{PendingLike, cache_key};

impl App {
    pub(super) fn expire_soundcloud_session(&mut self, request: u64) {
        if request != self.soundcloud_account_request {
            return;
        }
        let crate::soundcloud::account::Account::Session { user, .. } = &self.soundcloud_account
        else {
            return;
        };
        let account = crate::soundcloud::account::Account::Profile(user.clone());
        self.cancel_soundcloud_playlist_adds();
        self.soundcloud_account = account;
        self.reset_soundcloud_collections();
        self.soundcloud_library.reset();
        self.refresh_soundcloud_library();
        if let Page::SoundCloud(uri) = self.page().clone() {
            self.load_soundcloud_page(uri, false);
        }
        self.save_soundcloud_account();
        self.soundcloud_reconnect = true;
        self.toast_error(
            gettext(
                self.locale,
                "Your SoundCloud session expired. Reconnect SoundCloud.",
            )
            .into_owned(),
        );
    }

    pub(super) fn own_soundcloud_likes_context(&self, uri: &str) -> bool {
        self.soundcloud_account
            .user()
            .is_some_and(|user| uri == format!("soundcloud:likes:{}", user.user_id))
    }

    pub(super) fn overlay_soundcloud_likes(&self, tracks: &mut Vec<Track>) {
        tracks.retain(|track| {
            crate::soundcloud::track_id(&track.uri)
                .is_none_or(|id| self.soundcloud_library.edits.get(&id) != Some(&false))
        });
        let mut seen: HashSet<_> = tracks.iter().map(|track| track.uri.clone()).collect();
        for track in self.soundcloud_library.snapshot.likes.iter().rev() {
            if crate::soundcloud::track_id(&track.uri)
                .is_some_and(|id| self.soundcloud_library.edits.get(&id) == Some(&true))
                && seen.insert(track.uri.clone())
            {
                tracks.insert(0, track.clone());
            }
        }
    }

    pub(super) fn soundcloud_connect_failed(
        &mut self,
        error: crate::soundcloud::account::ConnectError,
        request: u64,
    ) {
        if request != self.soundcloud_request {
            return;
        }
        let error = crate::soundcloud::connect::error_message(self.locale, &error).into_owned();
        if let Some(Dialog::SoundCloudConnect(draft)) = &mut self.dialog {
            draft.busy = false;
            draft.error = Some(error.clone());
        }
        self.toast_error(error);
    }

    pub(super) fn accept_soundcloud_account(
        &mut self,
        account: crate::soundcloud::account::Account,
        request: u64,
    ) {
        if request != self.soundcloud_request {
            return;
        }
        self.backend.send(Command::AcceptSoundCloudAccount {
            account: account.clone(),
            request,
        });
        self.soundcloud_account_request = request;
        if let Some(user) = account.user()
            && matches!(&self.dialog, Some(Dialog::SoundCloudConnect(draft)) if draft.busy)
        {
            self.toast(
                gettext(self.locale, "Connected as {name}").replace("{name}", &user.username),
            );
            self.dialog = None;
        }
        self.cancel_soundcloud_playlist_adds();
        self.soundcloud_account = account;
        self.soundcloud_reconnect = false;
        self.reset_soundcloud_collections();
        self.soundcloud_library.reset();
        self.refresh_soundcloud_library();
        let page = self.page().clone();
        if let Page::SoundCloud(uri) = page {
            self.load_soundcloud_page(uri, false);
        }
        self.save_soundcloud_account();
    }

    // Keep generations monotonic so queued answers from the previous session
    // cannot repopulate a private playlist after disconnecting or switching.
    pub(super) fn reset_soundcloud_collections(&mut self) {
        self.reset_track_details();
        // Metadata learned from a private library/feed belongs to this
        // connection. User-saved mixed playlists remain explicit local data.
        self.track_cache
            .retain(|uri, _| !crate::soundcloud::is_uri(uri));
        self.track_used
            .retain(|uri, _| !crate::soundcloud::is_uri(uri));
        self.reload_soundcloud_hybrid_context();
        for page in self.soundcloud_pages.values_mut() {
            page.generation = page.generation.wrapping_add(1);
            page.content = Loadable::NotLoaded;
            page.loading = false;
            page.error = None;
        }
        self.search.soundcloud_request = self.search.soundcloud_request.wrapping_add(1);
        self.search.soundcloud_pending = false;
        self.search.soundcloud = Loadable::NotLoaded;
        self.backend.send(Command::SoundCloudSearch(
            crate::soundcloud::browse::SearchRequest {
                query: String::new(),
                serial: self.search.serial,
                request: self.search.soundcloud_request,
                more: None,
            },
        ));
    }

    pub(super) fn save_soundcloud_account(&mut self) {
        if !self.offline
            && let Err(error) =
                crate::soundcloud::account::State::from_account(&self.soundcloud_account).save(
                    &self.files,
                    &self.dirs.soundcloud_account_file(),
                    self.backend.soundcloud_write_failure(),
                )
        {
            self.toast_error(error.to_string());
        }
    }

    pub fn can_like_soundcloud(&self) -> bool {
        self.soundcloud_account.token().is_some()
    }

    pub fn can_repost_soundcloud(&self, uri: &str) -> bool {
        self.can_like_soundcloud()
            && !self.offline
            && self.network_reachable
            && self.soundcloud_library.snapshot.reposts.loaded
            && crate::soundcloud::reposts::Target::from_uri(uri)
                .is_some_and(|target| !self.soundcloud_library.repost_edits.pending(target))
    }

    pub fn soundcloud_reposted(&self, uri: &str) -> bool {
        crate::soundcloud::reposts::Target::from_uri(uri)
            .is_some_and(|target| self.soundcloud_library.snapshot.reposts.contains(target))
    }

    pub(super) fn toggle_soundcloud_repost(&mut self, uri: &str) {
        if !self.can_repost_soundcloud(uri) {
            return;
        }
        let Some(target) = crate::soundcloud::reposts::Target::from_uri(uri) else {
            return;
        };
        let library = &mut self.soundcloud_library;
        if let Some((serial, reposted)) = library
            .repost_edits
            .toggle(&mut library.snapshot.reposts, target)
        {
            self.backend
                .send(Command::SoundCloudLibrary(Request::Repost {
                    generation: library.generation,
                    target,
                    reposted,
                    serial,
                }));
        }
    }

    pub(super) fn refresh_soundcloud_library(&mut self) {
        if self.soundcloud_account.user().is_none()
            || self.soundcloud_library.refreshing
            || self.soundcloud_library.loading
            || self.offline
        {
            return;
        }
        self.soundcloud_library.refreshing = true;
        if self.can_like_soundcloud()
            && self.soundcloud_library.following.status
                == crate::soundcloud::following::Status::ConnectSoundCloud
        {
            self.soundcloud_library.following.status = crate::soundcloud::following::Status::Idle;
        }
        self.soundcloud_library.loading = true;
        self.backend
            .send(Command::SoundCloudLibrary(Request::Refresh {
                generation: self.soundcloud_library.generation,
            }));
    }

    pub(super) fn load_more_soundcloud_likes(&mut self) {
        let library = &mut self.soundcloud_library;
        if library.loading || self.offline || !self.network_reachable {
            return;
        }
        if let Some(next) = library.snapshot.next_likes.clone() {
            library.loading = true;
            library.error = None;
            self.backend.send(Command::SoundCloudLibrary(Request::More {
                generation: library.generation,
                next,
            }));
        }
    }

    fn save_soundcloud_library(&mut self) {
        if self.offline
            || !self.soundcloud_library.pending.is_empty()
            || self.soundcloud_library.repost_edits.is_pending()
        {
            return;
        }
        let Some(key) = cache_key(&self.soundcloud_account) else {
            return;
        };
        let cache =
            crate::account_cache::Cache::new(key.clone(), &self.soundcloud_library.snapshot);
        if let Ok(bytes) = serde_json::to_vec(&cache) {
            self.files.write_or(
                &self
                    .dirs
                    .cache
                    .join("soundcloud-library")
                    .join(format!("{key}.json")),
                bytes,
                fastframe_file::Access::Private,
                self.backend.soundcloud_write_failure(),
            );
        }
    }

    pub(super) fn handle_soundcloud_library(&mut self, response: Response) {
        let generation = match &response {
            Response::Cached { generation, .. }
            | Response::Likes { generation, .. }
            | Response::Playlists { generation, .. }
            | Response::Artists { generation, .. }
            | Response::Reposts { generation, .. }
            | Response::Reposted { generation, .. }
            | Response::Following { generation, .. }
            | Response::Refreshed { generation }
            | Response::Liked { generation, .. } => *generation,
        };
        if generation != self.soundcloud_library.generation {
            return;
        }
        match response {
            Response::Cached {
                snapshot: Some(mut snapshot),
                ..
            } => {
                if !self.soundcloud_library.snapshot.likes_loaded
                    && self.soundcloud_library.pending.is_empty()
                {
                    self.overlay_soundcloud_likes(&mut snapshot.likes);
                    if self.soundcloud_library.snapshot.following.loaded {
                        snapshot.following = self.soundcloud_library.snapshot.following.clone();
                    }
                    if self.soundcloud_library.snapshot.reposts.loaded {
                        snapshot.reposts = self.soundcloud_library.snapshot.reposts.clone();
                    }
                    self.soundcloud_library
                        .repost_edits
                        .overlay(&mut snapshot.reposts);
                    self.cache_soundcloud_tracks(&snapshot.likes);
                    self.soundcloud_library.snapshot = snapshot;
                    self.soundcloud_library.rebuild_membership();
                    self.soundcloud_library.revision =
                        self.soundcloud_library.revision.wrapping_add(1);
                }
            }
            Response::Cached { .. } => {}
            Response::Following {
                serial,
                more,
                result,
                ..
            } => {
                let library = &mut self.soundcloud_library;
                if serial != library.following.serial {
                    return;
                }
                match result {
                    Ok(page) => {
                        library.snapshot.following.absorb(page, more);
                        library.following.status = crate::soundcloud::following::Status::Ready;
                        let tracks = library
                            .snapshot
                            .following
                            .items
                            .iter()
                            .filter_map(|item| match &item.content {
                                crate::soundcloud::following::Content::Track(track) => {
                                    Some(track.as_ref().clone())
                                }
                                crate::soundcloud::following::Content::Playlist(_) => None,
                            })
                            .collect::<Vec<_>>();
                        self.cache_soundcloud_tracks(&tracks);
                        self.save_soundcloud_library();
                    }
                    Err(error) => {
                        library.following.status =
                            crate::soundcloud::following::Status::Error(error)
                    }
                }
            }
            Response::Reposts { result, .. } => match result {
                Ok(mut snapshot) => {
                    self.soundcloud_library.repost_edits.overlay(&mut snapshot);
                    self.soundcloud_library.snapshot.reposts = snapshot;
                    self.save_soundcloud_library();
                }
                Err(error) => {
                    self.toast_error(format!("Could not load SoundCloud reposts: {error}"))
                }
            },
            Response::Reposted {
                target,
                serial,
                result,
                ..
            } => {
                let library = &mut self.soundcloud_library;
                if library.repost_edits.finish(
                    &mut library.snapshot.reposts,
                    target,
                    serial,
                    result.is_ok(),
                ) {
                    if let Err(error) = result {
                        self.toast_error(format!(
                            "Could not change the SoundCloud repost: {error}"
                        ));
                    }
                    self.save_soundcloud_library();
                }
            }
            Response::Likes { more, result, .. } => {
                self.soundcloud_library.loading = false;
                match result {
                    Ok(page) => {
                        self.cache_soundcloud_tracks(&page.collection);
                        self.soundcloud_library
                            .absorb_likes(page.collection, page.next_href, more);
                        self.save_soundcloud_library();
                        if self.table_sorts.contains_key(&Page::SoundCloudLikes) {
                            self.load_more_soundcloud_likes();
                        }
                    }
                    Err(error) => {
                        self.soundcloud_library.error = Some(error.clone());
                        self.toast_error(error);
                    }
                }
            }
            Response::Refreshed { .. } => self.soundcloud_library.refreshing = false,
            Response::Playlists { result, .. } => match result {
                Ok(playlists) => {
                    self.soundcloud_library.snapshot.playlists = playlists;
                    self.save_soundcloud_library();
                }
                Err(error) => self.toast_error(error),
            },
            Response::Artists { result, .. } => match result {
                Ok(artists) => {
                    self.soundcloud_library.snapshot.artists = artists;
                    self.save_soundcloud_library();
                }
                Err(error) => self.toast_error(error),
            },
            Response::Liked {
                id, serial, result, ..
            } => {
                if self
                    .soundcloud_library
                    .pending
                    .get(&id)
                    .is_none_or(|pending| pending.serial != serial)
                {
                    return;
                }
                let pending = self.soundcloud_library.pending.remove(&id).unwrap();
                if let Err(error) = result {
                    self.soundcloud_library.edits.insert(id, pending.before);
                    let uri = crate::soundcloud::track_uri(id);
                    self.soundcloud_library
                        .snapshot
                        .likes
                        .retain(|track| track.uri != uri);
                    if pending.before
                        && let Some(track) = pending.track
                    {
                        let index = pending
                            .index
                            .unwrap_or(0)
                            .min(self.soundcloud_library.snapshot.likes.len());
                        self.soundcloud_library.snapshot.likes.insert(index, track);
                    }
                    self.soundcloud_library.revision =
                        self.soundcloud_library.revision.wrapping_add(1);
                    self.toast_error(error);
                }
                self.save_soundcloud_library();
            }
        }
    }

    pub(super) fn toggle_soundcloud_like(&mut self, uri: &str) {
        let Some(id) = crate::soundcloud::track_id(uri) else {
            return;
        };
        if !self.can_like_soundcloud() || self.soundcloud_library.pending.contains_key(&id) {
            return;
        }
        let library = &mut self.soundcloud_library;
        let before = library.is_liked(id);
        let index = library
            .snapshot
            .likes
            .iter()
            .position(|track| track.uri == uri);
        let track = index
            .map(|index| library.snapshot.likes[index].clone())
            .or_else(|| self.track_cache.get(uri).cloned());
        library.serial = library.serial.wrapping_add(1);
        let serial = library.serial;
        library.pending.insert(
            id,
            PendingLike {
                serial,
                before,
                track: track.clone(),
                index,
            },
        );
        library.edits.insert(id, !before);
        if before {
            library.snapshot.likes.retain(|track| track.uri != uri);
        } else if let Some(track) = track {
            library.snapshot.likes.insert(0, track);
        }
        library.revision = library.revision.wrapping_add(1);
        self.backend.send(Command::SoundCloudLibrary(Request::Like {
            generation: library.generation,
            id,
            liked: !before,
            serial,
        }));
    }

    pub(super) fn load_soundcloud_following(&mut self, more: bool) {
        use crate::soundcloud::following::Status;
        if !self.can_like_soundcloud() {
            self.soundcloud_library.following.status = Status::ConnectSoundCloud;
            return;
        }
        let library = &mut self.soundcloud_library;
        if self.offline || !self.network_reachable || library.following.status == Status::Loading {
            return;
        }
        let next = if more {
            let Some(next) = library.snapshot.following.next.clone() else {
                return;
            };
            Some(next)
        } else {
            None
        };
        library.following.serial = library.following.serial.wrapping_add(1);
        library.following.status = Status::Loading;
        self.backend
            .send(Command::SoundCloudLibrary(Request::Following {
                generation: library.generation,
                serial: library.following.serial,
                next,
                cached: library.snapshot.following.items.len(),
            }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soundcloud::account::{Account, SessionToken};

    fn app(name: &str) -> App {
        let root = std::env::current_dir()
            .unwrap()
            .join(".cache")
            .join(format!("soundcloud-{name}-{}", std::process::id()));
        let mut app = App::new(
            &Waker::default(),
            AppDirs {
                config: root.join("config"),
                state: root.join("state"),
                cache: root.join("cache"),
            },
            Settings::default(),
            AppOptions {
                media_controls: false,
                restore_sign_in: false,
                tray: false,
                autostart: false,
            },
        );
        crate::demo::populate(&mut app);
        crate::demo::soundcloud_profile(&mut app);
        app
    }

    fn session(app: &mut App) {
        app.soundcloud_account = Account::Session {
            user: app.soundcloud_account.user().unwrap().clone(),
            token: SessionToken::parse("2-000000-dummy_session_token").unwrap(),
        };
    }

    fn finish(mut app: App) {
        app.backend.shutdown();
        let root = app.dirs.state.parent().unwrap().to_owned();
        drop(app);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn account_reset_discards_learned_soundcloud_metadata_but_keeps_other_sources() {
        let mut app = app("account-cache-reset");
        let private = Track {
            uri: "soundcloud:track:999".into(),
            name: "Private feed recording".into(),
            ..Default::default()
        };
        let spotify = "spotify:track:retained".to_owned();
        app.cache_soundcloud_tracks(std::slice::from_ref(&private));
        app.track_cache.insert(spotify.clone(), Track::default());
        app.track_used.insert(spotify.clone(), Instant::now());
        let saved = crate::mixed_playlists::Entry::new(&private);
        let playlist = crate::mixed_playlists::Playlist {
            id: "explicitly-saved".into(),
            entries: vec![saved.clone()],
            ..Default::default()
        };
        app.mixed_playlists.playlists.push(playlist);
        app.reset_soundcloud_collections();
        assert!(!app.track_cache.contains_key(&private.uri));
        assert!(!app.track_used.contains_key(&private.uri));
        assert!(app.track_cache.contains_key(&spotify));
        assert!(app.track_used.contains_key(&spotify));
        assert_eq!(
            app.mixed_playlists.get("explicitly-saved").unwrap().entries,
            vec![saved]
        );
        finish(app);
    }

    #[test]
    fn repost_actions_are_session_only_optimistic_and_account_scoped() {
        use crate::soundcloud::reposts::{Snapshot, Target};
        let mut app = app("reposts");
        let uri = crate::soundcloud::track_uri(42);
        let target = Target::Track(42);
        app.soundcloud_library.snapshot.reposts.loaded = true;
        assert!(!app.can_repost_soundcloud(&uri));
        app.toggle_soundcloud_repost(&uri);
        assert!(!app.soundcloud_reposted(&uri));
        session(&mut app);
        app.offline = false;
        app.network_reachable = true;
        app.toggle_soundcloud_repost(&uri);
        app.offline = true;
        assert!(app.soundcloud_reposted(&uri));
        assert!(!app.can_repost_soundcloud(&uri));
        let generation = app.soundcloud_library.generation;
        app.handle_soundcloud_library(Response::Reposts {
            generation,
            result: Ok(Snapshot {
                loaded: true,
                ..Default::default()
            }),
        });
        assert!(app.soundcloud_reposted(&uri));
        app.handle_soundcloud_library(Response::Reposted {
            generation,
            target,
            serial: 999,
            result: Err("Old failure".into()),
        });
        assert!(app.soundcloud_reposted(&uri));
        app.handle_soundcloud_library(Response::Reposted {
            generation,
            target,
            serial: 1,
            result: Ok(()),
        });
        app.handle_soundcloud_library(Response::Reposts {
            generation,
            result: Ok(Snapshot {
                loaded: true,
                ..Default::default()
            }),
        });
        assert!(app.soundcloud_reposted(&uri));
        app.soundcloud_library.reset();
        app.handle_soundcloud_library(Response::Reposts {
            generation,
            result: Ok(Snapshot {
                tracks: [42].into(),
                loaded: true,
                ..Default::default()
            }),
        });
        assert!(!app.soundcloud_reposted(&uri));
        assert!(!app.soundcloud_library.snapshot.reposts.loaded);
        finish(app);
    }

    #[test]
    fn following_feed_requires_a_session_retains_failed_pages_and_ignores_old_accounts() {
        use crate::soundcloud::following::{Content, Item, Kind, Status};
        let mut app = app("following");
        app.load_soundcloud_following(false);
        assert_eq!(
            app.soundcloud_library.following.status,
            Status::ConnectSoundCloud
        );
        session(&mut app);
        let generation = app.soundcloud_library.generation;
        let page = crate::soundcloud::models::Page {
            collection: vec![Item {
                kind: Kind::Reposted,
                by: Artist {
                    uri: "soundcloud:user:2".into(),
                    ..Default::default()
                },
                created_at: "2026-10-30T12:00:00Z".into(),
                content: Content::Track(Box::new(Track {
                    uri: "soundcloud:track:1".into(),
                    ..Default::default()
                })),
            }],
            next_href: Some("stream?cursor=next".into()),
            total_results: None,
        };
        app.handle_soundcloud_library(Response::Following {
            generation,
            serial: 0,
            more: false,
            result: Ok(page.clone()),
        });
        assert_eq!(app.soundcloud_library.snapshot.following.items.len(), 1);
        app.handle_soundcloud_library(Response::Following {
            generation,
            serial: 0,
            more: true,
            result: Err("Try again".into()),
        });
        assert_eq!(app.soundcloud_library.snapshot.following.items.len(), 1);
        assert_eq!(
            app.soundcloud_library.snapshot.following.next.as_deref(),
            Some("stream?cursor=next")
        );
        assert_eq!(
            app.soundcloud_library.following.status,
            Status::Error("Try again".into())
        );
        app.soundcloud_library.reset();
        app.handle_soundcloud_library(Response::Following {
            generation,
            serial: 0,
            more: false,
            result: Ok(page),
        });
        assert!(app.soundcloud_library.snapshot.following.items.is_empty());
        assert_eq!(
            app.soundcloud_library.following.status,
            Status::ConnectSoundCloud
        );
        finish(app);
    }

    #[test]
    fn soundcloud_sidebar_collections_reuse_main_pages_and_reject_old_accounts() {
        let mut app = app("canonical-pages");
        let uri = crate::soundcloud::playlist_uri(424244);
        let page = Page::SoundCloud(uri.clone());
        app.open(page.clone());
        let generation = app.soundcloud_pages[&uri].generation;
        app.ensure_loaded(page.clone());
        assert_eq!(app.soundcloud_pages[&uri].generation, generation);
        assert!(app.soundcloud_pages[&uri].content.get().is_some());
        assert_eq!(Page::decode(&page.encode()), Some(page));
        let old_search = crate::soundcloud::browse::SearchRequest {
            query: app.search.committed.clone(),
            serial: app.search.serial,
            request: app.search.soundcloud_request,
            more: None,
        };
        app.apply(Action::DisconnectSoundCloud, &egui::Context::default());
        assert!(app.soundcloud_pages[&uri].content.get().is_none());
        app.offline = false;
        app.handle_backend_events(vec![Event::SoundCloudCollection {
            uri: uri.clone(),
            generation,
            more: false,
            result: Ok(SoundCloudCollection {
                title: "Old private playlist".into(),
                ..Default::default()
            }),
        }]);
        app.offline = true;
        app.soundcloud_search_finished(old_search, SoundCloudSearchResults::default());
        assert!(app.soundcloud_pages[&uri].content.get().is_none());
        assert!(app.search.soundcloud.get().is_none());
        finish(app);
    }

    #[test]
    fn liking_a_native_collection_row_inserts_its_full_track_immediately() {
        let mut app = app("collection-like-metadata");
        session(&mut app);
        let uri = crate::soundcloud::user_uri(123);
        app.soundcloud_pages
            .insert(uri.clone(), SoundCloudPage::default());
        let track = Track {
            uri: crate::soundcloud::track_uri(999),
            name: "Fresh upload".into(),
            ..Default::default()
        };
        app.offline = false;
        app.handle_backend_events(vec![Event::SoundCloudCollection {
            uri,
            generation: 0,
            more: false,
            result: Ok(SoundCloudCollection {
                tracks: vec![track.clone()],
                ..Default::default()
            }),
        }]);
        app.offline = true;
        app.apply(
            Action::ToggleSoundCloudLike(track.uri.clone()),
            &egui::Context::default(),
        );
        assert_eq!(app.soundcloud_library.snapshot.likes[0], track);
        assert_eq!(app.is_saved(&track.uri), Some(true));
        finish(app);
    }

    #[test]
    fn legacy_soundcloud_toggle_routes_to_session_likes_and_never_spotify() {
        let mut app = app("legacy-like-route");
        session(&mut app);
        let uri = app.soundcloud_library.snapshot.likes[0].uri.clone();
        app.apply(Action::ToggleSaved(uri.clone()), &egui::Context::default());
        assert_eq!(app.is_saved(&uri), Some(false));
        assert!(!app.soundcloud_library.pending.is_empty());
        assert!(!app.saved.contains_key(&uri));
        assert!(!app.saved_writes.contains_key(&uri));
        finish(app);
    }

    #[test]
    fn a_late_cached_library_preserves_completed_like_edits() {
        let mut app = app("late-cache-like");
        session(&mut app);
        let snapshot = app.soundcloud_library.snapshot.clone();
        let removed = snapshot.likes[0].clone();
        let added = Track {
            uri: crate::soundcloud::track_uri(999),
            name: "New like".into(),
            ..Default::default()
        };
        app.cache_soundcloud_tracks(std::slice::from_ref(&added));
        app.soundcloud_library.snapshot = Default::default();
        app.soundcloud_library.rebuild_membership();
        // An unlike completed before this refresh started loading its cache.
        let removed_id = crate::soundcloud::track_id(&removed.uri).unwrap();
        app.soundcloud_library.edits.insert(removed_id, false);
        app.toggle_soundcloud_like(&added.uri);
        let serial = app.soundcloud_library.pending[&999].serial;
        let generation = app.soundcloud_library.generation;
        app.handle_soundcloud_library(Response::Liked {
            generation,
            id: 999,
            serial,
            result: Ok(()),
        });
        app.handle_soundcloud_library(Response::Cached {
            generation,
            snapshot: Some(snapshot.clone()),
        });
        let expected: Vec<_> = std::iter::once(added.clone())
            .chain(
                snapshot
                    .likes
                    .into_iter()
                    .filter(|track| track.uri != removed.uri),
            )
            .collect();
        assert_eq!(app.soundcloud_library.snapshot.likes, expected);
        assert_eq!(app.is_saved(&added.uri), Some(true));
        assert_eq!(app.is_saved(&removed.uri), Some(false));
        finish(app);
    }

    #[test]
    fn soundcloud_likes_require_a_session_and_failed_writes_revert() {
        let mut app = app("like-rollback");
        let original = app.soundcloud_library.snapshot.likes.clone();
        let uri = original[0].uri.clone();
        let id = crate::soundcloud::track_id(&uri).unwrap();
        assert!(!app.can_like_soundcloud());
        app.apply(
            Action::ToggleSoundCloudLike(uri.clone()),
            &egui::Context::default(),
        );
        assert_eq!(app.soundcloud_library.snapshot.likes, original);
        assert!(app.soundcloud_library.pending.is_empty());
        session(&mut app);
        assert!(app.can_like_soundcloud());
        app.apply(
            Action::ToggleSoundCloudLike(uri.clone()),
            &egui::Context::default(),
        );
        assert_eq!(app.is_saved(&uri), Some(false));
        let serial = app.soundcloud_library.pending[&id].serial;
        // A lagging refresh cannot bring the removed row back.
        app.handle_soundcloud_library(Response::Likes {
            generation: app.soundcloud_library.generation,
            more: false,
            result: Ok(crate::soundcloud::models::Page {
                collection: original.clone(),
                next_href: None,
                total_results: None,
            }),
        });
        assert!(
            !app.soundcloud_library
                .snapshot
                .likes
                .iter()
                .any(|track| track.uri == uri)
        );
        app.handle_soundcloud_library(Response::Liked {
            generation: app.soundcloud_library.generation,
            id,
            serial,
            result: Err("Refused".into()),
        });
        assert_eq!(app.soundcloud_library.snapshot.likes, original);
        assert_eq!(app.is_saved(&uri), Some(true));
        assert_eq!(app.toasts.last().unwrap().message, "Refused");
        finish(app);
    }

    #[test]
    fn expiry_belongs_to_the_active_session_even_during_a_reconnect() {
        let mut app = app("expiry-during-reconnect");
        session(&mut app);
        app.soundcloud_request = 1;
        // The current session can expire while a new token is being verified.
        app.expire_soundcloud_session(0);
        assert!(!app.can_like_soundcloud());
        assert!(app.soundcloud_reconnect);
        let account = Account::Session {
            user: app.soundcloud_account.user().unwrap().clone(),
            token: SessionToken::parse("2-000000-another_session_token").unwrap(),
        };
        app.accept_soundcloud_account(account, 1);
        // An already queued expiry notice from the old connection cannot
        // disconnect the newly accepted token, even for the same user.
        app.expire_soundcloud_session(0);
        assert!(app.can_like_soundcloud());
        assert!(!app.soundcloud_reconnect);
        app.expire_soundcloud_session(1);
        assert!(!app.can_like_soundcloud());
        finish(app);
    }

    #[test]
    fn expired_soundcloud_session_clears_private_views_and_ignores_stale_expiry() {
        let mut app = app("session-expiry");
        session(&mut app);
        let user = app.soundcloud_account.user().unwrap().clone();
        let old_generation = app.soundcloud_library.generation;
        app.dialog = Some(Dialog::SoundCloudConnect(
            crate::soundcloud::connect::Draft {
                busy: true,
                advanced: true,
                ..Default::default()
            },
        ));
        let uri = crate::soundcloud::playlist_uri(424244);
        assert!(app.soundcloud_pages[&uri].content.get().is_some());
        app.expire_soundcloud_session(app.soundcloud_request.wrapping_add(1));
        assert!(app.can_like_soundcloud());
        app.expire_soundcloud_session(app.soundcloud_request);
        assert!(app.soundcloud_account == Account::Profile(user));
        assert!(app.soundcloud_reconnect);
        assert!(matches!(&app.dialog, Some(Dialog::SoundCloudConnect(draft)) if draft.busy));
        assert!(!app.can_like_soundcloud());
        assert!(app.soundcloud_pages[&uri].content.get().is_none());
        assert!(
            app.toasts
                .last()
                .unwrap()
                .message
                .contains("Reconnect SoundCloud")
        );
        app.handle_soundcloud_library(Response::Playlists {
            generation: old_generation,
            result: Ok(vec![Playlist {
                uri,
                name: "Private playlist".into(),
                ..Default::default()
            }]),
        });
        assert!(app.soundcloud_library.snapshot.playlists.is_empty());
        app.apply(Action::DisconnectSoundCloud, &egui::Context::default());
        assert!(!app.soundcloud_reconnect);
        finish(app);
    }

    #[test]
    fn account_changes_retry_pending_contexts_and_reject_already_queued_private_replies() {
        for change in 0..3 {
            let mut app = app(&format!("pending-context-account-{change}"));
            session(&mut app);
            let uri = crate::soundcloud::playlist_uri(999999);
            app.apply(
                Action::PlayContext {
                    uri: uri.clone(),
                    offset_uri: None,
                    offset_index: None,
                },
                &egui::Context::default(),
            );
            let resolved_generation = |commands: Vec<crate::backend::HybridCommand>| {
                commands
                    .into_iter()
                    .find_map(|command| match command {
                        crate::backend::HybridCommand::Resolve { generation, .. } => {
                            Some(generation)
                        }
                        _ => None,
                    })
                    .expect("context resolution")
            };
            let old = resolved_generation(app.backend.take_hybrid_commands());
            match change {
                0 => app.accept_soundcloud_account(
                    Account::Profile(app.soundcloud_account.user().unwrap().clone()),
                    app.soundcloud_request,
                ),
                1 => app.apply(Action::DisconnectSoundCloud, &egui::Context::default()),
                _ => app.expire_soundcloud_session(app.soundcloud_request),
            }
            let fresh = resolved_generation(app.backend.take_hybrid_commands());
            assert_ne!(old, fresh);
            for (generation, id) in [(old, 123), (fresh, 456)] {
                app.handle_hybrid_context(
                    generation,
                    Ok(crate::hybrid_queue::Context {
                        uri: uri.clone(),
                        name: "Set".into(),
                        tracks: vec![Track {
                            uri: crate::soundcloud::track_uri(id),
                            ..Default::default()
                        }],
                    }),
                );
                if generation == old {
                    assert!(app.now_playing().is_none());
                } else {
                    assert_eq!(
                        app.now_playing().unwrap().uri,
                        crate::soundcloud::track_uri(456)
                    );
                }
            }
            finish(app);
        }
    }

    #[test]
    fn account_changes_cancel_playlist_copies_and_reject_late_private_rows() {
        for change in 0..3 {
            let mut app = app(&format!("copy-account-change-{change}"));
            session(&mut app);
            let id = app
                .mixed_playlists
                .create("Private copy".into(), String::new());
            app.apply(
                Action::AddSoundCloudPlaylist {
                    uri: crate::soundcloud::playlist_uri(999999),
                    playlist_id: format!("spotifast:playlist:{id}"),
                    playlist_name: "Private copy".into(),
                },
                &egui::Context::default(),
            );
            let request = *app
                .pending_soundcloud_adds
                .keys()
                .next()
                .expect("copy pending");
            match change {
                0 => app.accept_soundcloud_account(
                    Account::Profile(app.soundcloud_account.user().unwrap().clone()),
                    app.soundcloud_request,
                ),
                1 => app.apply(Action::DisconnectSoundCloud, &egui::Context::default()),
                _ => app.expire_soundcloud_session(app.soundcloud_request),
            }
            assert!(app.pending_soundcloud_adds.is_empty());
            app.soundcloud_playlist_add_finished(
                request,
                true,
                Ok(vec![Track {
                    uri: crate::soundcloud::track_uri(999999),
                    name: "Late private track".into(),
                    ..Default::default()
                }]),
            );
            assert!(app.mixed_playlists.get(&id).unwrap().entries.is_empty());
            finish(app);
        }
    }

    #[test]
    fn soundcloud_failed_addition_reverts_and_disconnect_rejects_old_pages() {
        let mut app = app("like-addition");
        session(&mut app);
        let original = app.soundcloud_library.snapshot.likes.clone();
        let mut track = original[0].clone();
        track.uri = crate::soundcloud::track_uri(999);
        app.cache_soundcloud_tracks(&[track.clone()]);
        app.apply(
            Action::ToggleSoundCloudLike(track.uri.clone()),
            &egui::Context::default(),
        );
        assert_eq!(app.soundcloud_library.snapshot.likes[0], track);
        let generation = app.soundcloud_library.generation;
        let serial = app.soundcloud_library.pending[&999].serial;
        app.handle_soundcloud_library(Response::Liked {
            generation,
            id: 999,
            serial,
            result: Err("Offline".into()),
        });
        assert_eq!(app.soundcloud_library.snapshot.likes, original);
        app.apply(Action::DisconnectSoundCloud, &egui::Context::default());
        assert!(app.soundcloud_account.user().is_none());
        app.handle_soundcloud_library(Response::Likes {
            generation,
            more: false,
            result: Ok(crate::soundcloud::models::Page {
                collection: original,
                next_href: None,
                total_results: None,
            }),
        });
        assert!(app.soundcloud_library.snapshot.likes.is_empty());
        finish(app);
    }

    #[test]
    fn soundcloud_dialog_errors_clear_tokens_and_cancel_pending_connection() {
        let mut app = app("dialog-transitions");
        let ctx = egui::Context::default();
        app.dialog = Some(Dialog::SoundCloudConnect(
            crate::soundcloud::connect::Draft {
                advanced: true,
                token: "bad".into(),
                ..Default::default()
            },
        ));
        app.apply(Action::ConnectSoundCloud, &ctx);
        let Some(Dialog::SoundCloudConnect(draft)) = &app.dialog else {
            panic!("dialog retained")
        };
        assert!(draft.token.is_empty());
        assert!(draft.error.is_some());
        assert!(!draft.busy);
        if let Some(Dialog::SoundCloudConnect(draft)) = &mut app.dialog {
            draft.token = "2-000000-dummy_session_token".into();
        }
        app.apply(Action::ConnectSoundCloud, &ctx);
        let Some(Dialog::SoundCloudConnect(draft)) = &app.dialog else {
            panic!("dialog retained")
        };
        assert!(draft.busy);
        assert!(draft.token.is_empty());
        let request = app.soundcloud_request;
        app.apply(Action::CloseDialog, &ctx);
        assert!(app.dialog.is_none());
        assert!(app.soundcloud_request > request);
        // A result already queued by the backend cannot undo the cancellation.
        let stale = app.soundcloud_account.clone();
        app.accept_soundcloud_account(Account::Disconnected, request);
        assert!(app.soundcloud_account == stale);
        app.soundcloud_connect_failed(
            crate::soundcloud::account::ResolveError::Unauthorized.into(),
            request,
        );
        assert!(app.dialog.is_none());
        finish(app);
    }

    #[test]
    fn soundcloud_verified_connection_closes_the_dialog_and_reports_the_name() {
        let mut app = app("verified-connection");
        let account = app.soundcloud_account.clone();
        let name = account.user().unwrap().username.clone();
        app.dialog = Some(Dialog::SoundCloudConnect(
            crate::soundcloud::connect::Draft {
                profile: "demo-listener".into(),
                busy: true,
                ..Default::default()
            },
        ));
        app.accept_soundcloud_account(account, app.soundcloud_request);
        assert!(app.dialog.is_none());
        assert_eq!(
            app.toasts.last().unwrap().message,
            format!("Connected as {name}")
        );
        finish(app);
    }

    #[test]
    fn soundcloud_startup_restore_keeps_an_unsubmitted_connection_draft() {
        let mut app = app("restore-draft");
        app.dialog = Some(Dialog::SoundCloudConnect(
            crate::soundcloud::connect::Draft {
                profile: "another-listener".into(),
                ..Default::default()
            },
        ));
        app.accept_soundcloud_account(app.soundcloud_account.clone(), 0);
        let Some(Dialog::SoundCloudConnect(draft)) = &app.dialog else {
            panic!("restore must retain the draft")
        };
        assert_eq!(draft.profile, "another-listener");
        assert!(app.toasts.is_empty());
        finish(app);
    }

    #[test]
    fn soundcloud_likes_resolve_the_full_context_and_keep_the_clicked_song() {
        let mut app = app("full-likes-context");
        app.soundcloud_library.snapshot.next_likes =
            Some("https://api-v2.soundcloud.com/users/4242/track_likes?cursor=next".into());
        let original = app.soundcloud_library.snapshot.likes.clone();
        let uri = "soundcloud:likes:4242";
        app.apply(
            Action::PlayFromRow {
                context: RowContext::Context {
                    uri: uri.into(),
                    editable_playlist: None,
                },
                uri: original[1].uri.clone(),
                index: 1,
            },
            &egui::Context::default(),
        );
        let commands = app.backend.take_hybrid_commands();
        let generation = commands
            .iter()
            .find_map(|command| match command {
                crate::backend::HybridCommand::Resolve {
                    generation,
                    uri: context,
                } if context == uri => Some(*generation),
                _ => None,
            })
            .expect("partial likes must resolve their complete context");
        let mut extra = original[0].clone();
        extra.uri = crate::soundcloud::track_uri(9999);
        // The server inserted a row since the clicked page was read.
        app.handle_hybrid_context(
            generation,
            Ok(crate::hybrid_queue::Context {
                uri: uri.into(),
                name: "Likes".into(),
                tracks: vec![extra, original[0].clone(), original[1].clone()],
            }),
        );
        assert_eq!(app.now_playing().unwrap().uri, original[1].uri);
        finish(app);
    }
}
