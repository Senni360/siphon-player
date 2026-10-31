//! One app-owned queue and one routing seam for all mixed-source controls.
use super::*;
use crate::backend::HybridCommand;
use crate::hybrid_queue::{Context, HybridQueue, Repeat, Snapshot, Transition};
use crate::player::{HybridEvent, PlaybackTag};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlaybackOwner {
    #[default]
    SpotifyConnect,
    Hybrid,
}

struct PendingStart {
    generation: u64,
    request: PlayRequest,
    shuffle: bool,
    repeat: RepeatMode,
    play: bool,
    navigation: Vec<bool>,
    context_navigation: Vec<bool>,
    resolving: bool,
    manual_started: bool,
}

/// A not-yet-resolved context is saved as intent, never as a truncated list.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PendingHybridSnapshot {
    context_uri: Option<String>,
    uris: Vec<String>,
    offset_uri: Option<String>,
    offset_position: Option<u32>,
    position_ms: u32,
    shuffle: bool,
    repeat: Repeat,
    navigation: Vec<bool>,
    #[serde(default)]
    manual_started: bool,
    #[serde(default)]
    context_navigation: Vec<bool>,
}

pub(super) struct HybridPlayback {
    queue: HybridQueue,
    generation: u64,
    plan_generation: u64,
    pending: Option<PendingStart>,
    tag: Option<PlaybackTag>,
    state: LocalState,
    loaded: bool,
    load_settled: bool,
    seek_pending: Option<PendingSeek>,
    // Child snapshots report observations, never the user's transport intent.
    play_intent: bool,
    transport_sequence: u64,
    playing_pending: Option<u64>,
    audible: bool,
    failed_occurrences: HashSet<crate::hybrid_queue::EntryId>,
    last_added: HashMap<String, Instant>,
}

#[derive(Clone, Copy)]
struct PendingSeek {
    position_ms: u32,
    sequence: u64,
    // While loading, the backend or librespot may fold the requested
    // position into Load rather than issue a numbered seek to a decoder.
    allow_load_position_ack: bool,
}

impl Default for HybridPlayback {
    fn default() -> Self {
        Self {
            queue: HybridQueue::new(rand::random()),
            generation: 0,
            plan_generation: 0,
            pending: None,
            tag: None,
            state: LocalState::default(),
            loaded: false,
            load_settled: false,
            seek_pending: None,
            playing_pending: None,
            play_intent: false,
            transport_sequence: 0,
            audible: false,
            failed_occurrences: HashSet::new(),
            last_added: HashMap::new(),
        }
    }
}

impl App {
    pub(super) fn hybrid_context_uri(&self) -> Option<String> {
        if let Some(pending) = &self.hybrid.pending {
            return pending.request.context_uri.clone();
        }
        self.hybrid
            .queue
            .context()
            .filter(|c| !c.uri.is_empty())
            .map(|c| c.uri.clone())
    }

    pub(super) fn hybrid_current_item(&self) -> Option<PlayableItem> {
        self.hybrid
            .queue
            .current()
            .map(|e| PlayableItem::Track(e.track.clone()))
    }

    pub(super) fn hybrid_playing_from(&self) -> Option<PlayingFrom> {
        let context = self.hybrid.queue.context()?;
        (!context.uri.is_empty()).then(|| PlayingFrom {
            name: context.name.clone(),
            page: Page::from_uri(&context.uri),
        })
    }

    pub(super) fn hybrid_needs_activity(&self) -> bool {
        self.hybrid_owned()
            && (self.hybrid.state.playback == Playback::Playing || self.hybrid.state.loading)
    }

    pub(super) fn hybrid_audible(&self) -> bool {
        self.hybrid.audible && self.hybrid.play_intent
    }

    pub(super) fn hybrid_row_is_played_copy(&self, context: &RowContext, index: usize) -> bool {
        let Some(current) = self.hybrid.queue.current() else {
            return false;
        };
        let crate::hybrid_queue::Origin::Context {
            index: current_index,
        } = current.origin
        else {
            return true;
        };
        match context {
            RowContext::View {
                uris, context_uri, ..
            } if self
                .hybrid
                .queue
                .context()
                .is_some_and(|c| c.uri == *context_uri) =>
            {
                // A remembered plan index belongs to the order at the click.
                // Once sorting moves that row, fall back to URI identity, as
                // the ordinary Spotify row-marker path does.
                uris.get(current_index)
                    .is_none_or(|uri| *uri != current.track.uri)
                    || current_index == index
            }
            RowContext::Context { uri, .. }
                if self.hybrid.queue.context().is_some_and(|c| c.uri == *uri) =>
            {
                let current_index = self
                    .hybrid
                    .pending
                    .as_ref()
                    .and_then(|p| p.request.offset_position)
                    .map_or(current_index, |i| i as usize);
                self.context_row_holds(uri, current_index as u32, &current.track.uri) == Some(false)
                    || current_index == index
            }
            RowContext::Uris(uris)
                if self
                    .hybrid
                    .queue
                    .context()
                    .is_some_and(|c| c.entries.iter().map(|e| &e.track.uri).eq(uris.iter())) =>
            {
                current_index == index
            }
            _ => true,
        }
    }

    pub(super) fn hybrid_play_pending(&self, key: &str) -> bool {
        self.hybrid.state.loading
            && (self
                .hybrid
                .pending
                .as_ref()
                .is_some_and(|p| p.request.context_uri.as_deref() == Some(key))
                || self
                    .hybrid
                    .queue
                    .current()
                    .is_some_and(|e| e.track.uri == key)
                || self.hybrid.queue.context().is_some_and(|c| c.uri == key))
    }
    /// Uses the same transport actions as Spotify. No source decision belongs
    /// in a view, media-key adapter or platform integration.
    pub(super) fn dispatch_hybrid_action(&mut self, action: &Action) -> bool {
        let start = match action {
            Action::Search(text) if crate::youtube::parse(text).is_some() => Some((
                PlayRequest::context(crate::youtube::parse(text).unwrap()),
                false,
                None,
            )),
            Action::Search(text) if crate::link::soundcloud_url(text).is_some() => Some((
                PlayRequest::context(crate::link::soundcloud_url(text).unwrap()),
                false,
                None,
            )),
            Action::PlayContext {
                uri,
                offset_uri,
                offset_index,
            } => Some((
                PlayRequest {
                    context_uri: Some(uri.clone()),
                    offset_uri: offset_uri.clone(),
                    offset_position: *offset_index,
                    ..PlayRequest::default()
                },
                false,
                None,
            )),
            Action::ShufflePlay(uri) => Some((PlayRequest::context(uri.clone()), true, None)),
            Action::PlayUris { uris, index } => Some((
                PlayRequest::tracks(uris.clone()).starting_at_index(*index),
                false,
                None,
            )),
            Action::PlaySearchMatch { uri, serial } if *serial == self.play_search_serial => {
                Some((PlayRequest::context(uri.clone()), false, None))
            }
            Action::PlayFromRow {
                context,
                uri,
                index,
            } => match context {
                RowContext::Queue if self.hybrid_owned() => {
                    if self.hybrid.pending.is_some() {
                        let manual = self.hybrid.queue.playing_next(usize::MAX);
                        if let Some(entry) = manual.get(*index as usize)
                            && entry.track.uri == *uri
                            && let Some(transition) = self.hybrid.queue.skip_to(entry.id)
                        {
                            let pending = self.hybrid.pending.as_mut().unwrap();
                            pending.manual_started = true;
                            pending.context_navigation.append(&mut pending.navigation);
                            let play = self.hybrid.play_intent;
                            self.hybrid_transition(transition, 0, play);
                            return true;
                        }
                        if let Some(pending) = &mut self.hybrid.pending {
                            pending
                                .navigation
                                .extend(std::iter::repeat_n(true, *index as usize + 1));
                            pending.request.position_ms = 0;
                        }
                        return true;
                    }
                    if let Some(entry) = self.hybrid_rows().get(*index as usize)
                        && entry.track.uri == *uri
                        && let Some(transition) = self.hybrid.queue.skip_to(entry.id)
                    {
                        self.hybrid_transition(transition, 0, true);
                    }
                    return true;
                }
                RowContext::Context {
                    uri: context_uri, ..
                } => Some((
                    PlayRequest::context(context_uri.clone())
                        .starting_at_index(*index)
                        .starting_at_uri(uri.clone()),
                    false,
                    None,
                )),
                RowContext::Uris(uris) => {
                    let index = if uris.get(*index as usize) == Some(uri) {
                        *index
                    } else {
                        uris.iter()
                            .position(|u| u == uri)
                            .map_or(*index, |i| i as u32)
                    };
                    Some((
                        PlayRequest::tracks(uris.to_vec()).starting_at_index(index),
                        false,
                        None,
                    ))
                }
                RowContext::View {
                    uris, context_uri, ..
                } => Some((
                    PlayRequest::tracks(uris.to_vec()).starting_at_index(*index),
                    false,
                    Some(context_uri.clone()),
                )),
                _ => None,
            },
            _ => None,
        };
        if let Some((request, shuffle, logical_context)) = start {
            return self.try_hybrid_start(request, shuffle, logical_context);
        }
        // Queue additions can promote a complete local Spotify context. A
        // remote queue must never be silently pulled onto this computer.
        let additions = match action {
            Action::AddToQueue { uri, label }
                if self.hybrid_owned() && uri.starts_with("spotify:album:") =>
            {
                self.queue_album(uri.clone(), label.clone());
                return true;
            }
            Action::AddToQueue { uri, label } => {
                Some((vec![self.hybrid_track(uri, Some(label))], None))
            }
            Action::QueueMany { songs } => Some((
                songs
                    .iter()
                    .map(|(uri, label)| self.hybrid_track(uri, Some(label)))
                    .collect(),
                None,
            )),
            Action::PlayNext { items } => Some((items.iter().map(as_track).collect(), Some(0))),
            Action::InsertInQueue { items, position } => {
                Some((items.iter().map(as_track).collect(), Some(*position)))
            }
            _ => None,
        };
        if let Some((tracks, position)) = additions {
            let foreign = tracks.iter().any(|t| crate::youtube::is_external(&t.uri));
            if !self.hybrid_owned() && !foreign {
                return false;
            }
            if !self.hybrid_owned() && !self.promote_hybrid() {
                return true;
            }
            self.hybrid_add(tracks, position);
            return true;
        }
        if !self.hybrid_owned() {
            return false;
        }
        match action {
            Action::TogglePlay => self.hybrid_toggle(),
            Action::Next => {
                self.hybrid_next();
            }
            Action::Previous => {
                let position = self.hybrid_position();
                if self.hybrid.pending.is_some()
                    && self.hybrid.tag.is_some()
                    && (position > RESTART_BEFORE_PREVIOUS
                        || self
                            .hybrid
                            .queue
                            .history()
                            .next_back()
                            .is_some_and(|entry| {
                                entry.origin == crate::hybrid_queue::Origin::Manual
                            }))
                {
                    // Known manual history does not depend on the unresolved
                    // context. Apply Previous now, including across sources.
                    if position > RESTART_BEFORE_PREVIOUS {
                        self.hybrid_seek(0);
                    } else {
                        let play = self.hybrid.play_intent;
                        let transition = self.hybrid.queue.previous(position);
                        self.hybrid_transition(transition, 0, play);
                    }
                } else if let Some(pending) = &mut self.hybrid.pending {
                    if position <= RESTART_BEFORE_PREVIOUS {
                        pending.navigation.push(false);
                    }
                    pending.request.position_ms = 0;
                } else {
                    let play = self.hybrid.play_intent;
                    let t = self.hybrid.queue.previous(position);
                    self.hybrid_transition(t, 0, play);
                }
            }
            Action::Seek(position) => self.hybrid_seek(*position),
            Action::SeekBy(delta) => {
                let duration = self
                    .hybrid
                    .queue
                    .current()
                    .map_or(0, |e| e.track.duration_ms);
                let duration = if duration == 0 { u32::MAX } else { duration };
                let position = (i64::from(self.hybrid_position()) + delta)
                    .clamp(0, i64::from(duration)) as u32;
                self.hybrid_seek(position);
            }
            Action::ToggleShuffle => self.hybrid_shuffle(!self.hybrid.queue.shuffle()),
            Action::SetShuffle(on) => self.hybrid_shuffle(*on),
            Action::CycleRepeat => self.hybrid_repeat(self.hybrid_repeat_mode().next()),
            Action::SetRepeat(mode) => self.hybrid_repeat(*mode),
            Action::ClearQueue => {
                self.hybrid.queue.clear_queue();
                self.hybrid.last_added.clear();
                self.pending_album_queues.clear();
                self.project_hybrid();
                self.preload_hybrid();
            }
            Action::MoveInQueue { from, to } => {
                let rows = self.hybrid.queue.playing_next(usize::MAX);
                if let Some(entry) = rows.get(*from) {
                    let id = entry.id;
                    self.hybrid
                        .queue
                        .move_entry(id, if to > from { to.saturating_sub(1) } else { *to });
                    self.project_hybrid();
                    self.preload_hybrid();
                }
            }
            Action::RemoveFromQueue { rows } => {
                let manual = self.hybrid.queue.playing_next(usize::MAX);
                let ids: Vec<_> = rows
                    .iter()
                    .filter_map(|(index, uri)| {
                        manual
                            .get(*index)
                            .filter(|e| e.track.uri == *uri)
                            .map(|e| e.id)
                    })
                    .collect();
                self.hybrid.queue.remove(&ids);
                self.project_hybrid();
                self.preload_hybrid();
            }
            Action::RefreshQueue => self.project_hybrid(),
            Action::Transfer(_) | Action::ActivateReceiver(_) => {
                self.toast(gettext(
                    self.locale,
                    "Mixed playback is available only on this computer",
                ));
            }
            // Volume and mute share the existing local mixer helpers below.
            Action::SetVolume(_)
            | Action::PreviewVolume(_)
            | Action::VolumeBy(_)
            | Action::ToggleMute => return false,
            _ => return false,
        }
        true
    }

    pub fn hybrid_owned(&self) -> bool {
        self.playback_owner == PlaybackOwner::Hybrid
    }

    pub(super) fn hybrid_pause_for_signout(&mut self) {
        self.hybrid.state.position_ms = self.hybrid_position();
        self.hybrid.state.position_at = None;
        self.hybrid.state.playback = Playback::Paused;
        self.hybrid.state.loading = false;
        self.hybrid.tag = None;
        self.hybrid.loaded = false;
        self.hybrid.play_intent = false;
        self.hybrid.audible = false;
        self.hybrid.pending = None;
        self.project_hybrid();
    }

    /// Catalogue UIs retain complete SoundCloud metadata by full URI, never
    /// by the numeric ID that could collide with another provider's ID.
    pub fn cache_soundcloud_tracks(&mut self, tracks: &[Track]) {
        for track in tracks
            .iter()
            .filter(|t| crate::youtube::is_external(&t.uri))
        {
            self.track_cache.insert(track.uri.clone(), track.clone());
            self.track_used.insert(track.uri.clone(), Instant::now());
        }
    }

    pub(super) fn hybrid_queue_tracks(&mut self, tracks: Vec<Track>) {
        self.hybrid_add(tracks, None);
    }

    pub(super) fn try_hybrid_start(
        &mut self,
        request: PlayRequest,
        shuffle_first: bool,
        logical_context: Option<String>,
    ) -> bool {
        let context_uri = logical_context
            .clone()
            .or_else(|| request.context_uri.clone())
            .unwrap_or_default();
        let special = context_uri.starts_with(crate::mixed_playlists::PLAYLIST_PREFIX)
            || crate::youtube::is_track_uri(&context_uri)
            || context_uri == "youtube:likes"
            || crate::soundcloud::is_uri(&context_uri)
            || crate::link::soundcloud_url(&context_uri).is_some()
            || context_uri
                .strip_prefix("spotify:playlist:")
                .is_some_and(|id| self.playlist_overlay.has_overlay(id));
        let foreign = request
            .uris
            .iter()
            .chain(request.offset_uri.iter())
            .any(|uri| crate::youtube::is_external(uri));
        // Keep manual occurrences across a new context, including when its
        // current row happens to be Spotify. Pure starts with no manual rows
        // return to the unchanged Connect path.
        let keep_manual = self.hybrid_owned() && !self.hybrid.queue.playing_next(1).is_empty();
        if !special && !foreign && !keep_manual {
            if self.hybrid_owned() {
                self.leave_hybrid();
            }
            return false;
        }
        // External links and submitted search links reach this seam without
        // the ordinary PlayContext action's manual-song bookkeeping.
        self.leave_repeat_one();
        self.supersede_play_search();
        let repeat = if self.hybrid_owned() {
            self.hybrid_repeat_mode()
        } else {
            self.now_playing().map_or(self.repeat_wanted, |n| n.repeat)
        };
        let shuffle = shuffle_first || self.shuffle_wanted;
        if !self.hybrid_owned() {
            let manual: Vec<_> = self
                .manual_queue
                .iter()
                .map(|uri| self.hybrid_track(uri, None))
                .collect();
            self.hybrid.queue.add_to_queue(manual);
        }
        self.enter_hybrid();
        self.hybrid.plan_generation = self.hybrid.plan_generation.wrapping_add(1);
        self.hybrid.pending = None;
        self.hybrid.failed_occurrences.clear();
        self.hybrid.play_intent = true;
        self.hybrid.audible = false;
        self.shuffle_wanted = shuffle;
        self.repeat_wanted = repeat;
        let loaded_context = self.loaded_hybrid_context(&context_uri);
        let context = if !request.uris.is_empty() {
            let metadata: HashMap<_, _> = loaded_context
                .as_ref()
                .into_iter()
                .flat_map(|c| &c.tracks)
                .map(|t| (t.uri.as_str(), t))
                .collect();
            Some(Context {
                uri: context_uri.clone(),
                name: self.hybrid_context_name(&context_uri),
                tracks: request
                    .uris
                    .iter()
                    .map(|uri| {
                        metadata
                            .get(uri.as_str())
                            .map_or_else(|| self.hybrid_track(uri, None), |track| (*track).clone())
                    })
                    .collect(),
            })
        } else {
            loaded_context
        };
        if let Some(context) = context {
            self.start_hybrid_context(context, &request, shuffle, repeat);
        } else {
            // Show the clicked row immediately while the complete context is
            // resolved. No prefix is sent to an engine as the whole playlist.
            if let Some(uri) = request.offset_uri.as_ref() {
                let track = self.hybrid_track(uri, None);
                self.hybrid.queue.start(
                    Context {
                        uri: context_uri.clone(),
                        name: self.hybrid_context_name(&context_uri),
                        tracks: vec![track],
                    },
                    Some(0),
                    shuffle,
                    repeat_to_queue(repeat),
                );
            }
            self.hybrid.state = LocalState {
                loading: true,
                playback: Playback::Playing,
                position_ms: request.position_ms,
                ..LocalState::default()
            };
            self.hybrid.loaded = false;
            self.hybrid.tag = None;
            let generation = self.hybrid.plan_generation;
            self.hybrid.pending = Some(PendingStart {
                generation,
                request,
                shuffle,
                repeat,
                play: true,
                navigation: Vec::new(),
                context_navigation: Vec::new(),
                resolving: true,
                manual_started: false,
            });
            self.backend.send(Command::Hybrid(HybridCommand::Resolve {
                generation,
                uri: context_uri,
            }));
            self.project_hybrid();
        }
        true
    }

    fn enter_hybrid(&mut self) {
        // Soloist cannot consume this app's mixed-source queue. Stop its child
        // before claiming the shared local output, so two players never overlap.
        if self.settings.soloist_enabled {
            self.settings.soloist_enabled = false;
            self.mark_settings_dirty();
            self.soloist_select = false;
            self.soloist_key.clear();
            self.clear_soloist_target();
            self.soloist_status = crate::soloist::Status::Off;
            self.backend.send(Command::Soloist {
                enabled: false,
                key: None,
            });
            self.toast("Mixed playback uses the built-in player. Your Soloist key is saved.");
        }
        self.backend.send(Command::Hybrid(HybridCommand::Claim));
        self.playback_owner = PlaybackOwner::Hybrid;
        self.selected_device = None;
        self.remote = None;
        self.pending_transfer_to = None;
        self.activating_receiver = None;
        self.local_transfer_sequence = None;
        self.queued_play = None;
        self.intent_track = None;
        self.optimistic_playing = None;
        self.pending_local_position = None;
        self.pending_remote_position = None;
        self.pending_remote_volume = None;
        self.queue_start_pending = None;
        self.pending_queue_adds.clear();
        self.pending_queue_batches.clear();
        // Starting another local collection preserves album additions whose
        // pages are still loading, just like already queued manual rows.
        self.resume_queue.clear();
        self.queue_shuffle_pending = None;
        self.queue_recheck_at = None;
        self.remote_poll_seq = self.remote_poll_seq.wrapping_add(1);
        self.remote_poll_pending = false;
        self.queue_seq = self.queue_seq.wrapping_add(1);
        self.clear_play_pending();
    }

    fn leave_hybrid(&mut self) {
        self.local.repeat = self.hybrid_repeat_mode();
        self.playback_owner = PlaybackOwner::SpotifyConnect;
        self.hybrid.plan_generation = self.hybrid.plan_generation.wrapping_add(1);
        self.hybrid.pending = None;
        self.hybrid.tag = None;
        self.hybrid.loaded = false;
        self.remote = None;
        self.remote_poll_seq = self.remote_poll_seq.wrapping_add(1);
        self.queue_seq = self.queue_seq.wrapping_add(1);
        self.backend.send(Command::Hybrid(HybridCommand::Release));
        self.frame_now = None;
    }

    fn start_hybrid_context(
        &mut self,
        context: Context,
        request: &PlayRequest,
        shuffle: bool,
        repeat: RepeatMode,
    ) {
        let index = request.offset_position.map(|i| i as usize).or_else(|| {
            request
                .offset_uri
                .as_ref()
                .and_then(|uri| context.tracks.iter().position(|t| &t.uri == uri))
        });
        if !context.uri.is_empty() {
            self.note_recent_context(&context.uri);
        }
        let t = self
            .hybrid
            .queue
            .start(context, index, shuffle, repeat_to_queue(repeat));
        self.hybrid_transition(t, request.position_ms, true);
    }

    pub(super) fn reload_soundcloud_hybrid_context(&mut self) {
        let Some(pending) = &mut self.hybrid.pending else {
            return;
        };
        if !pending.resolving {
            return;
        }
        let Some(uri) =
            pending.request.context_uri.as_ref().filter(|uri| {
                uri.starts_with("soundcloud:") || crate::soundcloud::browse::is_url(uri)
            })
        else {
            return;
        };
        self.hybrid.plan_generation = self.hybrid.plan_generation.wrapping_add(1);
        pending.generation = self.hybrid.plan_generation;
        self.backend.send(Command::Hybrid(HybridCommand::Resolve {
            generation: pending.generation,
            uri: uri.clone(),
        }));
    }

    pub(super) fn handle_hybrid_context(
        &mut self,
        generation: u64,
        result: Result<Context, String>,
    ) {
        if !self.hybrid_owned()
            || self
                .hybrid
                .pending
                .as_ref()
                .is_none_or(|p| p.generation != generation)
        {
            return;
        }
        let pending = self.hybrid.pending.take().unwrap();
        match result {
            Ok(mut context) => {
                self.cache_soundcloud_tracks(&context.tracks);
                if self.own_soundcloud_likes_context(&context.uri) {
                    self.overlay_soundcloud_likes(&mut context.tracks);
                }
                if let Some(id) = context.uri.strip_prefix("spotify:playlist:")
                    && let Some(overlay) = self.playlist_overlay.get(id)
                {
                    context.tracks = crate::playlist_overlay::merge(&context.tracks, overlay)
                        .into_iter()
                        .map(|r| r.to_track())
                        .collect();
                }
                let index =
                    if self.own_soundcloud_likes_context(&context.uri) {
                        pending.request.offset_uri.as_ref().and_then(|uri| {
                            context.tracks.iter().position(|track| &track.uri == uri)
                        })
                    } else {
                        None
                    }
                    .or(pending
                        .request
                        .offset_position
                        .map(|i| i as usize)
                        .or_else(|| {
                            pending
                                .request
                                .offset_uri
                                .as_ref()
                                .and_then(|uri| context.tracks.iter().position(|t| &t.uri == uri))
                        }));
                if !context.uri.is_empty() {
                    self.note_recent_context(&context.uri);
                }
                let mut transition = if pending.manual_started {
                    self.hybrid.queue.resolve_context_behind_manual(
                        context,
                        index,
                        pending.shuffle,
                        repeat_to_queue(pending.repeat),
                        &pending.context_navigation,
                    );
                    None
                } else {
                    Some(self.hybrid.queue.start(
                        context,
                        index,
                        pending.shuffle,
                        repeat_to_queue(pending.repeat),
                    ))
                };
                for next in pending.navigation {
                    transition = Some(if next {
                        self.hybrid.queue.next()
                    } else {
                        self.hybrid.queue.previous(0)
                    });
                }
                if let Some(transition) = transition {
                    self.hybrid_transition(transition, pending.request.position_ms, pending.play);
                } else if self.hybrid.tag.is_none() && pending.play {
                    if let Some(entry) = self.hybrid.queue.current() {
                        self.hybrid_transition(
                            Transition::Play(entry.id),
                            pending.request.position_ms,
                            true,
                        );
                    }
                } else {
                    self.project_hybrid();
                    self.preload_hybrid();
                }
            }
            Err(error) => {
                if self.hybrid.tag.is_none() {
                    self.hybrid.state.loading = false;
                    self.hybrid.state.playback = Playback::Paused;
                    self.hybrid.play_intent = false;
                }
                self.toast_error(error);
                self.hybrid.pending = Some(PendingStart {
                    play: self.hybrid.play_intent,
                    resolving: false,
                    ..pending
                });
                self.project_hybrid();
            }
        }
    }

    fn loaded_hybrid_context(&self, uri: &str) -> Option<Context> {
        if uri == "youtube:likes" {
            return Some(Context {
                uri: uri.into(),
                name: "YouTube Likes".into(),
                tracks: self.youtube_likes()?.to_vec(),
            });
        }
        if self.own_soundcloud_likes_context(uri) {
            return (self.soundcloud_library.snapshot.likes_loaded
                && self.soundcloud_library.snapshot.next_likes.is_none())
            .then(|| Context {
                uri: uri.into(),
                name: gettext(self.locale, "SoundCloud Likes").into_owned(),
                tracks: self.soundcloud_library.snapshot.likes.clone(),
            });
        }
        if uri.starts_with("spotify:")
            && uri.ends_with(":collection")
            && self.library.liked.is_complete()
        {
            return Some(Context {
                uri: uri.into(),
                name: gettext(self.locale, "Liked Songs").into_owned(),
                tracks: self
                    .library
                    .liked
                    .items
                    .iter()
                    .map(|saved| saved.track.clone())
                    .collect(),
            });
        }
        if let Some(id) = uri.strip_prefix(crate::mixed_playlists::PLAYLIST_PREFIX) {
            let playlist = self.mixed_playlists.get(id)?;
            return Some(Context {
                uri: uri.into(),
                name: playlist.name.clone(),
                tracks: playlist.to_tracks(),
            });
        }
        if crate::youtube::is_external(uri) {
            let track = self.hybrid_cached_track(uri)?;
            return Some(Context {
                uri: uri.into(),
                name: track.name.clone(),
                tracks: vec![track],
            });
        }
        if let Some(id) = uri.strip_prefix("spotify:playlist:") {
            let page = self.playlist_pages.get(id)?;
            if page.items.base_offset != 0
                || page
                    .items
                    .total
                    .is_none_or(|n| n as usize != page.items.items.len())
            {
                return None;
            }
            let mut tracks: Vec<_> = page
                .items
                .items
                .iter()
                .map(|i| match i.playable() {
                    Some(PlayableItem::Track(t)) => t.clone(),
                    _ => Track {
                        is_playable: Some(false),
                        ..Track::default()
                    },
                })
                .collect();
            if let Some(overlay) = self.playlist_overlay.get(id) {
                tracks = crate::playlist_overlay::merge(&tracks, overlay)
                    .into_iter()
                    .map(|r| r.to_track())
                    .collect();
            }
            return Some(Context {
                uri: uri.into(),
                name: page.playlist.get().map_or_else(
                    || gettext(self.locale, "Playlist").into_owned(),
                    |p| p.name.clone(),
                ),
                tracks,
            });
        }
        if let Some(id) = uri.strip_prefix("spotify:album:") {
            let page = self.album_pages.get(id)?;
            if page.tracks.base_offset != 0
                || page
                    .tracks
                    .total
                    .is_none_or(|n| n as usize != page.tracks.items.len())
            {
                return None;
            }
            return Some(Context {
                uri: uri.into(),
                name: page.album.get().map_or_else(
                    || gettext(self.locale, "Album").into_owned(),
                    |a| a.name.clone(),
                ),
                tracks: page.tracks.items.clone(),
            });
        }
        if uri.starts_with("spotify:track:") {
            let track = self.hybrid_track(uri, None);
            return Some(Context {
                uri: uri.into(),
                name: track.name.clone(),
                tracks: vec![track],
            });
        }
        None
    }

    fn hybrid_context_name(&self, uri: &str) -> String {
        if uri == "youtube:likes" {
            return "YouTube Likes".into();
        }
        if uri.starts_with("soundcloud:likes:") {
            return gettext(self.locale, "SoundCloud Likes").into_owned();
        }
        if crate::soundcloud::playlist_id(uri).is_some()
            || crate::soundcloud::user_id(uri).is_some()
        {
            return self
                .soundcloud_pages
                .get(uri)
                .and_then(|page| page.content.get())
                .map(|page| page.title.clone())
                .filter(|title| !title.is_empty())
                .or_else(|| {
                    self.soundcloud_library
                        .snapshot
                        .playlists
                        .iter()
                        .find(|playlist| playlist.uri == uri)
                        .map(|playlist| playlist.name.clone())
                })
                .or_else(|| {
                    self.soundcloud_library
                        .snapshot
                        .artists
                        .iter()
                        .find(|artist| artist.uri == uri)
                        .map(|artist| artist.name.clone())
                })
                .unwrap_or_else(|| gettext(self.locale, "SoundCloud").into_owned());
        }
        if let Some(id) = uri.strip_prefix(crate::mixed_playlists::PLAYLIST_PREFIX) {
            return self.mixed_playlists.get(id).map_or_else(
                || gettext(self.locale, "Mixed playlist").into_owned(),
                |p| p.name.clone(),
            );
        }
        if let Some(id) = uri.strip_prefix("spotify:playlist:") {
            return self
                .playlist_pages
                .get(id)
                .and_then(|p| p.playlist.get())
                .map_or_else(
                    || gettext(self.locale, "Playlist").into_owned(),
                    |p| p.name.clone(),
                );
        }
        if let Some(id) = uri.strip_prefix("spotify:album:") {
            return self
                .album_pages
                .get(id)
                .and_then(|p| p.album.get())
                .map_or_else(
                    || gettext(self.locale, "Album").into_owned(),
                    |p| p.name.clone(),
                );
        }
        gettext(self.locale, "Mixed playback").into_owned()
    }

    pub(super) fn hybrid_cached_track(&self, uri: &str) -> Option<Track> {
        if let Some(track) = self
            .youtube_likes()
            .unwrap_or_default()
            .iter()
            .find(|track| track.uri == uri)
        {
            return Some(track.clone());
        }
        self.hybrid
            .queue
            .current()
            .filter(|e| e.track.uri == uri)
            .map(|e| e.track.clone())
            .or_else(|| {
                self.hybrid
                    .queue
                    .context()
                    .and_then(|c| c.entries.iter().find(|e| e.track.uri == uri))
                    .map(|e| e.track.clone())
            })
            .or_else(|| {
                self.hybrid
                    .queue
                    .playing_next(usize::MAX)
                    .iter()
                    .find(|e| e.track.uri == uri)
                    .map(|e| e.track.clone())
            })
            .or_else(|| {
                self.track_cache
                    .get(uri)
                    .or_else(|| {
                        uri.strip_prefix("spotify:track:")
                            .and_then(|id| self.track_cache.get(id))
                    })
                    .cloned()
            })
            .or_else(|| {
                self.queue
                    .get()
                    .and_then(|q| {
                        q.queue
                            .iter()
                            .chain(q.currently_playing.iter())
                            .find(|i| i.uri() == uri)
                    })
                    .map(as_track)
            })
            .or_else(|| {
                self.playlist_pages
                    .values()
                    .flat_map(|p| &p.items.items)
                    .filter_map(|i| i.playable())
                    .find(|i| i.uri() == uri)
                    .map(as_track)
            })
            .or_else(|| {
                self.mixed_playlists
                    .playlists
                    .iter()
                    .flat_map(|p| &p.entries)
                    .find(|e| e.track.uri == uri)
                    .map(|e| e.to_track())
            })
            .or_else(|| {
                self.playlist_overlay
                    .playlists
                    .values()
                    .flat_map(|p| &p.entries)
                    .find(|e| e.entry.track.uri == uri)
                    .map(|e| e.entry.to_track())
            })
    }

    fn hybrid_track(&self, uri: &str, label: Option<&str>) -> Track {
        self.hybrid_cached_track(uri).unwrap_or_else(|| Track {
            uri: uri.into(),
            id: uri.strip_prefix("spotify:track:").map(str::to_owned),
            name: label.unwrap_or(uri).into(),
            ..Track::default()
        })
    }

    fn hybrid_rows(&self) -> Vec<crate::hybrid_queue::Entry> {
        self.hybrid
            .queue
            .playing_next(usize::MAX)
            .into_iter()
            .chain(self.hybrid.queue.next_up(usize::MAX))
            .cloned()
            .collect()
    }

    fn promote_hybrid(&mut self) -> bool {
        if matches!(self.target(), Target::Remote(Some(_)))
            || self
                .remote_fresh()
                .is_some_and(|r| r.state.is_playing && !self.local.is_active())
        {
            self.toast(gettext(self.locale, "SoundCloud songs cannot be queued on another device. Start mixed playback on this computer first."));
            return false;
        }
        let current = self.now_playing();
        let context = if let Some(now) = &current {
            if now.shuffle {
                self.toast(gettext(
                    self.locale,
                    "Start a mixed playlist to keep its full shuffle order when adding SoundCloud",
                ));
                return false;
            }
            if let Some(uris) = &self.local_list {
                Some(Context {
                    uri: self.playing_context_uri().unwrap_or_default(),
                    name: gettext(self.locale, "Queue").into_owned(),
                    tracks: uris.iter().map(|u| self.hybrid_track(u, None)).collect(),
                })
            } else if let Some(uri) = self.playing_context_uri() {
                self.loaded_hybrid_context(&uri)
            } else {
                Some(Context {
                    uri: String::new(),
                    name: gettext(self.locale, "Queue").into_owned(),
                    tracks: vec![self.hybrid_track(&now.uri, Some(&now.title))],
                })
            }
        } else {
            Some(Context {
                uri: String::new(),
                name: gettext(self.locale, "Queue").into_owned(),
                tracks: Vec::new(),
            })
        };
        let Some(context) = context else {
            self.toast(gettext(
                self.locale,
                "Open the complete playlist before adding SoundCloud, or start a mixed playlist",
            ));
            return false;
        };
        let index = if let Some(now) = &current {
            let matches: Vec<_> = context
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.uri == now.uri)
                .map(|(i, _)| i)
                .collect();
            let chosen = self
                .played_row
                .as_ref()
                .filter(|row| row.uri == now.uri)
                .map(|row| row.index as usize)
                .filter(|i| matches.contains(i));
            if let Some(index) = chosen.or_else(|| (matches.len() == 1).then(|| matches[0])) {
                Some(index)
            } else {
                self.toast(gettext(self.locale, "Start a mixed playlist to keep the current song and its complete queue when adding SoundCloud"));
                return false;
            }
        } else {
            None
        };
        let manual: Vec<_> = self
            .manual_queue
            .iter()
            .map(|u| self.hybrid_track(u, None))
            .collect();
        self.hybrid.queue.add_to_queue(manual);
        self.enter_hybrid();
        let t = self
            .hybrid
            .queue
            .start(context, index, false, repeat_to_queue(self.repeat_wanted));
        self.hybrid_transition(
            t,
            current.as_ref().map_or(0, |n| n.position_ms),
            current.is_some_and(|n| n.playing),
        );
        true
    }

    fn hybrid_add(&mut self, tracks: Vec<Track>, position: Option<usize>) {
        self.hybrid
            .last_added
            .retain(|_, at| at.elapsed() < QUEUE_ADD_DEBOUNCE);
        // Check against previous gestures before recording this gesture, so
        // repeated occurrences selected together remain distinct.
        let tracks: Vec<_> = tracks
            .into_iter()
            .filter(|t| !self.hybrid.last_added.contains_key(&t.uri) && self.can_queue(&t.uri))
            .collect();
        for track in &tracks {
            self.hybrid
                .last_added
                .insert(track.uri.clone(), Instant::now());
        }
        let at = position
            .unwrap_or(usize::MAX)
            .min(self.hybrid.queue.playing_next(usize::MAX).len());
        let ids = self.hybrid.queue.add_to_queue(tracks);
        if position.is_some() {
            for (offset, id) in ids.iter().enumerate() {
                self.hybrid.queue.move_entry(*id, at + offset);
            }
        }
        self.queued_toast(ids.len());
        self.project_hybrid();
        self.preload_hybrid();
    }

    fn hybrid_transition(&mut self, transition: Transition, position_ms: u32, play: bool) {
        self.hybrid.generation = self.hybrid.generation.wrapping_add(1);
        self.hybrid.seek_pending = None;
        self.hybrid.playing_pending = None;
        self.hybrid.loaded = false;
        self.hybrid.load_settled = false;
        self.hybrid.play_intent = play;
        self.hybrid.audible = false;
        if let Some(pending) = &mut self.hybrid.pending {
            pending.request.position_ms = position_ms;
        }
        self.listening = None;
        self.played_row = None;
        if let Some(controls) = &mut self.media_controls {
            controls.seeked(Duration::from_millis(u64::from(position_ms)));
        }
        match transition {
            Transition::Play(id) | Transition::Restart(id) => {
                let tag = PlaybackTag {
                    generation: self.hybrid.generation,
                    occurrence: id,
                };
                self.hybrid.tag = Some(tag);
                let track = self.hybrid.queue.current().unwrap().track.clone();
                self.hybrid.queue.set_position_ms(position_ms);
                self.hybrid.state = LocalState {
                    playback: if play {
                        Playback::Playing
                    } else {
                        Playback::Paused
                    },
                    loading: true,
                    position_ms,
                    ..LocalState::default()
                };
                self.backend.send(Command::Hybrid(HybridCommand::Load {
                    tag,
                    track: Box::new(track),
                    position_ms,
                    play,
                }));
            }
            Transition::Stopped => {
                self.hybrid.play_intent = false;
                self.hybrid.tag = None;
                self.hybrid.state = LocalState::default();
                self.backend.send(Command::Hybrid(HybridCommand::Stop));
            }
        }
        self.project_hybrid();
        self.preload_hybrid();
        if let Some(now) = self.hybrid_now_playing() {
            self.last_now_playing_uri = Some(now.uri.clone());
            if now.uri.starts_with("spotify:") {
                self.request_contains(vec![now.uri]);
            }
            self.tint_for(now.art_small.as_deref().or(now.art_url.as_deref()));
        }
        if self.show_lyrics_panel || self.lyrics_fullscreen.is_some() {
            self.request_lyrics();
        }
    }

    fn hybrid_next(&mut self) {
        if let Some(pending) = &mut self.hybrid.pending {
            pending.request.position_ms = 0;
            if self.hybrid.queue.playing_next(1).is_empty() {
                pending.navigation.push(true);
                self.hybrid.audible = false;
                self.hybrid.loaded = false;
                self.hybrid.state.loading = pending.resolving;
                if !pending.resolving {
                    pending.play = false;
                    self.hybrid.play_intent = false;
                    self.hybrid.state.playback = Playback::Paused;
                }
                self.hybrid.state.position_ms = 0;
                self.hybrid.state.position_at = None;
                self.hybrid.tag = None;
                self.backend.send(Command::Hybrid(HybridCommand::Stop));
                self.project_hybrid();
                return;
            }
            pending.manual_started = true;
            pending.context_navigation.append(&mut pending.navigation);
        }
        let play = self.hybrid.play_intent;
        let transition = self.hybrid.queue.next();
        self.hybrid_transition(transition, 0, play);
    }

    fn preload_hybrid(&self) {
        if self.hybrid.state.playback != Playback::Playing {
            return;
        }
        if let (Some(tag), Some(entry)) = (self.hybrid.tag, self.hybrid.queue.upcoming()) {
            self.backend.send(Command::Hybrid(HybridCommand::Preload {
                tag,
                track: Box::new(entry.track.clone()),
            }));
        }
    }

    pub(super) fn handle_hybrid_event(&mut self, tag: PlaybackTag, event: HybridEvent) {
        if !self.hybrid_owned() || self.hybrid.tag != Some(tag) {
            return;
        }
        match event {
            HybridEvent::Interrupted => {
                if let Some(entry) = self.hybrid.queue.current() {
                    self.hybrid_transition(
                        Transition::Play(entry.id),
                        self.hybrid_position(),
                        self.hybrid.play_intent,
                    );
                }
            }
            HybridEvent::State(mut state) => {
                if state.track.as_ref().is_none_or(|t| {
                    self.hybrid
                        .queue
                        .current()
                        .is_none_or(|e| e.track.uri != t.uri)
                }) {
                    return;
                }
                self.hybrid.load_settled |= !state.loading;
                if let Some(pending) = self.hybrid.seek_pending {
                    let load_position_ack = pending.allow_load_position_ack
                        && !state.loading
                        && state.position_ms.abs_diff(pending.position_ms) <= 1_000;
                    if state.seek_sequence < pending.sequence && !load_position_ack {
                        state.position_ms = pending.position_ms;
                        state.position_at = None;
                    } else {
                        self.hybrid.seek_pending = None;
                    }
                }
                self.hybrid.audible =
                    state.playback == Playback::Playing && !state.loading && state.error.is_none();
                if self.hybrid.audible {
                    self.hybrid.failed_occurrences.clear();
                }
                // A matching boolean can be a pre-command periodic snapshot.
                // Only a numbered acknowledgement clears a pending command.
                if (state.playback == Playback::Playing) != self.hybrid.play_intent {
                    state.position_ms = self.hybrid_position();
                    state.position_at = None;
                }
                state.playback = if self.hybrid.play_intent {
                    Playback::Playing
                } else {
                    Playback::Paused
                };
                self.hybrid.loaded = true;
                self.hybrid.queue.set_position_ms(state.position_now());
                if state.seek_sequence != self.hybrid.state.seek_sequence
                    && let Some(controls) = &mut self.media_controls
                {
                    controls.seeked(Duration::from_millis(u64::from(state.position_now())));
                }
                self.hybrid.state = *state;
                self.project_hybrid();
            }
            HybridEvent::EndOfTrack => {
                let play = self.hybrid.play_intent;
                if let Some(pending) = self.hybrid.seek_pending {
                    self.hybrid_transition(
                        Transition::Restart(tag.occurrence),
                        pending.position_ms,
                        play,
                    );
                    return;
                }
                self.hybrid.tag = None;
                if self.hybrid.pending.is_some() && self.hybrid.queue.repeat() != Repeat::Track {
                    self.hybrid_next();
                    return;
                }
                let t = self.hybrid.queue.end_of_track();
                self.hybrid_transition(t, 0, play);
            }
            HybridEvent::Failed(error) => {
                let play = self.hybrid.play_intent;
                let playback_blocked = error == crate::player::LOCAL_PLAYBACK_UNAVAILABLE
                    || error == crate::player::AUDIO_KEY_REFUSED;
                self.toast_error(error);
                self.hybrid.tag = None;
                if playback_blocked {
                    // Missing output and refused audio keys are playback-wide
                    // failures. Keep the occurrence for an explicit retry,
                    // without consuming manual rows or the context behind it.
                    self.hybrid.play_intent = false;
                    if let Some(pending) = &mut self.hybrid.pending {
                        pending.play = false;
                    }
                    self.hybrid.audible = false;
                    self.hybrid.loaded = false;
                    self.hybrid.state.loading = false;
                    self.hybrid.state.playback = Playback::Paused;
                    self.hybrid.state.position_at = None;
                    self.show_devices = true;
                    self.refresh_devices();
                    self.project_hybrid();
                    return;
                }
                self.hybrid.failed_occurrences.insert(tag.occurrence);
                if self.hybrid.pending.is_some() {
                    self.hybrid_next();
                    return;
                }
                // Occurrence IDs form a stable retry budget even as manual
                // rows leave the queue. Never wrap into an already failed row.
                if self
                    .hybrid
                    .queue
                    .next_entry()
                    .is_none_or(|entry| self.hybrid.failed_occurrences.contains(&entry.id))
                {
                    self.hybrid.play_intent = false;
                    self.hybrid.audible = false;
                    self.hybrid.state.loading = false;
                    self.hybrid.state.playback = Playback::Paused;
                    self.hybrid.state.position_at = None;
                    self.hybrid.loaded = false;
                    self.backend.send(Command::Hybrid(HybridCommand::Stop));
                    self.project_hybrid();
                } else {
                    let t = self.hybrid.queue.next();
                    self.hybrid_transition(t, 0, play);
                }
            }
            HybridEvent::PlayingAcknowledged { sequence } => {
                if self.hybrid.playing_pending == Some(sequence) {
                    self.hybrid.playing_pending = None;
                }
            }
        }
    }

    pub(super) fn hybrid_position(&self) -> u32 {
        self.hybrid.state.position_now()
    }

    pub(super) fn hybrid_toggle(&mut self) {
        if let Some(pending) = &mut self.hybrid.pending {
            pending.play = !self.hybrid.play_intent;
            if !pending.resolving && pending.play {
                pending.resolving = true;
                if self.hybrid.tag.is_none() {
                    self.hybrid.state.loading = true;
                    self.backend.send(Command::Hybrid(HybridCommand::Claim));
                }
                self.backend.send(Command::Hybrid(HybridCommand::Resolve {
                    generation: pending.generation,
                    uri: pending.request.context_uri.clone().unwrap_or_default(),
                }));
            }
            if self.hybrid.tag.is_some() {
                // The manual row is already playing while its context loads.
                // Carry intent to resolution and also control that decoder.
                self.hybrid_toggle_loaded();
                return;
            }
            self.hybrid.play_intent = pending.play;
            self.hybrid.state.playback = if pending.play {
                Playback::Playing
            } else {
                Playback::Paused
            };
            self.project_hybrid();
            return;
        }
        self.hybrid_toggle_loaded();
    }

    fn hybrid_toggle_loaded(&mut self) {
        if self.hybrid.tag.is_none() {
            if let Some(entry) = self.hybrid.queue.current() {
                self.hybrid_transition(
                    Transition::Play(entry.id),
                    self.hybrid.queue.position_ms(),
                    true,
                );
            } else {
                let t = self.hybrid.queue.next();
                self.hybrid_transition(t, 0, true);
            }
            return;
        }
        let playing = !self.hybrid.play_intent;
        self.hybrid.play_intent = playing;
        self.hybrid.transport_sequence = self.hybrid.transport_sequence.wrapping_add(1);
        let sequence = self.hybrid.transport_sequence;
        self.hybrid.playing_pending = Some(sequence);
        self.hybrid.state.position_ms = self.hybrid_position();
        self.hybrid.state.playback = if playing {
            Playback::Playing
        } else {
            Playback::Paused
        };
        self.hybrid.state.position_at = playing.then(Instant::now);
        if let Some(tag) = self.hybrid.tag {
            self.backend.send(Command::Hybrid(HybridCommand::Playing {
                tag,
                playing,
                sequence,
            }));
        }
        self.project_hybrid();
        self.preload_hybrid();
    }

    pub(super) fn hybrid_seek(&mut self, position_ms: u32) {
        let duration = self.hybrid_now_playing().map_or(0, |n| n.duration_ms);
        let position_ms = if duration == 0 {
            position_ms
        } else {
            position_ms.min(duration)
        };
        if let Some(pending) = &mut self.hybrid.pending {
            pending.request.position_ms = position_ms;
        }
        self.hybrid.queue.set_position_ms(position_ms);
        self.hybrid.state.position_ms = position_ms;
        self.hybrid.state.position_at =
            (self.hybrid.state.playback == Playback::Playing).then(Instant::now);
        let sequence = self
            .hybrid
            .seek_pending
            .map_or(self.hybrid.state.seek_sequence, |pending| pending.sequence)
            .wrapping_add(1);
        // Decoder reopen/buffering after a seek is not an initial Load. Once
        // this generation has settled, every newer seek needs its sequence.
        let allow_load_position_ack = !self.hybrid.load_settled;
        self.hybrid.seek_pending = Some(PendingSeek {
            position_ms,
            sequence,
            allow_load_position_ack,
        });
        if let Some(tag) = self.hybrid.tag {
            self.backend
                .send(Command::Hybrid(HybridCommand::Seek { tag, position_ms }));
        }
        self.project_hybrid();
    }

    pub(super) fn hybrid_shuffle(&mut self, on: bool) {
        if let Some(pending) = &mut self.hybrid.pending {
            pending.shuffle = on;
        }
        self.hybrid.queue.set_shuffle(on);
        self.shuffle_wanted = on;
        self.project_hybrid();
        self.preload_hybrid();
    }

    pub(super) fn hybrid_repeat(&mut self, mode: RepeatMode) {
        if let Some(pending) = &mut self.hybrid.pending {
            pending.repeat = mode;
        }
        self.hybrid.queue.set_repeat(repeat_to_queue(mode));
        self.repeat_wanted = mode;
        self.project_hybrid();
        self.preload_hybrid();
    }

    pub(super) fn hybrid_repeat_mode(&self) -> RepeatMode {
        match self.hybrid.queue.repeat() {
            Repeat::Off => RepeatMode::Off,
            Repeat::Context => RepeatMode::Context,
            Repeat::Track => RepeatMode::Track,
        }
    }

    pub(super) fn project_hybrid(&mut self) {
        self.queue = Loadable::Loaded(Queue {
            currently_playing: self
                .hybrid
                .queue
                .current()
                .map(|e| PlayableItem::Track(e.track.clone())),
            queue: self
                .hybrid_rows()
                .into_iter()
                .map(|e| PlayableItem::Track(e.track))
                .collect(),
        });
        self.manual_queue = self
            .hybrid
            .queue
            .playing_next(usize::MAX)
            .iter()
            .map(|e| e.track.uri.clone())
            .collect();
        self.frame_now = None;
        self.session_dirty = true;
        self.resume_context = self.hybrid_context_uri();
        self.resume_track = self.hybrid.queue.current().map(|e| e.track.uri.clone());
        self.resume_position_ms = self.hybrid_position();
    }

    pub(super) fn hybrid_now_playing(&self) -> Option<NowPlaying> {
        let track = &self.hybrid.queue.current()?.track;
        let soundcloud = crate::soundcloud::is_uri(&track.uri);
        let child = self
            .hybrid
            .state
            .track
            .as_ref()
            .filter(|t| t.uri == track.uri);
        let mut artists = child
            .filter(|t| track.artists.is_empty() && !t.artists.is_empty())
            .map_or_else(|| track.artists.clone(), |t| t.artists.clone());
        if soundcloud {
            for artist in &mut artists {
                artist.id = None;
            }
        }
        Some(NowPlaying {
            local: true,
            device_name: Some(self.settings.device_name.clone()),
            device_kind: Some("Computer".into()),
            uri: track.uri.clone(),
            id: track.id.clone(),
            title: child
                .filter(|t| !t.title.is_empty())
                .map_or_else(|| track.name.clone(), |t| t.title.clone()),
            subtitle: crate::api::models::join_names(artists.iter().map(|a| a.name.as_str())),
            artists,
            album_name: track.album.as_ref().map_or_else(
                || child.map_or_else(String::new, |t| t.album.clone()),
                |a| a.name.clone(),
            ),
            album_id: track
                .album
                .as_ref()
                .filter(|a| !soundcloud && !a.id.is_empty())
                .map(|a| a.id.clone()),
            show_id: None,
            art_url: track
                .image(640)
                .map(str::to_owned)
                .or_else(|| child.and_then(|t| t.art_url.clone())),
            art_small: track
                .image(64)
                .map(str::to_owned)
                .or_else(|| child.and_then(|t| t.art_small_url.clone())),
            duration_ms: child
                .filter(|t| t.duration_ms != 0)
                .map_or(track.duration_ms, |t| t.duration_ms),
            position_ms: self.hybrid_position(),
            playing: self.hybrid.state.playback == Playback::Playing,
            loading: self.hybrid.state.loading,
            shuffle: self.hybrid.queue.shuffle(),
            repeat: self.hybrid_repeat_mode(),
            volume_percent: volume_to_percent(self.local.volume),
            can_control: true,
            can_set_volume: true,
            is_episode: track.uri.starts_with("spotify:episode:")
                || child.is_some_and(|t| t.is_episode),
            resuming: !self.hybrid.loaded,
        })
    }

    /// Provider-safe destination for the player bar's title and cover.
    pub fn now_playing_collection_page(&self, now: &NowPlaying) -> Option<Page> {
        if crate::soundcloud::is_uri(&now.uri) {
            return self
                .hybrid
                .queue
                .current()
                .filter(|entry| entry.track.uri == now.uri)
                .and_then(|entry| entry.track.album.as_ref())
                .and_then(|album| Page::from_uri(&album.uri));
        }
        if let Some(id) = &now.album_id
            && util::uri_kind(&now.uri) == Some("track")
        {
            return Some(Page::Album(id.clone()));
        }
        now.show_id.as_ref().map(|id| Page::Show(id.clone()))
    }

    pub(super) fn hybrid_snapshot(&self) -> Option<Snapshot> {
        self.hybrid_owned().then(|| {
            let mut queue = self.hybrid.queue.clone();
            queue.set_position_ms(self.hybrid_position());
            queue.snapshot()
        })
    }

    pub(super) fn hybrid_start_snapshot(&self) -> Option<PendingHybridSnapshot> {
        let pending = self
            .hybrid
            .pending
            .as_ref()
            .filter(|_| self.hybrid_owned())?;
        Some(PendingHybridSnapshot {
            context_uri: pending.request.context_uri.clone(),
            uris: pending.request.uris.clone(),
            offset_uri: pending.request.offset_uri.clone(),
            offset_position: pending.request.offset_position,
            position_ms: if pending.manual_started {
                self.hybrid_position()
            } else {
                pending.request.position_ms
            },
            shuffle: pending.shuffle,
            repeat: repeat_to_queue(pending.repeat),
            navigation: pending.navigation.clone(),
            manual_started: pending.manual_started,
            context_navigation: pending.context_navigation.clone(),
        })
    }

    pub(super) fn restore_hybrid_start(&mut self, saved: Option<PendingHybridSnapshot>) {
        let Some(saved) = saved else {
            return;
        };
        self.playback_owner = PlaybackOwner::Hybrid;
        self.hybrid.plan_generation = self.hybrid.plan_generation.wrapping_add(1);
        self.hybrid.state = LocalState {
            playback: Playback::Paused,
            position_ms: saved.position_ms,
            ..Default::default()
        };
        let repeat = match saved.repeat {
            Repeat::Off => RepeatMode::Off,
            Repeat::Context => RepeatMode::Context,
            Repeat::Track => RepeatMode::Track,
        };
        self.hybrid.pending = Some(PendingStart {
            generation: self.hybrid.plan_generation,
            request: PlayRequest {
                context_uri: saved.context_uri,
                uris: saved.uris,
                offset_uri: saved.offset_uri,
                offset_position: saved.offset_position,
                position_ms: saved.position_ms,
            },
            shuffle: saved.shuffle,
            repeat,
            play: false,
            navigation: saved.navigation,
            resolving: false,
            manual_started: saved.manual_started,
            context_navigation: saved.context_navigation,
        });
        self.project_hybrid();
    }

    #[cfg(test)]
    pub(super) fn hybrid_tag(&self) -> Option<PlaybackTag> {
        self.hybrid.tag
    }

    pub(super) fn restore_hybrid(&mut self, snapshot: Option<Snapshot>) {
        if let Some(snapshot) = snapshot
            && let Ok(queue) = HybridQueue::restore(snapshot)
        {
            let position_ms = queue.position_ms();
            self.hybrid.queue = queue;
            self.playback_owner = PlaybackOwner::Hybrid;
            self.resume_queue.clear();
            self.intent_track = None;
            self.hybrid.state = LocalState {
                playback: Playback::Paused,
                position_ms,
                ..LocalState::default()
            };
            self.shuffle_wanted = self.hybrid.queue.shuffle();
            self.repeat_wanted = self.hybrid_repeat_mode();
            self.project_hybrid();
        }
    }
}

fn as_track(item: &PlayableItem) -> Track {
    match item {
        PlayableItem::Track(t) => t.clone(),
        PlayableItem::Episode(e) => Track {
            id: Some(e.id.clone()),
            uri: e.uri.clone(),
            name: e.name.clone(),
            duration_ms: e.duration_ms,
            explicit: e.explicit,
            album: Some(Album {
                images: e.images.clone(),
                ..Album::default()
            }),
            ..Track::default()
        },
    }
}
fn repeat_to_queue(mode: RepeatMode) -> Repeat {
    match mode {
        RepeatMode::Off => Repeat::Off,
        RepeatMode::Context => Repeat::Context,
        RepeatMode::Track => Repeat::Track,
    }
}
