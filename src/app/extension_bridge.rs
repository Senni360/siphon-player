//! External additions reuse the normal catalogue requests and playlist writes.
use super::*;

pub(super) struct PendingPlaylistLink {
    uri: String,
    request: u64,
    started: bool,
    waiting_for_sign_in: bool,
    offset: Option<u32>,
    album: Option<Album>,
    items: Vec<PlayableItem>,
    soundcloud_account: u64,
}

impl App {
    pub(super) fn open_playlist_link(&mut self, text: &str) {
        let Some(uri) = crate::link::playlist_item(text) else {
            self.toast_error("Unsupported playlist URL");
            return;
        };
        self.load_generation += 1;
        let request = self.load_generation;
        let waiting_for_sign_in = uri.starts_with("spotify:") && self.user.is_none();
        self.dialog = Some(Dialog::AddToPlaylist {
            request,
            name: if uri.starts_with("spotify:") {
                uri.clone()
            } else if crate::youtube::is_track_uri(&uri) {
                "YouTube".into()
            } else {
                "SoundCloud".into()
            },
            items: Vec::new(),
            source: None,
            query: String::new(),
            loading: true,
        });
        if waiting_for_sign_in {
            self.dialog = None;
            self.toast("Sign in to Spotify to choose a playlist");
        }
        self.pending_playlist_link = Some(PendingPlaylistLink {
            uri,
            request,
            started: false,
            waiting_for_sign_in,
            offset: None,
            album: None,
            items: Vec::new(),
            soundcloud_account: self.soundcloud_account_request,
        });
        self.start_pending_playlist_link();
    }

    pub(super) fn cancel_playlist_link(&mut self) {
        self.pending_playlist_link = None;
        if matches!(self.dialog, Some(Dialog::AddToPlaylist { .. })) {
            self.dialog = None;
        }
    }

    pub(super) fn playlist_link_signed_out(&mut self) {
        if self.pending_playlist_link.as_ref().is_some_and(|pending| {
            pending.uri.starts_with("spotify:") && !pending.waiting_for_sign_in
        }) {
            self.playlist_link_failed();
        }
    }

    fn playlist_link_is_open(&self, request: u64) -> bool {
        matches!(self.dialog, Some(Dialog::AddToPlaylist { request: current, .. }) if current == request)
    }

    pub(super) fn start_pending_playlist_link(&mut self) {
        let Some(pending) = &self.pending_playlist_link else {
            return;
        };
        if pending.waiting_for_sign_in {
            if self.user.is_none() {
                return;
            }
            self.dialog = Some(Dialog::AddToPlaylist {
                request: pending.request,
                name: pending.uri.clone(),
                items: Vec::new(),
                source: None,
                query: String::new(),
                loading: true,
            });
            self.pending_playlist_link
                .as_mut()
                .unwrap()
                .waiting_for_sign_in = false;
        }
        let pending = self.pending_playlist_link.as_ref().unwrap();
        if !self.playlist_link_is_open(pending.request) {
            self.pending_playlist_link = None;
            return;
        }
        if pending.started || (pending.uri.starts_with("spotify:") && self.user.is_none()) {
            return;
        }
        if pending.uri.starts_with("spotify:playlist:") && self.playlist_link_waits_for_session() {
            return;
        }
        if crate::youtube::is_track_uri(&pending.uri) {
            let uri = pending.uri.clone();
            if let Some(track) = self.hybrid_cached_track(&uri) {
                self.playlist_link_ready(
                    track.name.clone(),
                    vec![PlayableItem::Track(track)],
                    None,
                );
            } else {
                self.pending_playlist_link.as_mut().unwrap().started = true;
                self.backend.send(Command::YouTubeTrack {
                    uri,
                    generation: self.youtube.generation,
                });
            }
            return;
        }
        let cached_id = pending
            .uri
            .strip_prefix("spotify:track:")
            .map(str::to_owned);
        if let Some(id) = cached_id
            && let Some(track) = self.read_cached_track(&id)
        {
            self.playlist_link_ready(track.name.clone(), vec![PlayableItem::Track(track)], None);
            return;
        }
        let pending = self.pending_playlist_link.as_mut().unwrap();
        pending.started = true;
        let id = util::uri_id(&pending.uri).unwrap_or_default().to_owned();
        match util::uri_kind(&pending.uri) {
            Some("track") => self.backend.api(ApiRequest::Track { id }),
            Some("album") => self.backend.api(ApiRequest::Album { id }),
            Some("playlist") => self.backend.api(ApiRequest::Playlist {
                id,
                generation: pending.request,
            }),
            _ => self.backend.send(Command::ResolvePlaylistLink {
                url: pending.uri.clone(),
                request: pending.request,
            }),
        }
    }

    fn playlist_link_failed(&mut self) {
        self.cancel_playlist_link();
        self.toast_error("Could not load tracks from this link");
    }

    pub(super) fn youtube_playlist_link_pending(&self, uri: &str) -> bool {
        self.pending_playlist_link.as_ref().is_some_and(|pending| {
            pending.uri == uri && self.playlist_link_is_open(pending.request)
        })
    }

    pub(super) fn youtube_playlist_link_received(
        &mut self,
        uri: &str,
        result: &Result<Box<Track>, String>,
    ) {
        if !self.youtube_playlist_link_pending(uri) {
            return;
        }
        match result {
            Ok(track) => self.playlist_link_ready(
                track.name.clone(),
                vec![PlayableItem::Track(*track.clone())],
                None,
            ),
            Err(_) => self.cancel_playlist_link(),
        }
    }

    fn playlist_link_waits_for_session(&self) -> bool {
        !self.shared_grant
            && matches!(
                self.local_playback,
                LocalPlayback::Connecting | LocalPlayback::WaitingForNetwork
            )
    }

    fn playlist_link_ready(
        &mut self,
        name: String,
        items: Vec<PlayableItem>,
        source: Option<String>,
    ) {
        self.pending_playlist_link = None;
        if items.is_empty() && source.is_none() {
            self.playlist_link_failed();
            return;
        }
        if let Some(Dialog::AddToPlaylist {
            name: shown,
            items: selected,
            source: collection,
            loading,
            ..
        }) = &mut self.dialog
        {
            *shown = name;
            *selected = items;
            *collection = source;
            *loading = false;
        }
    }

    pub(super) fn playlist_link_resolved(
        &mut self,
        request: u64,
        result: Result<crate::extension_bridge::ResolvedItem, String>,
    ) {
        let Some(pending) = &self.pending_playlist_link else {
            return;
        };
        if pending.request != request || !self.playlist_link_is_open(request) {
            return;
        }
        if pending.soundcloud_account != self.soundcloud_account_request {
            self.playlist_link_failed();
            return;
        }
        match result {
            Ok(crate::extension_bridge::ResolvedItem::Track(track)) => {
                self.playlist_link_ready(
                    track.name.clone(),
                    vec![PlayableItem::Track(*track)],
                    None,
                );
            }
            Ok(crate::extension_bridge::ResolvedItem::Set { uri, name, tracks }) => {
                if tracks.is_empty() {
                    self.playlist_link_failed();
                    return;
                }
                self.soundcloud_pages.insert(
                    uri.clone(),
                    SoundCloudPage {
                        generation: request,
                        content: Loadable::Loaded(SoundCloudCollection {
                            title: name.clone(),
                            tracks,
                            next: None,
                        }),
                        ..Default::default()
                    },
                );
                self.playlist_link_ready(name, Vec::new(), Some(uri));
            }
            Err(_) => self.playlist_link_failed(),
        }
    }

    /// Generation-tagged collection pages belong solely to this picker. Track
    /// and album headers also continue through the normal metadata cache path.
    pub(super) fn playlist_link_response(&mut self, response: &ApiResponse) -> bool {
        let Some(mut pending) = self.pending_playlist_link.take() else {
            return false;
        };
        if pending.waiting_for_sign_in {
            self.pending_playlist_link = Some(pending);
            return false;
        }
        if !self.playlist_link_is_open(pending.request) || !pending.started {
            if self.playlist_link_is_open(pending.request) {
                self.pending_playlist_link = Some(pending);
            }
            return false;
        }
        let id = util::uri_id(&pending.uri).unwrap_or_default();
        let kind = util::uri_kind(&pending.uri);
        let mut consumed = false;
        let mut complete = false;
        let mut failed = false;
        match response {
            ApiResponse::Track {
                id: received,
                result,
            } if kind == Some("track") && received == id => match result {
                Ok(track) => {
                    self.playlist_link_ready(
                        track.name.clone(),
                        vec![PlayableItem::Track(track.clone())],
                        None,
                    );
                    return false;
                }
                Err(_) => failed = true,
            },
            ApiResponse::Album {
                id: received,
                result,
            } if kind == Some("album") && received == id && pending.offset.is_none() => {
                match result {
                    Ok(album) => {
                        let mut album = album.clone();
                        album.tracks = None;
                        pending.album = Some(album);
                        pending.offset = Some(0);
                        self.backend.api(ApiRequest::AlbumTracks {
                            id: id.into(),
                            offset: 0,
                            generation: pending.request,
                        });
                    }
                    Err(_) => failed = true,
                }
            }
            ApiResponse::Playlist {
                id: received,
                generation,
                result,
            } if kind == Some("playlist") && received == id && *generation == pending.request => {
                consumed = true;
                match result {
                    Ok(playlist) => {
                        if let Some(Dialog::AddToPlaylist { name, .. }) = &mut self.dialog {
                            name.clone_from(&playlist.name);
                        }
                        pending.offset = Some(0);
                        self.backend.api(ApiRequest::PlaylistItems {
                            id: id.into(),
                            offset: 0,
                            generation: pending.request,
                        });
                    }
                    Err(_) => failed = true,
                }
            }
            ApiResponse::AlbumTracks {
                id: received,
                generation,
                offset,
                result,
            } if kind == Some("album") && received == id && *generation == pending.request => {
                consumed = true;
                if pending.offset != Some(*offset) {
                    self.pending_playlist_link = Some(pending);
                    return true;
                }
                match result {
                    Ok(page) => {
                        for mut track in page.items.clone() {
                            if !track.uri.starts_with("spotify:track:") {
                                continue;
                            }
                            track.album.clone_from(&pending.album);
                            pending.items.push(PlayableItem::Track(track));
                        }
                        let next = offset.saturating_add(page.items.len() as u32);
                        complete = next >= page.total && page.next.is_none();
                        failed = !complete && (page.items.is_empty() || next > 100_000);
                        if !complete && !failed {
                            pending.offset = Some(next);
                            self.backend.api(ApiRequest::AlbumTracks {
                                id: id.into(),
                                offset: next,
                                generation: pending.request,
                            });
                        }
                    }
                    Err(_) => failed = true,
                }
            }
            ApiResponse::PlaylistItems {
                id: received,
                generation,
                offset,
                result,
            } if kind == Some("playlist") && received == id && *generation == pending.request => {
                consumed = true;
                if pending.offset != Some(*offset) {
                    self.pending_playlist_link = Some(pending);
                    return true;
                }
                match result {
                    Ok(page) => {
                        pending.items.extend(page.items.iter().filter_map(
                            |row| match row.playable() {
                                Some(PlayableItem::Track(track))
                                    if track.uri.starts_with("spotify:track:") =>
                                {
                                    Some(PlayableItem::Track(track.clone()))
                                }
                                _ => None,
                            },
                        ));
                        let next = offset.saturating_add(page.items.len() as u32);
                        complete = next >= page.total && page.next.is_none();
                        failed = !complete && (page.items.is_empty() || next > 100_000);
                        if !complete && !failed {
                            pending.offset = Some(next);
                            self.backend.api(ApiRequest::PlaylistItems {
                                id: id.into(),
                                offset: next,
                                generation: pending.request,
                            });
                        }
                    }
                    Err(_) => failed = true,
                }
            }
            _ => {}
        }
        if failed
            && pending.uri.starts_with("spotify:playlist:")
            && self.playlist_link_waits_for_session()
        {
            // The first Web API fallback may have raced playback restoration.
            // Retry once that session can serve the existing playlist requests.
            pending.started = false;
            pending.offset = None;
            pending.items.clear();
            self.pending_playlist_link = Some(pending);
        } else if failed {
            self.playlist_link_failed();
        } else if complete {
            let name =
                pending
                    .album
                    .map(|album| album.name)
                    .unwrap_or_else(|| match &self.dialog {
                        Some(Dialog::AddToPlaylist { name, .. }) => name.clone(),
                        _ => String::new(),
                    });
            self.playlist_link_ready(name, pending.items, None);
        } else {
            self.pending_playlist_link = Some(pending);
        }
        consumed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::Page as ApiPage;
    use crate::extension_bridge::ResolvedItem;

    fn app(name: &str) -> App {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".cache")
            .join(format!(
                "bridge-{name}-{}",
                crate::mixed_playlists::new_id()
            ));
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
        app.backend.set_offline(true);
        app.offline = true;
        app
    }

    fn cleanup(mut app: App) {
        app.backend.shutdown();
        assert!(app.files.flush(Duration::from_secs(10)));
        let root = app.dirs.state.parent().unwrap().to_owned();
        drop(app);
        if root.exists() {
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    fn track(uri: &str) -> Track {
        Track {
            uri: uri.into(),
            name: uri.into(),
            ..Default::default()
        }
    }

    fn signed_in(app: &mut App) {
        app.user = Some(User {
            id: "owner".into(),
            ..Default::default()
        });
    }

    fn selection(app: &App) -> Vec<String> {
        let Some(Dialog::AddToPlaylist {
            items,
            loading: false,
            ..
        }) = &app.dialog
        else {
            panic!("resolved picker");
        };
        items.iter().map(|item| item.uri().to_owned()).collect()
    }

    #[test]
    fn spotify_cold_start_waits_for_sign_in_without_blocking_it() {
        let mut app = app("signed-out");
        app.window_hidden = true;
        app.apply(
            Action::AddPlaylistLink("https://open.spotify.com/track/song".into()),
            &egui::Context::default(),
        );
        assert!(app.wants_show);
        assert!(app.dialog.is_none(), "the sign-in form remains accessible");
        assert!(!app.playlist_link_response(&ApiResponse::Devices(Ok(Vec::new()))));
        assert!(
            app.pending_playlist_link.is_some(),
            "unrelated startup replies retain the item"
        );
        app.playlist_link_signed_out();
        assert!(app.pending_playlist_link.is_some());
        signed_in(&mut app);
        app.start_pending_playlist_link();
        assert!(matches!(
            app.dialog,
            Some(Dialog::AddToPlaylist { loading: true, .. })
        ));
        app.handle_api(ApiResponse::Track {
            id: "song".into(),
            result: Ok(track("spotify:track:song")),
        });
        assert_eq!(selection(&app), ["spotify:track:song"]);
        assert!(app.pending_playlist_link.is_none());
        cleanup(app);
    }

    #[test]
    fn spotify_album_collects_every_page_and_preserves_duplicate_occurrences() {
        let mut app = app("album-pages");
        signed_in(&mut app);
        app.open_playlist_link("spotify:album:album");
        let request = app.pending_playlist_link.as_ref().unwrap().request;
        app.playlist_link_response(&ApiResponse::Album {
            id: "album".into(),
            result: Ok(Album {
                id: "album".into(),
                name: "Album".into(),
                ..Default::default()
            }),
        });
        app.playlist_link_response(&ApiResponse::AlbumTracks {
            id: "album".into(),
            generation: request,
            offset: 0,
            result: Ok(ApiPage {
                items: vec![
                    track("spotify:track:one"),
                    Track::default(),
                    track("spotify:track:two"),
                ],
                total: 4,
                ..Default::default()
            }),
        });
        assert!(matches!(
            app.dialog,
            Some(Dialog::AddToPlaylist { loading: true, .. })
        ));
        // A repeated earlier page must not duplicate its rows.
        app.playlist_link_response(&ApiResponse::AlbumTracks {
            id: "album".into(),
            generation: request,
            offset: 0,
            result: Ok(ApiPage::whole(vec![track("spotify:track:wrong")])),
        });
        app.playlist_link_response(&ApiResponse::AlbumTracks {
            id: "album".into(),
            generation: request,
            offset: 3,
            result: Ok(ApiPage {
                items: vec![track("spotify:track:one")],
                total: 4,
                ..Default::default()
            }),
        });
        assert_eq!(
            selection(&app),
            [
                "spotify:track:one",
                "spotify:track:two",
                "spotify:track:one"
            ]
        );
        let Some(Dialog::AddToPlaylist { items, .. }) = &app.dialog else {
            unreachable!()
        };
        assert!(items.iter().all(|item| matches!(item, PlayableItem::Track(track) if track.album.as_ref().is_some_and(|album| album.name == "Album"))));
        cleanup(app);
    }

    #[test]
    fn spotify_playlist_pages_count_unavailable_slots_and_keep_track_order() {
        let mut app = app("playlist-pages");
        signed_in(&mut app);
        app.open_playlist_link("spotify:playlist:source");
        let request = app.pending_playlist_link.as_ref().unwrap().request;
        app.playlist_link_response(&ApiResponse::Playlist {
            id: "source".into(),
            generation: request,
            result: Ok(Playlist {
                name: "Source".into(),
                ..Default::default()
            }),
        });
        app.playlist_link_response(&ApiResponse::PlaylistItems {
            id: "source".into(),
            generation: request,
            offset: 0,
            result: Ok(ApiPage {
                items: vec![
                    PlaylistItem {
                        item: Some(PlayableItem::Track(track("spotify:track:one"))),
                        ..Default::default()
                    },
                    PlaylistItem::default(),
                ],
                total: 3,
                ..Default::default()
            }),
        });
        assert_eq!(app.pending_playlist_link.as_ref().unwrap().offset, Some(2));
        app.playlist_link_response(&ApiResponse::PlaylistItems {
            id: "source".into(),
            generation: request,
            offset: 2,
            result: Ok(ApiPage {
                items: vec![PlaylistItem {
                    item: Some(PlayableItem::Track(track("spotify:track:one"))),
                    ..Default::default()
                }],
                total: 3,
                ..Default::default()
            }),
        });
        assert_eq!(selection(&app), ["spotify:track:one", "spotify:track:one"]);
        cleanup(app);
    }

    #[test]
    fn cold_playlist_add_waits_for_the_restoring_playback_session() {
        let mut app = app("playlist-restore");
        signed_in(&mut app);
        app.local_playback = LocalPlayback::Connecting;
        app.open_playlist_link("spotify:playlist:source");
        assert!(!app.pending_playlist_link.as_ref().unwrap().started);
        app.local_playback = LocalPlayback::Ready {
            device_id: "local".into(),
        };
        app.start_pending_playlist_link();
        assert!(app.pending_playlist_link.as_ref().unwrap().started);
        let request = app.pending_playlist_link.as_ref().unwrap().request;
        app.local_playback = LocalPlayback::Connecting;
        app.playlist_link_response(&ApiResponse::Playlist {
            id: "source".into(),
            generation: request,
            result: Err(crate::api::ApiError::Network(
                "session still restoring".into(),
            )),
        });
        assert!(!app.pending_playlist_link.as_ref().unwrap().started);
        assert!(matches!(
            app.dialog,
            Some(Dialog::AddToPlaylist { loading: true, .. })
        ));
        assert!(
            !app.toasts
                .iter()
                .any(|toast| toast.kind == ToastKind::Error)
        );
        cleanup(app);
    }

    #[test]
    fn soundcloud_sets_reuse_the_existing_optimistic_copy_and_track_addition_paths() {
        let mut app = app("soundcloud-add");
        let target = app
            .mixed_playlists
            .create("Destination".into(), String::new());
        app.open_playlist_link("https://soundcloud.com/artist/sets/set");
        let request = app.pending_playlist_link.as_ref().unwrap().request;
        let tracks = vec![
            track("soundcloud:track:1"),
            track("soundcloud:track:2"),
            track("soundcloud:track:1"),
        ];
        app.playlist_link_resolved(
            request,
            Ok(ResolvedItem::Set {
                uri: "soundcloud:playlist:7".into(),
                name: "Set".into(),
                tracks: tracks.clone(),
            }),
        );
        assert!(
            matches!(&app.dialog, Some(Dialog::AddToPlaylist { source: Some(uri), loading: false, .. }) if uri == "soundcloud:playlist:7")
        );
        app.apply(Action::PlaylistLinkChosen, &egui::Context::default());
        app.apply(
            Action::AddSoundCloudPlaylist {
                uri: "soundcloud:playlist:7".into(),
                playlist_id: format!("spotifast:playlist:{target}"),
                playlist_name: "Destination".into(),
            },
            &egui::Context::default(),
        );
        assert_eq!(
            app.mixed_playlists
                .get(&target)
                .unwrap()
                .entries
                .iter()
                .map(|entry| entry.track.clone())
                .collect::<Vec<_>>(),
            tracks
        );
        assert!(
            app.pending_soundcloud_adds.is_empty(),
            "resolved membership needs no second read"
        );
        assert!(
            app.toasts
                .iter()
                .any(|toast| toast.message == "Added 3 SoundCloud tracks to Destination")
        );
        app.open_playlist_link("https://soundcloud.com/artist/song");
        let request = app.pending_playlist_link.as_ref().unwrap().request;
        app.playlist_link_resolved(
            request,
            Ok(ResolvedItem::Track(Box::new(track("soundcloud:track:4")))),
        );
        assert_eq!(selection(&app), ["soundcloud:track:4"]);
        cleanup(app);
    }

    #[test]
    fn stale_canceled_and_failed_resolutions_cannot_replace_the_picker() {
        let mut app = app("stale-resolve");
        app.open_playlist_link("https://soundcloud.com/artist/first");
        let old = app.pending_playlist_link.as_ref().unwrap().request;
        app.open_playlist_link("https://soundcloud.com/artist/second");
        let current = app.pending_playlist_link.as_ref().unwrap().request;
        app.playlist_link_resolved(
            old,
            Ok(ResolvedItem::Track(Box::new(track("soundcloud:track:1")))),
        );
        assert!(matches!(
            app.dialog,
            Some(Dialog::AddToPlaylist { loading: true, .. })
        ));
        app.apply(Action::CloseDialog, &egui::Context::default());
        app.playlist_link_resolved(
            current,
            Ok(ResolvedItem::Track(Box::new(track("soundcloud:track:2")))),
        );
        assert!(app.dialog.is_none());
        app.open_playlist_link("https://soundcloud.com/artist/missing");
        let current = app.pending_playlist_link.as_ref().unwrap().request;
        app.playlist_link_resolved(
            current,
            Err("private diagnostics must not reach the toast".into()),
        );
        assert!(app.dialog.is_none());
        assert_eq!(
            app.toasts.last().unwrap().message,
            "Could not load tracks from this link"
        );
        cleanup(app);
    }

    #[test]
    fn an_incomplete_spotify_page_fails_without_offering_a_partial_copy() {
        let mut app = app("incomplete");
        signed_in(&mut app);
        app.open_playlist_link("spotify:album:album");
        let request = app.pending_playlist_link.as_ref().unwrap().request;
        app.playlist_link_response(&ApiResponse::Album {
            id: "album".into(),
            result: Ok(Album::default()),
        });
        app.playlist_link_response(&ApiResponse::AlbumTracks {
            id: "album".into(),
            generation: request,
            offset: 0,
            result: Ok(ApiPage {
                items: vec![],
                total: 1,
                ..Default::default()
            }),
        });
        assert!(app.dialog.is_none());
        assert!(app.pending_playlist_link.is_none());
        assert!(
            app.toasts
                .iter()
                .any(|toast| toast.kind == ToastKind::Error)
        );
        cleanup(app);
    }
}
