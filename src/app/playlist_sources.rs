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

pub(super) struct PendingSoundCloudAdd {
    target: String,
    name: String,
    entries: Vec<String>,
}

impl App {
    fn soundcloud_add_entries(&mut self, target: &str) -> Vec<&mut crate::mixed_playlists::Entry> {
        if let Some(id) = target.strip_prefix(crate::mixed_playlists::PLAYLIST_PREFIX) {
            self.mixed_playlists
                .get_mut(id)
                .map(|playlist| playlist.entries.iter_mut().collect())
                .unwrap_or_default()
        } else {
            self.playlist_overlay
                .get_mut(target.strip_prefix(OVERLAY_TARGET).unwrap_or(target))
                .entries
                .iter_mut()
                .map(|row| &mut row.entry)
                .collect()
        }
    }

    pub(super) fn add_soundcloud_playlist(&mut self, uri: &str, target: String, name: String) {
        let Some(id) = crate::soundcloud::playlist_id(uri) else {
            return;
        };
        if let Some(id) = target.strip_prefix(crate::mixed_playlists::PLAYLIST_PREFIX) {
            if !self.mixed_playlists_writable || self.mixed_playlists.get(id).is_none() {
                self.toast_error(gettext(self.locale, "Playlist unavailable"));
                return;
            }
        } else if !self.playlist_overlay_writable {
            self.toast_error(gettext(
                self.locale,
                "Playlist additions could not be loaded. Your saved file has been kept.",
            ));
            return;
        }
        self.load_generation += 1;
        let request = self.load_generation;
        self.pending_soundcloud_adds.insert(
            request,
            PendingSoundCloudAdd {
                target,
                name,
                entries: Vec::new(),
            },
        );
        if let Some(content) = self
            .soundcloud_pages
            .get(uri)
            .and_then(|page| page.content.get())
            && content.next.is_none()
        {
            self.soundcloud_playlist_add_finished(request, true, Ok(content.tracks.clone()));
        } else {
            self.backend
                .send(Command::AddSoundCloudPlaylist { id, request });
            self.toast(gettext(self.locale, "Adding SoundCloud tracks…"));
        }
    }

    pub(super) fn soundcloud_playlist_add_finished(
        &mut self,
        request: u64,
        complete: bool,
        result: Result<Vec<Track>, String>,
    ) {
        let Some(mut pending) = self.pending_soundcloud_adds.remove(&request) else {
            return;
        };
        // A deleted destination must never be recreated by a late reply.
        if let Some(id) = pending
            .target
            .strip_prefix(crate::mixed_playlists::PLAYLIST_PREFIX)
            && self.mixed_playlists.get(id).is_none()
        {
            return;
        }
        match result {
            Ok(tracks) => {
                if pending.entries.is_empty() {
                    let before: HashSet<_> = self
                        .soundcloud_add_entries(&pending.target)
                        .iter()
                        .map(|entry| entry.id.clone())
                        .collect();
                    let items: Vec<_> = tracks.iter().cloned().map(PlayableItem::Track).collect();
                    // Register the provisional identities before any snapshot
                    // reaches the writer. An interrupted process cannot resume
                    // these metadata requests on its next start.
                    self.insert_playlist_sources(&pending.target, &pending.name, &items, None);
                    pending.entries = self
                        .soundcloud_add_entries(&pending.target)
                        .iter()
                        .filter(|entry| !before.contains(&entry.id))
                        .map(|entry| entry.id.clone())
                        .collect();
                } else {
                    let filled: HashMap<_, _> = pending.entries.iter().zip(&tracks).collect();
                    for entry in self.soundcloud_add_entries(&pending.target) {
                        if let Some(track) = filled.get(&entry.id) {
                            entry.track = (*track).clone();
                            entry.permalink.clone_from(&track.external_urls.spotify);
                        }
                    }
                }
                if complete {
                    self.toast(
                        ngettext(
                            self.locale,
                            // Translators: {count} songs were added to playlist {name}.
                            "Added {count} SoundCloud track to {name}",
                            "Added {count} SoundCloud tracks to {name}",
                            tracks.len().try_into().unwrap_or(u32::MAX),
                        )
                        .replace("{count}", &tracks.len().to_string())
                        .replace("{name}", &pending.name),
                    );
                } else {
                    self.pending_soundcloud_adds.insert(request, pending);
                }
                self.save_playlist_sources();
            }
            Err(error) => {
                if let Some(id) = pending
                    .target
                    .strip_prefix(crate::mixed_playlists::PLAYLIST_PREFIX)
                {
                    if let Some(playlist) = self.mixed_playlists.get_mut(id) {
                        playlist.remove_entries(&pending.entries);
                    }
                } else {
                    let overlay = self.playlist_overlay.get_mut(
                        pending
                            .target
                            .strip_prefix(OVERLAY_TARGET)
                            .unwrap_or(&pending.target),
                    );
                    let ids: HashSet<_> = pending.entries.iter().collect();
                    overlay.entries.retain(|row| !ids.contains(&row.entry.id));
                }
                self.save_playlist_sources();
                log::warn!("Could not add SoundCloud playlist: {error}");
                self.toast_error(gettext(
                    self.locale,
                    "Could not add the SoundCloud playlist. Try again.",
                ));
            }
        }
    }

    pub(super) fn cancel_soundcloud_playlist_adds(&mut self) {
        let requests: Vec<_> = self.pending_soundcloud_adds.keys().copied().collect();
        for request in requests {
            self.soundcloud_playlist_add_finished(
                request,
                true,
                Err("Connection changed. Please add the playlist again.".into()),
            );
        }
    }

    pub(super) fn save_playlist_sources(&mut self) {
        self.playlist_sources_revision = self.playlist_sources_revision.wrapping_add(1);
        if self.offline {
            return;
        }
        let provisional: HashSet<_> = self
            .pending_soundcloud_adds
            .values()
            .flat_map(|pending| pending.entries.iter())
            .collect();
        let mut mixed = std::borrow::Cow::Borrowed(&self.mixed_playlists);
        let mut overlays = std::borrow::Cow::Borrowed(&self.playlist_overlay);
        if !provisional.is_empty() {
            for playlist in &mut mixed.to_mut().playlists {
                playlist
                    .entries
                    .retain(|entry| !provisional.contains(&entry.id));
            }
            for overlay in overlays.to_mut().playlists.values_mut() {
                overlay
                    .entries
                    .retain(|row| !provisional.contains(&row.entry.id));
            }
        }
        let mixed = self.mixed_playlists_writable.then(|| mixed.encode());
        let overlays = self.playlist_overlay_writable.then(|| overlays.encode());
        if let Some(result) = mixed {
            match result {
                Ok(bytes) => self.files.write(
                    &self.dirs.mixed_playlists_file(),
                    bytes,
                    fastframe_file::Access::Private,
                ),
                Err(error) => {
                    log::warn!("Could not encode mixed playlists: {error}");
                    self.toast_error(gettext(
                        self.locale,
                        "Could not save mixed playlists. Try again.",
                    ));
                }
            }
        }
        if let Some(result) = overlays {
            match result {
                Ok(bytes) => self.files.write(
                    &self.dirs.playlist_overlay_file(),
                    bytes,
                    fastframe_file::Access::Private,
                ),
                Err(error) => {
                    log::warn!("Could not encode playlist additions: {error}");
                    self.toast_error(gettext(
                        self.locale,
                        "Could not save playlist additions. Try again.",
                    ));
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
        let handled = self.insert_playlist_sources(target, name, items, position);
        if handled {
            self.save_playlist_sources();
        }
        handled
    }

    // The caller commits persistence after recording any pending row IDs.
    fn insert_playlist_sources(
        &mut self,
        target: &str,
        name: &str,
        items: &[PlayableItem],
        position: Option<u32>,
    ) -> bool {
        if let Some(id) = target.strip_prefix(crate::mixed_playlists::PLAYLIST_PREFIX) {
            if !self.mixed_playlists_writable {
                self.toast_error(gettext(
                    self.locale,
                    "Mixed playlists could not be loaded. Your saved file has been kept.",
                ));
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
            self.toast_error(gettext(
                self.locale,
                "Playlist additions could not be loaded. Your saved file has been kept.",
            ));
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
            self.toast_error(gettext(
                self.locale,
                "Mixed playlists could not be loaded. Your saved file has been kept.",
            ));
            return None;
        }
        match action {
            Action::SaveMixedPlaylist {
                id,
                name,
                description,
                tracks,
            } => {
                if !self.mixed_playlists_writable {
                    self.toast_error(gettext(
                        self.locale,
                        "Mixed playlists could not be loaded. Your saved file has been kept.",
                    ));
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
                    && let Some(copy) = self.mixed_playlists.duplicate(
                        &id,
                        gettext(self.locale, "{name} (copy)").replace("{name}", &playlist.name),
                    )
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

    fn set_tracks(app: &mut App) -> Vec<Track> {
        let tracks = vec![
            track("soundcloud:track:1"),
            track("soundcloud:track:2"),
            track("soundcloud:track:1"),
        ];
        app.soundcloud_pages.insert(
            "soundcloud:playlist:7".into(),
            SoundCloudPage {
                content: Loadable::Loaded(SoundCloudCollection {
                    title: "A set".into(),
                    tracks: tracks.clone(),
                    next: None,
                }),
                ..Default::default()
            },
        );
        tracks
    }

    #[test]
    fn soundcloud_set_confirmation_translates_the_actual_song_count() {
        for (count, locale, expected) in [
            (
                1,
                crate::i18n::Locale::English,
                "Added 1 SoundCloud track to Destination",
            ),
            (
                2,
                crate::i18n::Locale::Dutch,
                "2 SoundCloud-nummers toegevoegd aan Destination",
            ),
        ] {
            let mut app = app("set-count");
            app.locale = locale;
            set_tracks(&mut app);
            app.soundcloud_pages
                .get_mut("soundcloud:playlist:7")
                .unwrap()
                .content
                .get_mut()
                .unwrap()
                .tracks
                .truncate(count);
            let id = app.mixed_playlists.create("Mix".into(), String::new());
            app.apply(
                Action::AddSoundCloudPlaylist {
                    uri: "soundcloud:playlist:7".into(),
                    playlist_id: format!("spotifast:playlist:{id}"),
                    playlist_name: "Destination".into(),
                },
                &egui::Context::default(),
            );
            assert_eq!(app.toasts.last().unwrap().message, expected);
            cleanup(app);
        }
    }

    #[test]
    fn soundcloud_set_adds_every_occurrence_immediately_and_persists_locally() {
        let mut app = app("set-add");
        app.playlist_pages.insert("p".into(), spotify(&["a"]));
        let tracks = set_tracks(&mut app);
        let mixed = app.mixed_playlists.create("Mix".into(), String::new());
        for target in ["p".to_string(), format!("spotifast:playlist:{mixed}")] {
            for _ in 0..2 {
                app.apply(
                    Action::AddSoundCloudPlaylist {
                        uri: "soundcloud:playlist:7".into(),
                        playlist_id: target.clone(),
                        playlist_name: "Destination".into(),
                    },
                    &egui::Context::default(),
                );
            }
            let entries = app.soundcloud_add_entries(&target);
            assert_eq!(
                entries.iter().map(|entry| &entry.track).collect::<Vec<_>>(),
                tracks.iter().cycle().take(6).collect::<Vec<_>>()
            );
            assert_eq!(
                entries
                    .iter()
                    .map(|entry| &entry.id)
                    .collect::<HashSet<_>>()
                    .len(),
                6
            );
        }
        assert!(app.pending_soundcloud_adds.is_empty());
        assert!(
            app.dialog.is_none(),
            "local single-track adds also allow duplicates"
        );
        assert!(app.backend.take_playlist_add_requests().is_empty());
        assert!(
            app.toasts
                .iter()
                .all(|toast| toast.message == "Added 3 SoundCloud tracks to Destination")
        );
        assert!(app.files.flush(Duration::from_secs(10)));
        assert_eq!(
            crate::playlist_overlay::Store::load(&app.dirs.playlist_overlay_file()).unwrap(),
            app.playlist_overlay
        );
        assert_eq!(
            crate::mixed_playlists::Store::load(&app.dirs.mixed_playlists_file()).unwrap(),
            app.mixed_playlists
        );
        cleanup(app);
    }

    #[test]
    fn soundcloud_set_hydrates_placeholders_by_entry_id_without_reviving_removed_rows() {
        let mut app = app("set-hydrate");
        app.playlist_pages.insert("p".into(), spotify(&["a"]));
        let mixed = app.mixed_playlists.create("Mix".into(), String::new());
        for (request, target) in [
            (1, "p".to_string()),
            (2, format!("spotifast:playlist:{mixed}")),
        ] {
            let mut filled = vec![
                track("soundcloud:track:1"),
                track("soundcloud:track:2"),
                track("soundcloud:track:1"),
            ];
            filled[0].external_urls.spotify = Some("https://soundcloud.com/u/one".into());
            let placeholders = filled
                .iter()
                .map(|track| Track {
                    uri: track.uri.clone(),
                    name: "Loading".into(),
                    is_playable: Some(false),
                    ..Default::default()
                })
                .collect();
            app.pending_soundcloud_adds.insert(
                request,
                PendingSoundCloudAdd {
                    target: target.clone(),
                    name: "Target".into(),
                    entries: Vec::new(),
                },
            );
            app.handle_backend_events(vec![Event::SoundCloudPlaylistAdd {
                request,
                complete: false,
                result: Ok(placeholders),
            }]);
            let ids = app.pending_soundcloud_adds[&request].entries.clone();
            assert_eq!(ids.len(), 3);
            assert!(
                app.soundcloud_add_entries(&target)
                    .iter()
                    .all(|entry| entry.track.name == "Loading")
            );
            if target == "p" {
                app.playlist_overlay.get_mut("p").remove(&ids[1]);
                let source = app.source_tracks("p");
                app.playlist_overlay
                    .get_mut("p")
                    .move_entry(&source, &ids[2], 0);
            } else {
                let playlist = app.mixed_playlists.get_mut(&mixed).unwrap();
                playlist.remove_entries(&[ids[1].clone()]);
                playlist.move_entries(&[ids[2].clone()], 0);
            }
            app.soundcloud_playlist_add_finished(request, true, Ok(filled.clone()));
            let entries = app.soundcloud_add_entries(&target);
            assert_eq!(
                entries.iter().map(|entry| &entry.id).collect::<Vec<_>>(),
                [&ids[2], &ids[0]]
            );
            assert_eq!(entries[1].to_track(), filled[0]);
            assert_eq!(entries[0].to_track(), filled[2]);
            assert!(!app.pending_soundcloud_adds.contains_key(&request));
        }
        cleanup(app);
    }

    #[test]
    fn soundcloud_set_failure_or_connection_change_removes_only_its_provisional_rows() {
        let mut app = app("set-fail");
        for (request, fail) in [(1, true), (2, false)] {
            app.pending_soundcloud_adds.insert(
                request,
                PendingSoundCloudAdd {
                    target: "p".into(),
                    name: "Target".into(),
                    entries: Vec::new(),
                },
            );
            app.soundcloud_playlist_add_finished(
                request,
                false,
                Ok(vec![track("soundcloud:track:1")]),
            );
            app.add_playlist_sources(
                "p",
                "Target",
                &[PlayableItem::Track(track("soundcloud:track:2"))],
                None,
            );
            if fail {
                app.soundcloud_playlist_add_finished(request, true, Err("Missing track".into()));
            } else {
                app.cancel_soundcloud_playlist_adds();
            }
            app.soundcloud_playlist_add_finished(
                request,
                true,
                Ok(vec![track("soundcloud:track:1")]),
            );
            assert!(
                app.soundcloud_add_entries("p")
                    .iter()
                    .all(|entry| entry.track.uri == "soundcloud:track:2")
            );
        }
        assert_eq!(app.soundcloud_add_entries("p").len(), 2);
        cleanup(app);
    }

    #[test]
    fn soundcloud_set_persists_only_completed_copies_during_overlapping_loads() {
        for mixed in [false, true] {
            let mut app = app("set-pending-persistence");
            app.backend.set_offline(true);
            app.playlist_pages.insert("p".into(), spotify(&["a"]));
            let id = app.mixed_playlists.create("Mix".into(), String::new());
            let target = if mixed {
                format!("spotifast:playlist:{id}")
            } else {
                "p".into()
            };
            let existing = track("soundcloud:track:9");
            app.add_playlist_sources(
                &target,
                "Target",
                &[PlayableItem::Track(existing.clone())],
                None,
            );
            let mut requests = Vec::new();
            let filled = vec![track("soundcloud:track:1"), track("soundcloud:track:1")];
            let provisional: Vec<_> = filled
                .iter()
                .map(|track| Track {
                    uri: track.uri.clone(),
                    name: "Loading SoundCloud track…".into(),
                    is_playable: Some(false),
                    ..Default::default()
                })
                .collect();
            for _ in 0..2 {
                app.add_soundcloud_playlist(
                    "soundcloud:playlist:7",
                    target.clone(),
                    "Target".into(),
                );
                let request = app.load_generation;
                requests.push(request);
                app.soundcloud_playlist_add_finished(request, false, Ok(provisional.clone()));
            }
            // Navigation and page eviction do not own a copy's lifetime.
            app.open(Page::Search);
            app.soundcloud_pages.clear();
            app.playlist_pages.clear();
            assert_eq!(app.soundcloud_add_entries(&target).len(), 5);
            // An unrelated edit must not accidentally persist either request.
            app.add_playlist_sources(
                &target,
                "Target",
                &[PlayableItem::Track(existing.clone())],
                None,
            );
            let saved_tracks = |app: &App| {
                assert!(app.files.flush(Duration::from_secs(10)));
                if mixed {
                    crate::mixed_playlists::Store::load(&app.dirs.mixed_playlists_file())
                        .unwrap()
                        .get(&id)
                        .unwrap()
                        .to_tracks()
                } else {
                    crate::playlist_overlay::Store::load(&app.dirs.playlist_overlay_file())
                        .unwrap()
                        .get("p")
                        .unwrap()
                        .entries
                        .iter()
                        .map(|row| row.entry.to_track())
                        .collect()
                }
            };
            assert_eq!(saved_tracks(&app), [existing.clone(), existing.clone()]);
            // Complete the second request first. It becomes durable without
            // including the first request's still-visible provisional rows.
            let second_ids = app.pending_soundcloud_adds[&requests[1]].entries.clone();
            app.soundcloud_playlist_add_finished(requests[1], true, Ok(filled.clone()));
            assert_eq!(
                saved_tracks(&app),
                [
                    existing.clone(),
                    filled[0].clone(),
                    filled[1].clone(),
                    existing.clone()
                ]
            );
            app.soundcloud_playlist_add_finished(requests[0], true, Err("Missing track".into()));
            // Neither failure cleanup nor an obsolete reply can remove or
            // duplicate the other copy, even though every URI is the same.
            app.soundcloud_playlist_add_finished(requests[0], true, Ok(filled));
            let entries = app.soundcloud_add_entries(&target);
            assert_eq!(entries.len(), 4);
            assert_eq!(entries[1].id, second_ids[0]);
            assert_eq!(entries[2].id, second_ids[1]);
            let tracks: Vec<_> = entries.iter().map(|entry| entry.to_track()).collect();
            assert_eq!(saved_tracks(&app), tracks);
            cleanup(app);
        }
    }

    #[test]
    fn soundcloud_set_playback_uses_the_complete_hybrid_context_including_shuffle() {
        for shuffle in [false, true] {
            let mut app = app("set-play");
            app.backend.set_offline(true);
            app.playlist_pages.insert("p".into(), spotify(&["a", "b"]));
            let tracks = set_tracks(&mut app);
            let mixed = app.mixed_playlists.create("Mix".into(), String::new());
            for (target, uri) in [
                ("p".to_string(), "spotify:playlist:p".to_string()),
                (
                    format!("spotifast:playlist:{mixed}"),
                    format!("spotifast:playlist:{mixed}"),
                ),
            ] {
                app.add_soundcloud_playlist("soundcloud:playlist:7", target, "Target".into());
                app.shuffle_wanted = shuffle;
                app.apply(
                    Action::PlayContext {
                        uri: uri.clone(),
                        offset_uri: None,
                        offset_index: None,
                    },
                    &egui::Context::default(),
                );
                assert!(app.hybrid_owned());
                let snapshot = app.hybrid_snapshot().unwrap();
                let queue = crate::hybrid_queue::HybridQueue::restore(snapshot).unwrap();
                assert_eq!(queue.context().unwrap().uri, uri);
                assert_eq!(queue.shuffle(), shuffle);
                let mut played = vec![app.current_track_uri().unwrap()];
                while !app.queue.get().unwrap().queue.is_empty() {
                    app.apply(Action::Next, &egui::Context::default());
                    played.push(app.current_track_uri().unwrap());
                }
                let mut expected: Vec<_> = tracks.iter().map(|track| track.uri.clone()).collect();
                if uri.starts_with("spotify:") {
                    expected.splice(0..0, ["spotify:track:a".into(), "spotify:track:b".into()]);
                }
                if shuffle {
                    played.sort();
                    expected.sort();
                }
                assert_eq!(played, expected);
            }
            cleanup(app);
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
        let mut soundcloud = track("soundcloud:track:1");
        soundcloud.external_urls.spotify = Some("https://soundcloud.com/u/song".into());
        let songs = vec![
            PlayableItem::Track(soundcloud.clone()),
            PlayableItem::Track(track("spotify:track:a")),
            PlayableItem::Track(soundcloud),
        ];
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.apply(Action::CopySongs(songs.clone()), ui.ctx());
        });
        output.textures_delta.clear();
        let text = output
            .platform_output
            .commands
            .iter()
            .find_map(|command| {
                if let egui::OutputCommand::CopyText(text) = command {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .expect("copied public links");
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            [
                "https://soundcloud.com/u/song",
                "https://open.spotify.com/track/a",
                "https://soundcloud.com/u/song"
            ]
        );
        let id = app.mixed_playlists.create("Paste".into(), String::new());
        app.apply(
            Action::PasteSongs {
                playlist_id: format!("spotifast:playlist:{id}"),
                text,
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
        app.apply(
            Action::PasteSongs {
                playlist_id: format!("spotifast:playlist:{id}"),
                text: "soundcloud:track:1\nhttps://open.spotify.com/track/a\nsoundcloud:track:1"
                    .into(),
            },
            &ctx,
        );
        assert_eq!(app.mixed_playlists.get(&id).unwrap().entries.len(), 6);
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
