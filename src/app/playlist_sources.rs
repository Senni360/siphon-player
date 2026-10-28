//! Local playlist edits use their own coordinate space. Only this adapter can
//! translate a derived row into a Spotify write.
use super::*;
use crate::playlist_overlay::{MergedRow, Overlay};

pub(crate) const OVERLAY_TARGET: &str = "spotifast:overlay:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PlaylistRowOrigin {
    Spotify(u32),
    Overlay(String),
}

/// Keep null Spotify slots and duplicate occurrences in the merge. They still
/// occupy server positions, even though the table does not draw null slots.
pub(crate) fn overlay_rows(
    page: &PlaylistPage,
    overlay: &Overlay,
) -> Vec<(TableItem, PlaylistRowOrigin)> {
    let tracks: Vec<_> = page
        .items
        .items
        .iter()
        .map(|row| match row.playable() {
            Some(PlayableItem::Track(track)) => track.clone(),
            Some(item) => Track {
                uri: item.uri().into(),
                ..Track::default()
            },
            None => Track::default(),
        })
        .collect();
    crate::playlist_overlay::merge(&tracks, overlay)
        .into_iter()
        .filter_map(|row| match row {
            MergedRow::Spotify { spotify_index, .. } => {
                let row = &page.items.items[spotify_index];
                let mut item = row.playable()?.clone();
                if let PlayableItem::Track(track) = &mut item {
                    track.is_local |= row.is_local;
                }
                Some((
                    (
                        item,
                        row.added_at.clone(),
                        row.added_by.as_ref().and_then(|user| user.id.clone()),
                    ),
                    PlaylistRowOrigin::Spotify(
                        page.items.base_offset.saturating_add(spotify_index as u32),
                    ),
                ))
            }
            MergedRow::Overlay { entry } => Some((
                (PlayableItem::Track(entry.to_track()), None, None),
                PlaylistRowOrigin::Overlay(entry.id),
            )),
        })
        .collect()
}

/// Convert a boundary in the drawn rows to the full merge, which also holds
/// unavailable Spotify slots. No unavailable slot becomes a local row index.
fn merge_boundary(page: &PlaylistPage, overlay: &Overlay, index: usize) -> usize {
    let rows = overlay_rows(page, overlay);
    let tracks: Vec<_> = page
        .items
        .items
        .iter()
        .map(|row| match row.playable() {
            Some(PlayableItem::Track(track)) => track.clone(),
            Some(item) => Track {
                uri: item.uri().into(),
                ..Default::default()
            },
            None => Track::default(),
        })
        .collect();
    let merged = crate::playlist_overlay::merge(&tracks, overlay);
    let Some((_, next)) = rows.get(index) else {
        return merged.len();
    };
    merged
        .iter()
        .position(|row| match (row, next) {
            (MergedRow::Spotify { spotify_index, .. }, PlaylistRowOrigin::Spotify(at)) => {
                page.items.base_offset + *spotify_index as u32 == *at
            }
            (MergedRow::Overlay { entry }, PlaylistRowOrigin::Overlay(id)) => entry.id == *id,
            _ => false,
        })
        .unwrap_or(merged.len())
}

impl App {
    pub(super) fn save_playlist_sources(&mut self) {
        self.playlist_sources_revision = self.playlist_sources_revision.wrapping_add(1);
        if self.offline {
            return;
        }
        if self.mixed_playlists_writable {
            match self.mixed_playlists.encode() {
                Ok(bytes) => self.files.write(
                    &self.dirs.mixed_playlists_file(),
                    bytes,
                    fastframe_file::Access::Private,
                ),
                Err(error) => self.toast_error(format!("Could not save mixed playlists: {error}")),
            }
        }
        if self.playlist_overlay_writable {
            match self.playlist_overlay.encode() {
                Ok(bytes) => self.files.write(
                    &self.dirs.playlist_overlay_file(),
                    bytes,
                    fastframe_file::Access::Private,
                ),
                Err(error) => {
                    self.toast_error(format!("Could not save playlist overlays: {error}"))
                }
            }
        }
    }

    pub(super) fn source_tracks(&self, id: &str) -> Vec<Track> {
        self.playlist_pages
            .get(id)
            .map(|page| {
                page.items
                    .items
                    .iter()
                    .map(|row| match row.playable() {
                        Some(PlayableItem::Track(track)) => track.clone(),
                        Some(item) => Track {
                            uri: item.uri().into(),
                            ..Track::default()
                        },
                        None => Track::default(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A mixed insertion is applied in one pass, preserving interleaved order.
    /// Spotify rows follow the existing optimistic mutation path. Local rows
    /// never enter Spotify's raw PagedList or its requests.
    pub(super) fn add_playlist_sources(
        &mut self,
        target: &str,
        name: &str,
        items: &[PlayableItem],
        position: Option<u32>,
    ) -> bool {
        if let Some(id) = target.strip_prefix(crate::mixed_playlists::PLAYLIST_PREFIX) {
            if !self.mixed_playlists_writable {
                self.toast_error(
                    "Mixed playlists could not be loaded; the original file has been kept",
                );
                return true;
            }
            let tracks: Vec<_> = items
                .iter()
                .filter_map(|item| match item {
                    PlayableItem::Track(track) => Some(track.clone()),
                    _ => None,
                })
                .collect();
            if let Some(playlist) = self.mixed_playlists.get_mut(id) {
                playlist.add_tracks(&tracks, position.map(|at| at as usize));
                self.save_playlist_sources();
            }
            return true;
        }
        let id = target.strip_prefix(OVERLAY_TARGET).unwrap_or(target);
        if !target.starts_with(OVERLAY_TARGET)
            && !items
                .iter()
                .any(|item| item.uri().starts_with("soundcloud:track:"))
        {
            return false;
        }
        if !self.playlist_overlay_writable {
            self.toast_error(
                "Playlist overlays could not be loaded; the original file has been kept",
            );
            return true;
        }
        let complete = self
            .playlist_pages
            .get(id)
            .is_some_and(|page| page.items.base_offset == 0 && page.items.is_complete());
        // Appends also work before a page has loaded. An absolute fallback
        // anchor attaches at the real end once its Spotify rows arrive.
        let total = self
            .playlist_pages
            .get(id)
            .and_then(|page| page.items.total)
            .or_else(|| self.known_playlist(id).map(Playlist::track_total))
            .unwrap_or(0);
        let tracks = self.source_tracks(id);
        let overlay = self.playlist_overlay.get(id).cloned().unwrap_or_default();
        let mut rows = crate::playlist_overlay::merge(&tracks, &overlay);
        let merged_at = position
            .map_or(rows.len(), |at| {
                if target.starts_with(OVERLAY_TARGET) {
                    self.playlist_pages.get(id).map_or(at as usize, |page| {
                        merge_boundary(page, &overlay, at as usize)
                    })
                } else {
                    at as usize
                }
            })
            .min(rows.len());
        let raw_at = if complete {
            rows.iter()
                .take(merged_at)
                .filter(|row| matches!(row, MergedRow::Spotify { .. }))
                .count() as u32
        } else {
            position.unwrap_or(total)
        };
        let spotify: Vec<_> = items
            .iter()
            .filter(|item| !item.uri().starts_with("soundcloud:track:"))
            .cloned()
            .collect();
        let spotify_count = spotify.len();
        let name = if name.is_empty() {
            self.known_playlist(id)
                .map(|playlist| playlist.name.clone())
                .unwrap_or_default()
        } else {
            name.to_string()
        };
        if !spotify.is_empty() {
            self.add_to_playlist_now(id.into(), name, spotify, Some(raw_at));
        }
        if complete {
            // Describe the requested visual order first. Then checkpoint its
            // overlay anchors against the optimistic raw Spotify insertion.
            // This keeps a drop BEFORE an existing local row before that row.
            for row in &mut rows {
                if let MergedRow::Spotify { spotify_index, .. } = row
                    && *spotify_index >= raw_at as usize
                {
                    *spotify_index += spotify_count;
                }
            }
            let mut next = raw_at as usize;
            let inserted = items.iter().map(|item| {
                if let PlayableItem::Track(track) = item
                    && crate::soundcloud::is_track_uri(&track.uri)
                {
                    MergedRow::Overlay {
                        entry: crate::mixed_playlists::Entry::new(track),
                    }
                } else {
                    let track = match item {
                        PlayableItem::Track(track) => track.clone(),
                        _ => Track {
                            uri: item.uri().into(),
                            ..Default::default()
                        },
                    };
                    let row = MergedRow::Spotify {
                        track,
                        spotify_index: next,
                    };
                    next += 1;
                    row
                }
            });
            rows.splice(merged_at..merged_at, inserted);
            let tracks = self.source_tracks(id);
            self.playlist_overlay.get_mut(id).checkpoint(&tracks, rows);
        } else {
            let mut boundary = raw_at as usize;
            for item in items {
                if let PlayableItem::Track(track) = item
                    && crate::soundcloud::is_track_uri(&track.uri)
                {
                    self.playlist_overlay
                        .get_mut(id)
                        .insert_at_position(boundary, track);
                } else {
                    boundary += 1;
                }
            }
        }
        self.save_playlist_sources();
        true
    }

    pub(super) fn apply_playlist_source_action(
        &mut self,
        action: Action,
        ctx: &egui::Context,
    ) -> Option<Action> {
        // Failed reads leave the original file intact. Never accept an edit
        // that could only exist in memory and disappear on the next start.
        let mixed_edit = match &action {
            Action::SaveMixedPlaylist { .. }
            | Action::DuplicateMixedPlaylist(_)
            | Action::ImportMixedPlaylist(_) => true,
            Action::DeletePlaylist(id)
            | Action::RemoveFromPlaylist {
                playlist_id: id, ..
            }
            | Action::MoveInPlaylist {
                playlist_id: id, ..
            } => id.starts_with(crate::mixed_playlists::PLAYLIST_PREFIX),
            _ => false,
        };
        if mixed_edit && !self.mixed_playlists_writable {
            self.toast_error(
                "Mixed playlists could not be loaded; the original file has been kept",
            );
            return None;
        }
        match action {
            Action::CopySongs(items)
                if items
                    .iter()
                    .any(|item| item.uri().starts_with("soundcloud:track:")) =>
            {
                let links: Vec<_> = items
                    .iter()
                    .filter_map(|item| {
                        if item.uri().starts_with("soundcloud:track:") {
                            Some(item.uri().to_string())
                        } else {
                            util::open_spotify_url(item.uri())
                        }
                    })
                    .collect();
                ctx.copy_text(links.join(if cfg!(windows) { "\r\n" } else { "\n" }));
                self.copied_songs = items;
                self.toast(gettext(self.locale, "Link copied"));
            }
            Action::SaveMixedPlaylist {
                id,
                name,
                description,
                tracks,
            } => {
                if !self.mixed_playlists_writable {
                    self.toast_error(
                        "Mixed playlists could not be loaded; the original file has been kept",
                    );
                    return None;
                }
                let id = if let Some(id) = id {
                    if let Some(playlist) = self.mixed_playlists.get_mut(&id) {
                        playlist.rename(name);
                        playlist.set_description(description);
                    }
                    id
                } else {
                    self.mixed_playlists.import(name, description, tracks)
                };
                self.dialog = None;
                self.save_playlist_sources();
                self.open(Page::MixedPlaylist(id));
            }
            Action::DuplicateMixedPlaylist(id) => {
                if let Some(playlist) = self.mixed_playlists.get(&id)
                    && let Some(copy) = self
                        .mixed_playlists
                        .duplicate(&id, format!("{} (copy)", playlist.name))
                {
                    self.save_playlist_sources();
                    self.open(Page::MixedPlaylist(copy));
                }
            }
            Action::ImportMixedPlaylist(id) => {
                if let Some(page) = self.playlist_pages.get(&id) {
                    if !page.items.is_complete() || page.items.base_offset != 0 {
                        self.pending_mixed_imports.insert(id.clone());
                        self.load_more(Page::Playlist(id));
                        return None;
                    }
                    if let Some(playlist) = page.playlist.get() {
                        let tracks = if let Some(overlay) = self.playlist_overlay.get(&id) {
                            overlay_rows(page, overlay)
                                .into_iter()
                                .filter_map(|((item, _, _), _)| match item {
                                    PlayableItem::Track(track) => Some(track),
                                    _ => None,
                                })
                                .collect()
                        } else {
                            page.items
                                .items
                                .iter()
                                .filter_map(|row| match row.playable() {
                                    Some(PlayableItem::Track(track)) => Some(track.clone()),
                                    _ => None,
                                })
                                .collect()
                        };
                        self.dialog = Some(Dialog::MixedPlaylist {
                            id: None,
                            name: playlist.name.clone(),
                            description: playlist.description.clone().unwrap_or_default(),
                            tracks,
                        });
                    }
                }
            }
            Action::DeletePlaylist(uri)
                if uri.starts_with(crate::mixed_playlists::PLAYLIST_PREFIX) =>
            {
                let id = uri
                    .strip_prefix(crate::mixed_playlists::PLAYLIST_PREFIX)
                    .unwrap();
                self.mixed_playlists.delete(id);
                self.mixed_filters.remove(id);
                self.dialog = None;
                self.save_playlist_sources();
                if matches!(self.page(), Page::MixedPlaylist(current) if current == id) {
                    self.open(Page::Home);
                }
            }
            Action::InsertInPlaylist {
                playlist_id,
                position,
                items,
            } if playlist_id.starts_with(crate::mixed_playlists::PLAYLIST_PREFIX) => {
                self.add_playlist_sources(&playlist_id, "", &items, Some(position));
            }
            Action::RemoveFromPlaylist {
                playlist_id,
                positions,
                ..
            } if playlist_id.starts_with(crate::mixed_playlists::PLAYLIST_PREFIX) => {
                if let Some(playlist) = self.mixed_playlists.get_mut(
                    playlist_id
                        .strip_prefix(crate::mixed_playlists::PLAYLIST_PREFIX)
                        .unwrap(),
                ) {
                    let ids: Vec<_> = positions
                        .iter()
                        .filter_map(|at| {
                            playlist
                                .entries
                                .get(*at as usize)
                                .map(|entry| entry.id.clone())
                        })
                        .collect();
                    playlist.remove_entries(&ids);
                    self.save_playlist_sources();
                }
            }
            Action::MoveInPlaylist {
                playlist_id,
                from,
                to,
            } if playlist_id.starts_with(crate::mixed_playlists::PLAYLIST_PREFIX) => {
                if let Some(playlist) = self.mixed_playlists.get_mut(
                    playlist_id
                        .strip_prefix(crate::mixed_playlists::PLAYLIST_PREFIX)
                        .unwrap(),
                ) && let Some(entry) = playlist.entries.get(from as usize)
                {
                    let entry_id = entry.id.clone();
                    playlist.move_entries(std::slice::from_ref(&entry_id), to as usize);
                    self.save_playlist_sources();
                }
            }
            Action::RemoveFromPlaylist {
                playlist_id,
                positions,
                ..
            } if playlist_id.starts_with(OVERLAY_TARGET) => {
                let id = playlist_id.strip_prefix(OVERLAY_TARGET).unwrap();
                let page = self.playlist_pages.get(id)?;
                if !page.items.is_complete() {
                    return None;
                }
                let overlay = self.playlist_overlay.get(id)?;
                let rows = overlay_rows(page, overlay);
                let mut spotify = Vec::new();
                let mut uris = Vec::new();
                let mut local = Vec::new();
                for at in positions {
                    if let Some(((item, _, _), origin)) = rows.get(at as usize) {
                        match origin {
                            PlaylistRowOrigin::Spotify(at) => {
                                spotify.push(*at);
                                uris.push(item.uri().to_string());
                            }
                            PlaylistRowOrigin::Overlay(entry) => local.push(entry.clone()),
                        }
                    }
                }
                for entry in local {
                    self.playlist_overlay.get_mut(id).remove(&entry);
                }
                self.save_playlist_sources();
                if !spotify.is_empty() {
                    self.apply(
                        Action::RemoveFromPlaylist {
                            playlist_id: id.into(),
                            positions: spotify,
                            uris,
                        },
                        ctx,
                    );
                }
            }
            Action::MoveInPlaylist {
                playlist_id,
                from,
                to,
            } if playlist_id.starts_with(OVERLAY_TARGET) => {
                let id = playlist_id.strip_prefix(OVERLAY_TARGET).unwrap();
                let page = self.playlist_pages.get(id)?;
                if !page.items.is_complete() {
                    return None;
                }
                let overlay = self.playlist_overlay.get(id)?;
                let rows = overlay_rows(page, overlay);
                match rows.get(from as usize).map(|row| row.1.clone()) {
                    Some(PlaylistRowOrigin::Overlay(entry)) => {
                        let boundary = merge_boundary(page, overlay, to as usize);
                        let tracks = self.source_tracks(id);
                        self.playlist_overlay
                            .get_mut(id)
                            .move_entry(&tracks, &entry, boundary);
                        self.save_playlist_sources();
                    }
                    Some(PlaylistRowOrigin::Spotify(from)) => {
                        let mut tracks = self.source_tracks(id);
                        let mut merged = crate::playlist_overlay::merge(&tracks, overlay);
                        let boundary = merge_boundary(page, overlay, to as usize);
                        let raw_to = merged
                            .iter()
                            .take(boundary)
                            .filter(|row| matches!(row, MergedRow::Spotify { .. }))
                            .count();
                        let source = merged.iter().position(|row| matches!(row, MergedRow::Spotify { spotify_index, .. } if *spotify_index == from as usize))?;
                        let moved = merged.remove(source);
                        let destination = boundary
                            .saturating_sub(usize::from(source < boundary))
                            .min(merged.len());
                        merged.insert(destination, moved);
                        let moved = tracks.remove(from as usize);
                        let destination = raw_to
                            .saturating_sub(usize::from((from as usize) < raw_to))
                            .min(tracks.len());
                        tracks.insert(destination, moved);
                        let mut index = 0;
                        for row in &mut merged {
                            if let MergedRow::Spotify { spotify_index, .. } = row {
                                *spotify_index = index;
                                index += 1;
                            }
                        }
                        // A Spotify row can cross only local rows without a
                        // Spotify reorder. Re-anchor those rows so the visual
                        // drop still takes effect immediately and exactly.
                        self.playlist_overlay
                            .get_mut(id)
                            .checkpoint(&tracks, merged);
                        self.save_playlist_sources();
                        if raw_to == from as usize || raw_to == from as usize + 1 {
                            return None;
                        }
                        return Some(Action::MoveInPlaylist {
                            playlist_id: id.into(),
                            from,
                            to: raw_to as u32,
                        });
                    }
                    None => {}
                }
            }
            Action::InsertInPlaylist {
                playlist_id,
                position,
                items,
            } if playlist_id.starts_with(OVERLAY_TARGET) => {
                let id = playlist_id.strip_prefix(OVERLAY_TARGET).unwrap();
                if !self
                    .playlist_pages
                    .get(id)
                    .is_some_and(|page| page.items.is_complete())
                {
                    return None;
                }
                self.add_playlist_sources(&playlist_id, "", &items, Some(position));
            }
            other => return Some(other),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(uri: &str) -> Track {
        Track {
            uri: uri.into(),
            name: uri.into(),
            ..Default::default()
        }
    }
    fn spotify(ids: &[&str]) -> PlaylistPage {
        PlaylistPage {
            playlist: Loadable::Loaded(Playlist {
                id: "p".into(),
                uri: "spotify:playlist:p".into(),
                name: "Playlist".into(),
                ..Default::default()
            }),
            items: PagedList {
                items: ids
                    .iter()
                    .map(|id| PlaylistItem {
                        item: Some(PlayableItem::Track(track(&format!("spotify:track:{id}")))),
                        ..Default::default()
                    })
                    .collect(),
                total: Some(ids.len() as u32),
                next_offset: None,
                loaded_once: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    fn app(name: &str) -> App {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".cache")
            .join(format!(
                "playlist-ui-{name}-{}",
                crate::mixed_playlists::new_id()
            ));
        App::new(
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
        )
    }
    fn cleanup(mut app: App) {
        app.backend.shutdown();
        assert!(app.files.flush(Duration::from_secs(10)));
        let root = app.dirs.state.parent().unwrap().to_path_buf();
        drop(app);
        if root.exists() {
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn interleaved_additions_keep_order_without_sending_soundcloud_to_spotify() {
        let mut app = app("split");
        app.playlist_pages.insert("p".into(), spotify(&["a"]));
        let incoming = vec![
            PlayableItem::Track(track("soundcloud:track:1")),
            PlayableItem::Track(track("spotify:track:b")),
            PlayableItem::Track(track("soundcloud:track:2")),
            PlayableItem::Track(track("spotify:track:c")),
        ];
        app.request_playlist_add("p".into(), "Playlist".into(), incoming.clone(), None);
        let page = &app.playlist_pages["p"];
        assert_eq!(
            page.items
                .items
                .iter()
                .filter_map(PlaylistItem::playable)
                .map(PlayableItem::uri)
                .collect::<Vec<_>>(),
            ["spotify:track:a", "spotify:track:b", "spotify:track:c"]
        );
        let rows = overlay_rows(page, app.playlist_overlay.get("p").unwrap());
        assert_eq!(
            rows.iter().map(|row| row.0.0.uri()).collect::<Vec<_>>(),
            [
                "spotify:track:a",
                "soundcloud:track:1",
                "spotify:track:b",
                "soundcloud:track:2",
                "spotify:track:c"
            ]
        );
        let sent = app.backend.take_playlist_add_requests();
        assert_eq!(sent.len(), 1);
        assert!(sent.iter().all(|request| matches!(request, ApiRequest::AddToPlaylist { uris, .. } if uris.iter().all(|uri| uri.starts_with("spotify:")))));
        let id = app.mixed_playlists.create("Mix".into(), String::new());
        app.request_playlist_add(
            format!("spotifast:playlist:{id}"),
            "Mix".into(),
            incoming.clone(),
            None,
        );
        assert_eq!(
            app.mixed_playlists
                .get(&id)
                .unwrap()
                .to_tracks()
                .iter()
                .map(|track| track.uri.as_str())
                .collect::<Vec<_>>(),
            incoming.iter().map(PlayableItem::uri).collect::<Vec<_>>()
        );
        assert!(app.backend.take_playlist_add_requests().is_empty());
        cleanup(app);
    }

    #[test]
    fn derived_origins_preserve_window_offsets_null_slots_duplicates_and_removed_anchors() {
        let mut page = spotify(&["a", "a", "b"]);
        let mut overlay = Overlay::default();
        overlay.insert(
            &[
                track("spotify:track:a"),
                track("spotify:track:a"),
                track("spotify:track:b"),
            ],
            2,
            &track("soundcloud:track:1"),
        );
        page.items.items.insert(1, PlaylistItem::default());
        page.items.base_offset = 500;
        let rows = overlay_rows(&page, &overlay);
        assert_eq!(
            rows.iter()
                .filter_map(|row| match row.1 {
                    PlaylistRowOrigin::Spotify(at) => Some(at),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            [500, 502, 503]
        );
        assert!(matches!(rows[2].1, PlaylistRowOrigin::Overlay(_)));
        page.items.items.remove(2);
        let rows = overlay_rows(&page, &overlay);
        assert_eq!(
            rows.iter().map(|row| row.0.0.uri()).collect::<Vec<_>>(),
            ["spotify:track:a", "soundcloud:track:1", "spotify:track:b"]
        );
        assert_eq!(rows[2].1, PlaylistRowOrigin::Spotify(502));
    }

    #[test]
    fn inserting_a_selection_before_existing_overlay_rows_preserves_both_orders() {
        let mut app = app("drop-order");
        app.playlist_pages.insert("p".into(), spotify(&["a", "b"]));
        let tracks = app.source_tracks("p");
        app.playlist_overlay
            .get_mut("p")
            .insert(&tracks, 1, &track("soundcloud:track:1"));
        let items = vec![
            PlayableItem::Track(track("spotify:track:x")),
            PlayableItem::Track(track("soundcloud:track:2")),
            PlayableItem::Track(track("spotify:track:y")),
        ];
        app.apply(
            Action::InsertInPlaylist {
                playlist_id: format!("{OVERLAY_TARGET}p"),
                position: 1,
                items,
            },
            &egui::Context::default(),
        );
        let rows = overlay_rows(
            &app.playlist_pages["p"],
            app.playlist_overlay.get("p").unwrap(),
        );
        assert_eq!(
            rows.iter().map(|row| row.0.0.uri()).collect::<Vec<_>>(),
            [
                "spotify:track:a",
                "spotify:track:x",
                "soundcloud:track:2",
                "spotify:track:y",
                "soundcloud:track:1",
                "spotify:track:b"
            ]
        );
        let sent = app.backend.take_playlist_add_requests();
        assert!(
            matches!(sent.as_slice(), [ApiRequest::AddToPlaylist { uris, position: Some(1), .. }] if uris == &["spotify:track:x", "spotify:track:y"])
        );
        cleanup(app);
    }

    #[test]
    fn a_windowed_derived_edit_waits_for_a_full_spotify_snapshot() {
        let mut app = app("window-guard");
        let mut page = spotify(&["a", "a"]);
        page.items.base_offset = 500;
        app.playlist_pages.insert("p".into(), page);
        let tracks = app.source_tracks("p");
        app.playlist_overlay
            .get_mut("p")
            .insert(&tracks, 1, &track("soundcloud:track:1"));
        app.apply(
            Action::RemoveFromPlaylist {
                playlist_id: format!("{OVERLAY_TARGET}p"),
                uris: vec!["spotify:track:a".into()],
                positions: vec![2],
            },
            &egui::Context::default(),
        );
        assert_eq!(app.source_tracks("p").len(), 2);
        assert!(app.playlist_overlay.has_overlay("p"));
        cleanup(app);
    }

    #[test]
    fn removing_a_mixed_selection_maps_spotify_occurrence_and_overlay_identity() {
        let mut app = app("remove");
        app.playlist_pages
            .insert("p".into(), spotify(&["a", "a", "b"]));
        let tracks = app.source_tracks("p");
        app.playlist_overlay
            .get_mut("p")
            .insert(&tracks, 1, &track("soundcloud:track:1"));
        app.apply(
            Action::RemoveFromPlaylist {
                playlist_id: format!("{OVERLAY_TARGET}p"),
                uris: vec!["soundcloud:track:1".into(), "spotify:track:a".into()],
                positions: vec![1, 2],
            },
            &egui::Context::default(),
        );
        assert!(!app.playlist_overlay.has_overlay("p"));
        assert_eq!(
            app.source_tracks("p")
                .iter()
                .map(|track| track.uri.as_str())
                .collect::<Vec<_>>(),
            ["spotify:track:a", "spotify:track:b"]
        );
        cleanup(app);
    }

    #[test]
    fn overlay_and_spotify_moves_keep_their_own_coordinates_with_null_rows() {
        let mut app = app("moves");
        let mut page = spotify(&["a", "b", "c"]);
        page.items.items.insert(1, PlaylistItem::default());
        app.playlist_pages.insert("p".into(), page);
        let tracks = app.source_tracks("p");
        let local = app
            .playlist_overlay
            .get_mut("p")
            .insert(&tracks, 3, &track("soundcloud:track:1"))
            .unwrap();
        let ctx = egui::Context::default();
        // Drawn a, b, local, c. Move the local row before b (raw slot 2).
        app.apply(
            Action::MoveInPlaylist {
                playlist_id: format!("{OVERLAY_TARGET}p"),
                from: 2,
                to: 1,
            },
            &ctx,
        );
        let rows = overlay_rows(
            &app.playlist_pages["p"],
            app.playlist_overlay.get("p").unwrap(),
        );
        assert_eq!(rows[1].1, PlaylistRowOrigin::Overlay(local));
        // Drawn a, local, b, c. Moving c before b must send raw 3 -> 2.
        let mapped = app
            .apply_playlist_source_action(
                Action::MoveInPlaylist {
                    playlist_id: format!("{OVERLAY_TARGET}p"),
                    from: 3,
                    to: 2,
                },
                &ctx,
            )
            .unwrap();
        assert!(
            matches!(mapped, Action::MoveInPlaylist { playlist_id, from: 3, to: 2 } if playlist_id == "p")
        );
        cleanup(app);
    }

    #[test]
    fn a_spotify_row_can_cross_only_local_rows_without_a_spotify_write() {
        let mut app = app("cross-local");
        app.playlist_pages.insert("p".into(), spotify(&["a", "b"]));
        let tracks = app.source_tracks("p");
        app.playlist_overlay
            .get_mut("p")
            .insert(&tracks, 1, &track("soundcloud:track:1"));
        let action = app.apply_playlist_source_action(
            Action::MoveInPlaylist {
                playlist_id: format!("{OVERLAY_TARGET}p"),
                from: 0,
                to: 2,
            },
            &egui::Context::default(),
        );
        assert!(
            action.is_none(),
            "crossing local rows needs no Spotify reorder"
        );
        let rows = overlay_rows(
            &app.playlist_pages["p"],
            app.playlist_overlay.get("p").unwrap(),
        );
        assert_eq!(
            rows.iter().map(|row| row.0.0.uri()).collect::<Vec<_>>(),
            ["soundcloud:track:1", "spotify:track:a", "spotify:track:b"]
        );
        cleanup(app);
    }

    #[test]
    fn pasted_mixed_selection_keeps_sources_order_and_duplicate_identity() {
        let mut app = app("paste");
        let ctx = egui::Context::default();
        let songs = vec![
            PlayableItem::Track(track("soundcloud:track:1")),
            PlayableItem::Track(track("spotify:track:a")),
            PlayableItem::Track(track("soundcloud:track:1")),
        ];
        app.apply(Action::CopySongs(songs.clone()), &ctx);
        let id = app.mixed_playlists.create("Paste".into(), String::new());
        app.apply(
            Action::PasteSongs {
                playlist_id: format!("spotifast:playlist:{id}"),
                text: "soundcloud:track:1\nhttps://open.spotify.com/track/a\nsoundcloud:track:1"
                    .into(),
            },
            &ctx,
        );
        let playlist = app.mixed_playlists.get(&id).unwrap();
        assert_eq!(
            playlist
                .entries
                .iter()
                .map(|entry| entry.track.uri.as_str())
                .collect::<Vec<_>>(),
            songs.iter().map(PlayableItem::uri).collect::<Vec<_>>()
        );
        assert_ne!(playlist.entries[0].id, playlist.entries[2].id);
        cleanup(app);
    }

    #[test]
    fn source_actions_persist_through_the_app_writer_and_startup_loader() {
        let mut app = app("persist");
        app.apply(
            Action::SaveMixedPlaylist {
                id: None,
                name: "Across streams".into(),
                description: "Local".into(),
                tracks: vec![track("spotify:track:a"), track("soundcloud:track:1")],
            },
            &egui::Context::default(),
        );
        app.playlist_pages.insert("p".into(), spotify(&["a"]));
        app.request_playlist_add(
            "p".into(),
            "Playlist".into(),
            vec![PlayableItem::Track(track("soundcloud:track:2"))],
            None,
        );
        assert!(app.files.flush(Duration::from_secs(10)));
        let mixed = crate::mixed_playlists::Store::load(&app.dirs.mixed_playlists_file()).unwrap();
        let overlay =
            crate::playlist_overlay::Store::load(&app.dirs.playlist_overlay_file()).unwrap();
        assert_eq!(mixed, app.mixed_playlists);
        assert_eq!(overlay, app.playlist_overlay);
        let mut restored = App::new(
            &Waker::default(),
            app.dirs.clone(),
            Settings::default(),
            AppOptions {
                media_controls: false,
                restore_sign_in: false,
                tray: false,
                autostart: false,
            },
        );
        assert_eq!(restored.mixed_playlists, mixed);
        assert_eq!(restored.playlist_overlay, overlay);
        restored.backend.shutdown();
        drop(restored);
        cleanup(app);
    }
}
