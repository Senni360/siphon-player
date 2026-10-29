//! The app-owned play order for contexts containing Spotify and SoundCloud.
//!
//! No engine reports are applied here: each command changes the authoritative
//! queue immediately. The caller dispatches the returned [`Transition`] to the
//! engine selected by [`Entry::source`], and records elapsed playback with
//! [`HybridQueue::set_position_ms`]. Pause and engine ownership stay with the
//! caller. A snapshot restores state without consuming a row or starting audio.
//! Spotify-only contexts remain the caller's responsibility through Connect;
//! this module does not choose or contact a playback engine.

use std::collections::{HashSet, VecDeque};

use rand::{SeedableRng, seq::SliceRandom};
use serde::{Deserialize, Serialize};

use crate::{api::models::Track, soundcloud::Source};

/// Identifies an occurrence, never a recording or URI. IDs survive snapshots
/// and are not reused when a new context starts in the same queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    Context { index: usize },
    Manual,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: EntryId,
    pub track: Track,
    pub origin: Origin,
}

impl Entry {
    pub fn source(&self) -> Source {
        Source::of(&self.track.uri)
    }

    fn playable(&self) -> bool {
        self.track.is_playable != Some(false)
    }
}

/// The caller supplies the complete context in the desired original order.
/// Filtered or sorted collections supply their displayed playback order here.
#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    pub uri: String,
    pub name: String,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueueContext {
    pub uri: String,
    pub name: String,
    /// Includes unavailable rows so original indices and duplicate occurrences
    /// retain their identity. Only playable entries enter the play order.
    pub entries: Vec<Entry>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    #[default]
    Off,
    Context,
    Track,
}

/// An instruction to load an occurrence, seek it to zero, or stop audio.
/// Even when two occurrences share a URI, `Play` means a new playback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    Play(EntryId),
    Restart(EntryId),
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Played {
    entry: Entry,
    /// Context occurrence after which playback resumes, including when this
    /// entry was manually queued. Unlike an order index it survives shuffling.
    anchor: Option<EntryId>,
}

/// Versioned persistence value. Serialize this with serde, then pass it to
/// [`HybridQueue::restore`]; private fields prevent unchecked construction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    version: u32,
    context: Option<QueueContext>,
    order: Vec<usize>,
    cursor: usize,
    anchor: Option<EntryId>,
    current: Option<Played>,
    manual: VecDeque<Entry>,
    history: Vec<Played>,
    shuffle: bool,
    repeat: Repeat,
    seed: u64,
    shuffle_round: u64,
    next_id: u64,
    position_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RestoreError {
    #[error("unsupported hybrid queue snapshot version {0}")]
    UnsupportedVersion(u32),
    #[error("invalid hybrid queue snapshot: {0}")]
    InvalidState(&'static str),
}

/// Authoritative playback state for one app-owned context and its manual queue.
/// Commands mutate it synchronously and return instructions for the caller's
/// engine. No playback or network events can overwrite these decisions.
#[derive(Clone, Debug)]
pub struct HybridQueue {
    state: Snapshot,
}

impl HybridQueue {
    /// The seed and shuffle counter are persisted, so future shuffle commands
    /// produce the same results after restoration. No ambient RNG is used.
    pub fn new(seed: u64) -> Self {
        Self {
            state: Snapshot {
                version: 1,
                context: None,
                order: Vec::new(),
                cursor: 0,
                anchor: None,
                current: None,
                manual: VecDeque::new(),
                history: Vec::new(),
                shuffle: false,
                repeat: Repeat::Off,
                seed,
                shuffle_round: 0,
                next_id: 1,
                position_ms: 0,
            },
        }
    }

    /// Starts a collection immediately, preserving pending manually queued
    /// copies and clearing the previous collection's playback history.
    /// `start_at` names an original row, including a duplicate occurrence.
    /// An unavailable chosen row falls forward to the next playable row; an
    /// invalid index or exhausted suffix stops rather than playing another list.
    /// With shuffle, the chosen playable occurrence leads a permutation of all
    /// playable context entries. Without a choice the entire list is shuffled.
    pub fn start(
        &mut self,
        context: Context,
        start_at: Option<usize>,
        shuffle: bool,
        repeat: Repeat,
    ) -> Transition {
        let entries = context
            .tracks
            .into_iter()
            .enumerate()
            .map(|(index, track)| self.entry(track, Origin::Context { index }))
            .collect();
        self.state.context = Some(QueueContext {
            uri: context.uri,
            name: context.name,
            entries,
        });
        self.state.order = self.original_order();
        self.state.cursor = 0;
        self.state.anchor = None;
        self.state.current = None;
        self.state.history.clear();
        self.state.position_ms = 0;
        self.state.shuffle = shuffle;
        self.state.repeat = repeat;

        let chosen =
            start_at.and_then(|index| self.state.order.iter().copied().find(|row| *row >= index));
        if shuffle {
            self.shuffle_remainder();
            if let Some(index) = chosen {
                let position = self
                    .state
                    .order
                    .iter()
                    .position(|row| *row == index)
                    .unwrap();
                self.state.order.swap(0, position);
            }
        } else if let Some(index) = chosen {
            self.state.cursor = self
                .state
                .order
                .iter()
                .position(|row| *row == index)
                .unwrap();
        }
        if start_at.is_some() && chosen.is_none() {
            self.state.cursor = self.state.order.len();
            self.state.anchor = self
                .state
                .order
                .last()
                .map(|index| self.state.context.as_ref().unwrap().entries[*index].id);
            return Transition::Stopped;
        }
        // Starting a collection plays its own first song. Manual rows follow it.
        self.advance_context(false)
    }

    pub fn context(&self) -> Option<&QueueContext> {
        self.state.context.as_ref()
    }

    /// Fill an unresolved context behind manual occurrences already played.
    /// Their IDs, the current decoder, and remaining manual rows stay intact.
    pub fn resolve_context_behind_manual(
        &mut self,
        context: Context,
        start_at: Option<usize>,
        shuffle: bool,
        repeat: Repeat,
        navigation: &[bool],
    ) {
        let current = self.state.current.take();
        let history: Vec<_> = self
            .state
            .history
            .drain(..)
            .filter(|played| played.entry.origin == Origin::Manual)
            .collect();
        let position = self.state.position_ms;
        let manual = std::mem::take(&mut self.state.manual);
        self.start(context, start_at, shuffle, repeat);
        for next in navigation {
            if *next {
                self.next();
            } else {
                self.previous(0);
            }
        }
        self.state.manual = manual;
        if let Some(played) = self.state.current.take() {
            self.state.history.push(played);
        }
        let anchor = self.state.anchor;
        self.state
            .history
            .extend(history.into_iter().map(|mut played| {
                played.anchor = anchor;
                played
            }));
        self.state.current = current.map(|mut played| {
            played.anchor = anchor;
            played
        });
        self.state.position_ms = position;
    }

    pub fn current(&self) -> Option<&Entry> {
        self.state.current.as_ref().map(|played| &played.entry)
    }

    pub fn current_source(&self) -> Option<Source> {
        self.current().map(Entry::source)
    }

    /// The occurrence an explicit Next would play, ignoring repeat-one.
    pub fn next_entry(&self) -> Option<&Entry> {
        self.state.manual.front().or_else(|| {
            let index = self.state.order.get(self.state.cursor).or_else(|| {
                if self.state.repeat == Repeat::Context {
                    self.state.order.first()
                } else {
                    None
                }
            })?;
            self.context()?.entries.get(*index)
        })
    }

    /// Source for an explicit Next. Use [`Self::upcoming_source`] instead when
    /// preloading for natural completion, which honors repeat-one.
    pub fn next_source(&self) -> Option<Source> {
        self.next_entry().map(Entry::source)
    }

    /// The occurrence to preload for natural track completion. Repeat-one
    /// preloads the current occurrence, even with manual songs waiting.
    pub fn upcoming(&self) -> Option<&Entry> {
        if self.state.repeat == Repeat::Track && self.current().is_some() {
            self.current()
        } else {
            self.next_entry()
        }
    }

    pub fn upcoming_source(&self) -> Option<Source> {
        self.upcoming().map(Entry::source)
    }

    pub fn playing_next(&self, limit: usize) -> Vec<&Entry> {
        self.state.manual.iter().take(limit).collect()
    }

    /// Context rows in explicit Next order, after Playing next. Repeat-context
    /// includes the wrap, but at most one cycle, so a row ID occurs only once.
    /// Repeat-one does not hide the rows an explicit Next can still reach.
    pub fn next_up(&self, limit: usize) -> Vec<&Entry> {
        let Some(context) = self.context() else {
            return Vec::new();
        };
        let end = if self.state.repeat == Repeat::Context {
            self.state.cursor
        } else {
            0
        };
        self.state.order[self.state.cursor..]
            .iter()
            .chain(&self.state.order[..end])
            .take(limit)
            .map(|index| &context.entries[*index])
            .collect()
    }

    /// Oldest first. Contains playback visits, so repeat-context may visit the
    /// same occurrence more than once. Restarting a song adds no history visit.
    pub fn history(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.state.history.iter().map(|played| &played.entry)
    }

    pub fn shuffle(&self) -> bool {
        self.state.shuffle
    }

    /// Shuffle never touches manual entries. Enabling it permutes only the
    /// unplayed context suffix; disabling it resumes original order after the
    /// last context occurrence, also while a manual occurrence is playing.
    pub fn set_shuffle(&mut self, on: bool) {
        if self.state.shuffle == on {
            return;
        }
        self.state.shuffle = on;
        if on {
            self.shuffle_remainder();
        } else {
            self.state.order = self.original_order();
            self.restore_cursor();
        }
    }

    pub fn repeat(&self) -> Repeat {
        self.state.repeat
    }

    pub fn set_repeat(&mut self, mode: Repeat) {
        self.state.repeat = mode;
    }

    pub fn position_ms(&self) -> u32 {
        self.state.position_ms
    }

    /// Record an engine's position before saving. Transitions reset it to zero.
    pub fn set_position_ms(&mut self, position_ms: u32) {
        self.state.position_ms = if self.current().is_some() {
            position_ms
        } else {
            0
        };
    }

    /// Consumes a manual row first, then the context. Only repeat-context
    /// wraps the context; repeat-one never prevents an explicit skip.
    #[expect(
        clippy::should_implement_trait,
        reason = "Next is a playback command returning a Transition, not an Iterator item."
    )]
    pub fn next(&mut self) -> Transition {
        if let Some(played) = self.state.current.take() {
            self.state.history.push(played);
        }
        self.state.position_ms = 0;
        if let Some(entry) = self.state.manual.pop_front() {
            let id = entry.id;
            self.state.current = Some(Played {
                entry,
                anchor: self.state.anchor,
            });
            Transition::Play(id)
        } else {
            self.advance_context(self.state.repeat == Repeat::Context)
        }
    }

    pub fn end_of_track(&mut self) -> Transition {
        if self.state.repeat == Repeat::Track && self.current().is_some() {
            self.restart()
        } else {
            self.next()
        }
    }

    /// Over 3000 ms restarts; at or below it visits real playback history.
    /// Going back returns a consumed manual occurrence to the queue's front
    /// and rewinds the context to the historical anchor. Manual priority still
    /// applies on the next forward step, including after several Previous calls.
    pub fn previous(&mut self, position_ms: u32) -> Transition {
        if position_ms > 3000 && self.current().is_some() {
            return self.restart();
        }
        let Some(played) = self.state.history.pop() else {
            return self.restart();
        };
        if let Some(current) = self.state.current.take()
            && current.entry.origin == Origin::Manual
        {
            self.state.manual.push_front(current.entry);
        }
        self.state.anchor = played.anchor;
        self.restore_cursor();
        let id = played.entry.id;
        self.state.current = Some(played);
        self.state.position_ms = 0;
        Transition::Play(id)
    }

    /// Returns accepted occurrence IDs in input order. Unavailable entries are
    /// omitted; unknown playability is allowed. Gesture deduplication belongs
    /// to the caller: two explicit additions intentionally create two entries.
    pub fn add_to_queue(&mut self, tracks: Vec<Track>) -> Vec<EntryId> {
        let entries = self.manual_entries(tracks);
        let ids = entries.iter().map(|entry| entry.id).collect();
        self.state.manual.extend(entries);
        ids
    }

    pub fn play_next(&mut self, tracks: Vec<Track>) -> Vec<EntryId> {
        let entries = self.manual_entries(tracks);
        let ids = entries.iter().map(|entry| entry.id).collect();
        for entry in entries.into_iter().rev() {
            self.state.manual.push_front(entry);
        }
        ids
    }

    /// Only pending manual occurrences can be removed. Context, history and
    /// current IDs are harmless no-ops, including matching recordings.
    pub fn remove(&mut self, ids: &[EntryId]) {
        let ids: HashSet<_> = ids.iter().copied().collect();
        self.state.manual.retain(|entry| !ids.contains(&entry.id));
    }

    /// `to` is the final zero-based index, clamped to the last manual row.
    pub fn move_entry(&mut self, id: EntryId, to: usize) -> bool {
        let Some(from) = self.state.manual.iter().position(|entry| entry.id == id) else {
            return false;
        };
        let entry = self.state.manual.remove(from).unwrap();
        self.state
            .manual
            .insert(to.min(self.state.manual.len()), entry);
        true
    }

    pub fn clear_queue(&mut self) {
        self.state.manual.clear();
    }

    /// Exactly the same state changes as repeated Next, including its history.
    /// An ID absent from the upcoming queue leaves the entire state unchanged.
    pub fn skip_to(&mut self, id: EntryId) -> Option<Transition> {
        let manual_index = self.state.manual.iter().position(|entry| entry.id == id);
        let steps = if let Some(index) = manual_index {
            index + 1
        } else {
            self.state.manual.len()
                + self
                    .next_up(self.state.order.len())
                    .iter()
                    .position(|entry| entry.id == id)?
                + 1
        };
        let mut transition = Transition::Stopped;
        for _ in 0..steps {
            transition = self.next();
        }
        Some(transition)
    }

    pub fn snapshot(&self) -> Snapshot {
        self.state.clone()
    }

    pub fn restore(snapshot: Snapshot) -> Result<Self, RestoreError> {
        snapshot.validate()?;
        Ok(Self { state: snapshot })
    }

    fn entry(&mut self, track: Track, origin: Origin) -> Entry {
        let id = EntryId(self.state.next_id);
        self.state.next_id = self
            .state
            .next_id
            .checked_add(1)
            .expect("queue ID space exhausted");
        Entry { id, track, origin }
    }

    fn manual_entries(&mut self, tracks: Vec<Track>) -> Vec<Entry> {
        tracks
            .into_iter()
            .filter(|track| track.is_playable != Some(false))
            .map(|track| self.entry(track, Origin::Manual))
            .collect()
    }

    fn original_order(&self) -> Vec<usize> {
        self.context()
            .into_iter()
            .flat_map(|context| &context.entries)
            .enumerate()
            .filter_map(|(index, entry)| entry.playable().then_some(index))
            .collect()
    }

    fn shuffle_remainder(&mut self) {
        let seed = self.state.seed.wrapping_add(self.state.shuffle_round);
        self.state.shuffle_round = self.state.shuffle_round.wrapping_add(1);
        self.state.order[self.state.cursor..].shuffle(&mut rand::rngs::StdRng::seed_from_u64(seed));
    }

    fn restore_cursor(&mut self) {
        self.state.cursor = self
            .state
            .anchor
            .and_then(|id| {
                let context = self.context()?;
                self.state
                    .order
                    .iter()
                    .position(|index| context.entries[*index].id == id)
            })
            .map_or(0, |index| index + 1);
    }

    fn advance_context(&mut self, wrap: bool) -> Transition {
        if wrap && self.state.cursor == self.state.order.len() {
            self.state.cursor = 0;
        }
        let Some(index) = self.state.order.get(self.state.cursor).copied() else {
            return Transition::Stopped;
        };
        let entry = self.context().unwrap().entries[index].clone();
        self.state.cursor += 1;
        self.state.anchor = Some(entry.id);
        let id = entry.id;
        self.state.current = Some(Played {
            entry,
            anchor: Some(id),
        });
        Transition::Play(id)
    }

    fn restart(&mut self) -> Transition {
        self.state.position_ms = 0;
        self.current()
            .map_or(Transition::Stopped, |entry| Transition::Restart(entry.id))
    }
}

impl Snapshot {
    fn validate(&self) -> Result<(), RestoreError> {
        use RestoreError::InvalidState;
        if self.version != 1 {
            return Err(RestoreError::UnsupportedVersion(self.version));
        }
        let context = self
            .context
            .as_ref()
            .map_or(&[][..], |context| context.entries.as_slice());
        let mut ids = HashSet::new();
        for (index, entry) in context.iter().enumerate() {
            if entry.origin != (Origin::Context { index }) || !ids.insert(entry.id) {
                return Err(InvalidState("invalid context occurrence"));
            }
        }
        let expected: Vec<_> = context
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| entry.playable().then_some(index))
            .collect();
        let mut actual = self.order.clone();
        actual.sort_unstable();
        if actual != expected || self.cursor > self.order.len() {
            return Err(InvalidState("invalid play order or cursor"));
        }
        if !self.shuffle && self.order != expected {
            return Err(InvalidState("unshuffled order differs from context"));
        }
        let anchors: HashSet<_> = self.order.iter().map(|index| context[*index].id).collect();
        if self.anchor.is_some_and(|id| !anchors.contains(&id)) {
            return Err(InvalidState("unknown context anchor"));
        }
        if self.anchor.is_none() && self.cursor != 0 {
            return Err(InvalidState("cursor has no context anchor"));
        }
        if let Some(anchor) = self.anchor
            && self
                .cursor
                .checked_sub(1)
                .and_then(|cursor| self.order.get(cursor))
                .map(|index| context[*index].id)
                != Some(anchor)
        {
            return Err(InvalidState("cursor differs from context anchor"));
        }
        for entry in &self.manual {
            if entry.origin != Origin::Manual || !entry.playable() || !ids.insert(entry.id) {
                return Err(InvalidState("invalid manual occurrence"));
            }
        }
        let mut manual_visits = HashSet::new();
        for played in self.history.iter().chain(&self.current) {
            let entry = &played.entry;
            if !entry.playable() || played.anchor.is_some_and(|id| !anchors.contains(&id)) {
                return Err(InvalidState("invalid playback visit"));
            }
            match entry.origin {
                Origin::Context { index } => {
                    if context.get(index) != Some(entry) || played.anchor != Some(entry.id) {
                        return Err(InvalidState("unknown context occurrence"));
                    }
                }
                Origin::Manual => {
                    if ids.contains(&entry.id) || !manual_visits.insert(entry.id) {
                        return Err(InvalidState("duplicate manual occurrence"));
                    }
                }
            }
        }
        if ids
            .iter()
            .chain(&manual_visits)
            .any(|id| id.0 == 0 || id.0 >= self.next_id)
            || self.next_id == 0
            || self.next_id == u64::MAX
        {
            return Err(InvalidState("invalid next occurrence ID"));
        }
        if let Some(current) = &self.current {
            if current.anchor != self.anchor {
                return Err(InvalidState("current context anchor differs"));
            }
        } else if self.position_ms != 0 {
            return Err(InvalidState("stopped playback has a position"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolving_context_retains_manual_identity_history_and_position() {
        let mut queue = HybridQueue::new(7);
        queue.start(context(&["a"]), Some(0), false, Repeat::Off);
        let manual = queue.add_to_queue(tracks(&["soundcloud:track:7", "soundcloud:track:7", "z"]));
        assert_eq!(queue.next(), Transition::Play(manual[0]));
        assert_eq!(queue.next(), Transition::Play(manual[1]));
        queue.set_position_ms(7_000);
        queue.resolve_context_behind_manual(
            context(&["a", "b", "c"]),
            Some(0),
            false,
            Repeat::Context,
            &[],
        );
        assert_eq!(queue.current().unwrap().id, manual[1]);
        assert_eq!(queue.position_ms(), 7_000);
        assert_eq!(queue.playing_next(9)[0].id, manual[2]);
        assert_eq!(
            queue
                .next_up(9)
                .iter()
                .map(|entry| entry.track.name.as_str())
                .collect::<Vec<_>>(),
            ["b", "c", "a"]
        );
        // Rebased context anchors must also produce a valid saved queue.
        let mut restored = HybridQueue::restore(queue.snapshot()).unwrap();
        assert_eq!(restored.previous(0), Transition::Play(manual[0]));
        restored.previous(0);
        assert_eq!(restored.current().unwrap().track.name, "a");
    }

    fn track(name: &str) -> Track {
        Track {
            uri: if name.starts_with("soundcloud:") {
                name.into()
            } else {
                format!("spotify:track:{name}")
            },
            name: name.into(),
            ..Default::default()
        }
    }

    fn tracks(names: &[&str]) -> Vec<Track> {
        names.iter().map(|name| track(name)).collect()
    }

    fn unavailable(name: &str) -> Track {
        Track {
            is_playable: Some(false),
            ..track(name)
        }
    }

    fn context(names: &[&str]) -> Context {
        Context {
            uri: "spotify:playlist:mixed".into(),
            name: "Mixed collection".into(),
            tracks: tracks(names),
        }
    }

    fn queue(names: &[&str]) -> HybridQueue {
        let mut queue = HybridQueue::new(42);
        queue.start(context(names), None, false, Repeat::Off);
        queue
    }

    fn names(entries: impl IntoIterator<Item = impl std::borrow::Borrow<Entry>>) -> Vec<String> {
        entries
            .into_iter()
            .map(|entry| entry.borrow().track.name.clone())
            .collect()
    }

    fn current(queue: &HybridQueue) -> Option<&str> {
        queue.current().map(|entry| entry.track.name.as_str())
    }

    fn upcoming_names(queue: &HybridQueue) -> Vec<String> {
        names(
            queue
                .playing_next(100)
                .into_iter()
                .chain(queue.next_up(100)),
        )
    }

    fn round_trip(queue: &HybridQueue) -> HybridQueue {
        let json = serde_json::to_string(&queue.snapshot()).unwrap();
        let restored = HybridQueue::restore(serde_json::from_str(&json).unwrap()).unwrap();
        assert_eq!(restored.snapshot(), queue.snapshot());
        restored
    }

    /// Rules 1, 3 and 4: shown rows are consumed in order, immediately.
    #[test]
    fn next_plays_the_shown_head_and_consumes_only_its_occurrence() {
        let mut queue = queue(&["a", "b", "c"]);
        let manual = queue.add_to_queue(tracks(&["x", "x", "y"]));
        assert_eq!(upcoming_names(&queue), ["x", "x", "y", "b", "c"]);
        assert_ne!(manual[0], manual[1]);
        for expected in ["x", "x", "y", "b", "c"] {
            let next_id = queue.next_entry().unwrap().id;
            assert_eq!(queue.next(), Transition::Play(next_id));
            assert_eq!(current(&queue), Some(expected));
            assert!(
                !queue
                    .playing_next(100)
                    .iter()
                    .any(|entry| entry.id == next_id)
            );
            assert!(!queue.next_up(100).iter().any(|entry| entry.id == next_id));
        }
        assert_eq!(names(queue.history()), ["a", "x", "x", "y", "b"]);
        assert_eq!(queue.next(), Transition::Stopped);
        assert!(queue.current().is_none());
        assert_eq!(queue.next(), Transition::Stopped);
    }

    /// Rule 2: batches append in displayed order, preserving copies.
    #[test]
    fn add_to_queue_appends_after_manual_songs_before_context() {
        let mut queue = queue(&["a", "b"]);
        let first = queue.add_to_queue(tracks(&["x"]));
        let batch = queue.add_to_queue(tracks(&["b", "c", "b"]));
        assert_eq!(current(&queue), Some("a"));
        assert_eq!(upcoming_names(&queue), ["x", "b", "c", "b", "b"]);
        let ids: HashSet<_> = first.into_iter().chain(batch).collect();
        assert_eq!(ids.len(), 4);
        assert!(
            queue
                .next_up(1)
                .iter()
                .all(|entry| !ids.contains(&entry.id))
        );
    }

    #[test]
    fn play_next_puts_the_whole_selection_first_in_given_order() {
        let mut queue = queue(&["a", "b"]);
        queue.add_to_queue(tracks(&["old", "last"]));
        let current_id = queue.current().unwrap().id;
        let ids = queue.play_next(tracks(&["x", "y", "x"]));
        assert_eq!(queue.current().unwrap().id, current_id);
        assert_eq!(upcoming_names(&queue), ["x", "y", "x", "old", "last", "b"]);
        assert_eq!(
            queue
                .playing_next(3)
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            ids
        );
        assert_ne!(ids[0], ids[2]);
        queue.play_next(tracks(&["new"]));
        assert_eq!(names(queue.playing_next(2)), ["new", "x"]);
    }

    /// Rule 5: skip_to is precisely repeated Next, not URI matching.
    #[test]
    fn skip_to_a_manual_copy_consumes_rows_above_and_keeps_rows_below() {
        let mut queue = queue(&["a", "b", "c"]);
        let ids = queue.add_to_queue(tracks(&["x", "x", "y"]));
        let mut repeated = queue.clone();
        repeated.next();
        repeated.next();
        assert_eq!(queue.skip_to(ids[1]), Some(Transition::Play(ids[1])));
        assert_eq!(queue.snapshot(), repeated.snapshot());
        assert_eq!(queue.current().unwrap().id, ids[1]);
        assert_eq!(upcoming_names(&queue), ["y", "b", "c"]);
    }

    #[test]
    fn skip_to_a_context_row_consumes_manual_and_preceding_context_rows() {
        let mut queue = queue(&["a", "b", "b", "c"]);
        queue.add_to_queue(tracks(&["manual"]));
        let id = queue.next_up(3)[1].id;
        let mut repeated = queue.clone();
        for _ in 0..3 {
            repeated.next();
        }
        assert_eq!(queue.skip_to(id), Some(Transition::Play(id)));
        assert_eq!(queue.snapshot(), repeated.snapshot());
        assert_eq!(
            queue.current().unwrap().origin,
            Origin::Context { index: 2 }
        );
        assert_eq!(upcoming_names(&queue), ["c"]);
    }

    #[test]
    fn skip_to_an_absent_or_consumed_id_is_a_no_op() {
        let mut queue = queue(&["a", "b"]);
        let snapshot = queue.snapshot();
        assert_eq!(queue.skip_to(EntryId(9999)), None);
        assert_eq!(queue.skip_to(queue.current().unwrap().id), None);
        assert_eq!(queue.snapshot(), snapshot);
    }

    /// Rules 6 and 7: starting a collection retains manually queued copies.
    #[test]
    fn starting_an_album_keeps_its_separately_queued_copies_and_clear_keeps_context() {
        let mut queue = queue(&["old", "older"]);
        let manual_ids = queue.add_to_queue(tracks(&["a", "b", "a"]));
        queue.start(context(&["a", "b", "a"]), None, false, Repeat::Context);
        assert_eq!(current(&queue), Some("a"));
        assert_eq!(queue.repeat(), Repeat::Context);
        assert_eq!(names(queue.playing_next(10)), ["a", "b", "a"]);
        assert_eq!(
            queue
                .playing_next(10)
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            manual_ids
        );
        assert!(queue.history().next().is_none());
        let remaining: Vec<_> = queue.next_up(10).into_iter().cloned().collect();
        let current = queue.current().cloned();
        queue.clear_queue();
        assert!(queue.playing_next(10).is_empty());
        assert_eq!(queue.current(), current.as_ref());
        assert_eq!(queue.next_up(10), remaining.iter().collect::<Vec<_>>());
        assert_eq!(queue.context().unwrap().name, "Mixed collection");
        assert_eq!(queue.context().unwrap().uri, "spotify:playlist:mixed");
    }

    /// Rule 8 is intrinsic: all views change synchronously, without reports.
    /// Rule 10's stale network responses cannot enter this I/O-free API.
    #[test]
    fn edits_and_shuffle_are_visible_synchronously() {
        let mut queue = queue(&["a", "b", "c", "d", "e", "f"]);
        let current = queue.current().cloned();
        let original = names(queue.next_up(100));
        let ids = queue.add_to_queue(tracks(&["x", "y"]));
        assert_eq!(names(queue.playing_next(2)), ["x", "y"]);
        queue.set_shuffle(true);
        assert_ne!(names(queue.next_up(100)), original);
        assert_eq!(queue.current(), current.as_ref());
        assert_eq!(names(queue.playing_next(2)), ["x", "y"]);
        queue.remove(&ids[..1]);
        assert_eq!(names(queue.playing_next(2)), ["y"]);
    }

    /// Rule 9: exact shuffle, manual occurrences, history, repeat and position
    /// survive serialization, even when the current occurrence is manual.
    #[test]
    fn snapshot_restores_a_manual_current_without_consuming_anything() {
        let mut queue = queue(&["a", "soundcloud:track:12", "b", "c", "d"]);
        queue.set_shuffle(true);
        queue.set_repeat(Repeat::Context);
        queue.add_to_queue(tracks(&["soundcloud:track:7", "x", "x"]));
        queue.next();
        queue.set_position_ms(12345);
        let mut restored = round_trip(&queue);
        assert_eq!(restored.position_ms(), 12345);
        assert_eq!(restored.current_source(), Some(Source::SoundCloud));
        for _ in 0..12 {
            assert_eq!(queue.next(), restored.next());
            assert_eq!(queue.snapshot(), restored.snapshot());
        }
        queue.set_shuffle(false);
        restored.set_shuffle(false);
        queue.set_shuffle(true);
        restored.set_shuffle(true);
        assert_eq!(queue.snapshot(), restored.snapshot());
        assert_eq!(
            queue.add_to_queue(tracks(&["new"])),
            restored.add_to_queue(tracks(&["new"]))
        );
    }

    /// Rule 11: only Playing next can be reordered.
    #[test]
    fn moving_a_manual_row_uses_occurrence_id_and_final_index() {
        let mut queue = queue(&["a", "b"]);
        let ids = queue.add_to_queue(tracks(&["x", "y", "x", "z"]));
        let context_id = queue.next_up(1)[0].id;
        assert!(!queue.move_entry(context_id, 0));
        assert!(!queue.move_entry(queue.current().unwrap().id, 0));
        assert!(!queue.move_entry(EntryId(999), 0));
        assert!(queue.move_entry(ids[2], 0));
        assert_eq!(
            queue
                .playing_next(4)
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            [ids[2], ids[0], ids[1], ids[3]]
        );
        assert!(queue.move_entry(ids[2], 2));
        assert_eq!(names(queue.playing_next(4)), ["x", "y", "x", "z"]);
        assert!(queue.move_entry(ids[0], usize::MAX));
        assert_eq!(names(queue.playing_next(4)), ["y", "x", "z", "x"]);
        assert_eq!(names(queue.next_up(4)), ["b"]);
    }

    /// Rule 12: removing one copy preserves the others and matching context.
    #[test]
    fn remove_takes_only_named_pending_manual_occurrences() {
        let mut queue = queue(&["a", "x"]);
        let ids = queue.add_to_queue(tracks(&["x", "y", "x"]));
        let context_id = queue.next_up(1)[0].id;
        queue.remove(&[
            ids[2],
            ids[2],
            context_id,
            queue.current().unwrap().id,
            EntryId(999),
        ]);
        assert_eq!(names(queue.playing_next(10)), ["x", "y"]);
        assert_eq!(names(queue.next_up(10)), ["x"]);
        queue.next();
        let snapshot = queue.snapshot();
        queue.remove(&[ids[0]]);
        assert_eq!(queue.snapshot(), snapshot);
    }

    #[test]
    fn starts_at_the_first_playable_row_without_losing_original_indices() {
        let mut queue = HybridQueue::new(0);
        let mut context = context(&["bad", "a", "a", "b"]);
        context.tracks[0].is_playable = Some(false);
        queue.start(context.clone(), None, false, Repeat::Off);
        assert_eq!(
            queue.current().unwrap().origin,
            Origin::Context { index: 1 }
        );
        assert_eq!(names(queue.next_up(10)), ["a", "b"]);
        queue.start(context.clone(), Some(2), false, Repeat::Track);
        assert_eq!(
            queue.current().unwrap().origin,
            Origin::Context { index: 2 }
        );
        assert_eq!(names(queue.next_up(10)), ["b"]);
        assert_eq!(queue.repeat(), Repeat::Track);
        queue.start(context, Some(0), false, Repeat::Off);
        assert_eq!(
            queue.current().unwrap().origin,
            Origin::Context { index: 1 }
        );
    }

    #[test]
    fn empty_unplayable_and_out_of_bounds_starts_do_not_fall_back() {
        let mut queue = queue(&["old"]);
        queue.add_to_queue(tracks(&["manual"]));
        for context in [
            context(&[]),
            Context {
                tracks: vec![unavailable("bad")],
                ..context(&[])
            },
        ] {
            assert_eq!(
                queue.start(context, None, true, Repeat::Context),
                Transition::Stopped
            );
            assert!(queue.current().is_none());
            assert!(queue.next_up(10).is_empty());
            round_trip(&queue);
        }
        for index in [1, 2, usize::MAX] {
            assert_eq!(
                queue.start(context(&["a"]), Some(index), false, Repeat::Off),
                Transition::Stopped
            );
            assert_eq!(names(queue.playing_next(10)), ["manual"]);
            assert!(queue.current().is_none());
            round_trip(&queue);
        }
    }

    #[test]
    fn unplayable_manual_items_are_filtered_and_unknown_availability_plays() {
        let mut queue = queue(&["a", "b"]);
        let ids = queue.add_to_queue(vec![
            unavailable("bad"),
            track("unknown"),
            unavailable("bad2"),
        ]);
        assert_eq!(ids.len(), 1);
        let ids = queue.play_next(vec![track("first"), unavailable("bad3"), track("last")]);
        assert_eq!(ids.len(), 2);
        assert_eq!(upcoming_names(&queue), ["first", "last", "unknown", "b"]);
    }

    #[test]
    fn shuffle_start_is_a_deterministic_permutation_of_occurrences() {
        for seed in 0..32 {
            let mut first = HybridQueue::new(seed);
            let mut second = HybridQueue::new(seed);
            let mut context = context(&["a", "b", "b", "soundcloud:track:8", "bad", "c"]);
            context.tracks[4].is_playable = Some(false);
            first.start(context.clone(), None, true, Repeat::Off);
            second.start(context, None, true, Repeat::Off);
            assert_eq!(first.snapshot(), second.snapshot());
            let ids: HashSet<_> = first
                .current()
                .into_iter()
                .chain(first.next_up(100))
                .map(|entry| entry.id)
                .collect();
            assert_eq!(ids.len(), 5);
            assert_eq!(first.state.order.len(), 5);
            let mut indices = first.state.order.clone();
            indices.sort_unstable();
            assert_eq!(indices, [0, 1, 2, 3, 5]);
        }
    }

    #[test]
    fn shuffle_double_click_starts_the_chosen_duplicate_then_shuffles_the_rest() {
        for seed in 0..32 {
            let mut queue = HybridQueue::new(seed);
            queue.start(context(&["a", "b", "a", "c"]), Some(2), true, Repeat::Off);
            assert_eq!(
                queue.current().unwrap().origin,
                Origin::Context { index: 2 }
            );
            let rest = queue.next_up(10);
            assert_eq!(rest.len(), 3);
            assert!(
                rest.iter()
                    .any(|entry| entry.origin == Origin::Context { index: 0 })
            );
            assert!(
                !rest
                    .iter()
                    .any(|entry| entry.id == queue.current().unwrap().id)
            );
            round_trip(&queue);
        }
    }

    #[test]
    fn toggling_shuffle_keeps_current_and_manual_ids_and_permutes_only_remainder() {
        let mut queue = queue(&["a", "b", "c", "d", "e", "f"]);
        queue.next();
        let current = queue.current().cloned();
        let remainder: HashSet<_> = queue.next_up(10).iter().map(|entry| entry.id).collect();
        let ids = queue.add_to_queue(tracks(&["x", "x"]));
        queue.set_shuffle(true);
        assert_eq!(queue.current(), current.as_ref());
        assert_eq!(
            queue
                .next_up(10)
                .iter()
                .map(|entry| entry.id)
                .collect::<HashSet<_>>(),
            remainder
        );
        assert_eq!(
            queue
                .playing_next(10)
                .iter()
                .map(|entry| entry.id)
                .collect::<Vec<_>>(),
            ids
        );
        let snapshot = queue.snapshot();
        queue.set_shuffle(true);
        assert_eq!(queue.snapshot(), snapshot);
        queue.set_shuffle(false);
        assert_eq!(names(queue.next_up(10)), ["c", "d", "e", "f"]);
        assert_eq!(queue.current(), current.as_ref());
    }

    #[test]
    fn turning_shuffle_off_during_a_manual_song_resumes_after_context_anchor() {
        let mut queue = HybridQueue::new(42);
        queue.start(
            context(&["a", "b", "c", "d", "e"]),
            Some(2),
            true,
            Repeat::Off,
        );
        let ids = queue.add_to_queue(tracks(&["manual", "remaining"]));
        queue.next();
        queue.set_shuffle(false);
        assert_eq!(queue.current().unwrap().id, ids[0]);
        assert_eq!(names(queue.next_up(10)), ["d", "e"]);
        assert_eq!(names(queue.playing_next(10)), ["remaining"]);
        round_trip(&queue);
    }

    #[test]
    fn repeat_off_stops_and_repeat_context_wraps_after_manual_items() {
        let mut queue = queue(&["a", "b"]);
        queue.next();
        queue.add_to_queue(tracks(&["manual"]));
        queue.set_repeat(Repeat::Context);
        assert_eq!(upcoming_names(&queue), ["manual", "a", "b"]);
        queue.end_of_track();
        assert_eq!(current(&queue), Some("manual"));
        queue.end_of_track();
        assert_eq!(current(&queue), Some("a"));
        assert!(queue.playing_next(10).is_empty());
        queue.set_repeat(Repeat::Off);
        queue.end_of_track();
        assert_eq!(current(&queue), Some("b"));
        assert_eq!(queue.end_of_track(), Transition::Stopped);
        round_trip(&queue);
    }

    #[test]
    fn repeat_track_restarts_but_next_advances_and_does_not_wrap() {
        let mut queue = queue(&["a", "b"]);
        queue.add_to_queue(tracks(&["soundcloud:track:1"]));
        queue.set_repeat(Repeat::Track);
        let id = queue.current().unwrap().id;
        queue.set_position_ms(9000);
        assert_eq!(queue.upcoming().unwrap().id, id);
        assert_eq!(queue.end_of_track(), Transition::Restart(id));
        assert_eq!(queue.position_ms(), 0);
        assert_eq!(queue.history().count(), 0);
        assert_eq!(names(queue.playing_next(10)), ["soundcloud:track:1"]);
        queue.next();
        let manual_id = queue.current().unwrap().id;
        assert_eq!(queue.end_of_track(), Transition::Restart(manual_id));
        assert_eq!(current(&queue), Some("soundcloud:track:1"));
        queue.next();
        assert_eq!(current(&queue), Some("b"));
        assert_eq!(queue.next(), Transition::Stopped);
    }

    #[test]
    fn repeating_a_single_context_occurrence_is_bounded_and_skippable() {
        let mut queue = queue(&["a"]);
        queue.set_repeat(Repeat::Context);
        let id = queue.current().unwrap().id;
        assert_eq!(queue.next_up(usize::MAX).len(), 1);
        assert_eq!(queue.skip_to(id), Some(Transition::Play(id)));
        assert_eq!(queue.history().count(), 1);
        round_trip(&queue);
    }

    #[test]
    fn shuffled_repeat_context_wraps_in_the_same_order_without_manual_replays() {
        let mut queue = HybridQueue::new(42);
        queue.start(context(&["a", "b", "c", "d"]), None, true, Repeat::Context);
        let order = queue.state.order.clone();
        queue.add_to_queue(tracks(&["manual"]));
        queue.next();
        for expected_index in order.iter().cycle().skip(1).take(12) {
            queue.next();
            assert_eq!(
                queue.current().unwrap().origin,
                Origin::Context {
                    index: *expected_index
                }
            );
        }
        assert_eq!(queue.state.order, order);
        assert!(queue.playing_next(100).is_empty());
        round_trip(&queue);
    }

    #[test]
    fn previous_restarts_only_above_three_seconds_and_visits_manual_history() {
        for repeat in [Repeat::Off, Repeat::Context, Repeat::Track] {
            let mut queue = queue(&["a", "b", "c"]);
            queue.set_repeat(repeat);
            let manual = queue.add_to_queue(tracks(&["manual"]))[0];
            queue.next();
            queue.next();
            queue.set_position_ms(9000);
            let current_id = queue.current().unwrap().id;
            let history = names(queue.history());
            assert_eq!(queue.previous(3001), Transition::Restart(current_id));
            assert_eq!(queue.position_ms(), 0);
            assert_eq!(names(queue.history()), history);
            assert_eq!(queue.previous(3000), Transition::Play(manual));
            assert_eq!(names(queue.next_up(2)), ["b", "c"]);
            let first = queue.history().next().unwrap().id;
            assert_eq!(queue.previous(0), Transition::Play(first));
            assert_eq!(names(queue.playing_next(10)), ["manual"]);
            assert_eq!(names(queue.next_up(2)), ["b", "c"]);
            assert_eq!(queue.previous(1), Transition::Restart(first));
            round_trip(&queue);
            assert_eq!(queue.next(), Transition::Play(manual));
            queue.next();
            assert_eq!(current(&queue), Some("b"));
        }
    }

    #[test]
    fn previous_through_several_context_and_manual_visits_keeps_manual_priority() {
        let mut queue = queue(&["a", "b", "c"]);
        queue.add_to_queue(tracks(&["m"]));
        queue.next();
        queue.next();
        queue.add_to_queue(tracks(&["n"]));
        queue.next();
        queue.next();
        for expected in ["n", "b", "m", "a"] {
            queue.previous(0);
            assert_eq!(current(&queue), Some(expected));
            round_trip(&queue);
        }
        assert_eq!(upcoming_names(&queue), ["m", "n", "b", "c"]);
    }

    #[test]
    fn previous_after_stopping_restores_the_last_visit_and_resets_position() {
        let mut queue = queue(&["a", "b"]);
        queue.next();
        let id = queue.current().unwrap().id;
        queue.next();
        assert_eq!(queue.previous(9999), Transition::Play(id));
        assert_eq!(current(&queue), Some("b"));
        assert_eq!(queue.position_ms(), 0);
        queue.set_shuffle(true);
        queue.previous(0);
        assert_eq!(current(&queue), Some("a"));
        round_trip(&queue);
    }

    #[test]
    fn previous_follows_occurrence_history_after_shuffle_order_changes() {
        for seed in 0..16 {
            let mut queue = HybridQueue::new(seed);
            queue.start(
                context(&["same", "soundcloud:track:2", "same", "last"]),
                Some(2),
                true,
                Repeat::Context,
            );
            let first = queue.current().unwrap().id;
            let manual = queue.add_to_queue(tracks(&["same"]))[0];
            queue.next();
            queue.next();
            let third = queue.current().unwrap().id;
            queue.set_shuffle(false);
            queue.next();
            for id in [third, manual, first] {
                assert_eq!(queue.previous(0), Transition::Play(id));
                assert_eq!(queue.current().unwrap().id, id);
                round_trip(&queue);
            }
            assert_eq!(
                names(queue.next_up(10)),
                ["last", "same", "soundcloud:track:2", "same"]
            );
            assert_eq!(queue.playing_next(10)[0].id, manual);
        }
    }

    #[test]
    fn clear_while_a_manual_copy_plays_keeps_current_history_and_context() {
        let mut queue = queue(&["a", "soundcloud:track:2", "a"]);
        let ids = queue.add_to_queue(tracks(&["a", "a", "soundcloud:track:2"]));
        queue.next();
        queue.set_position_ms(4000);
        let context = queue.context().cloned();
        let history = names(queue.history());
        queue.clear_queue();
        assert_eq!(queue.current().unwrap().id, ids[0]);
        assert_eq!(queue.position_ms(), 4000);
        assert_eq!(queue.context(), context.as_ref());
        assert_eq!(names(queue.history()), history);
        assert!(queue.playing_next(10).is_empty());
        assert_eq!(names(queue.next_up(10)), ["soundcloud:track:2", "a"]);
        round_trip(&queue);
        queue.next();
        assert_eq!(current(&queue), Some("soundcloud:track:2"));
    }

    #[test]
    fn source_queries_follow_mixed_context_manual_priority_and_repeat_track() {
        let mut queue = queue(&["spotify", "soundcloud:track:2", "last"]);
        assert_eq!(queue.current_source(), Some(Source::Spotify));
        assert_eq!(queue.upcoming_source(), Some(Source::SoundCloud));
        queue.add_to_queue(tracks(&["manual"]));
        assert_eq!(queue.upcoming_source(), Some(Source::Spotify));
        queue.play_next(tracks(&["soundcloud:track:4"]));
        assert_eq!(queue.upcoming_source(), Some(Source::SoundCloud));
        queue.set_repeat(Repeat::Track);
        assert_eq!(queue.upcoming_source(), Some(Source::Spotify));
        assert_eq!(queue.next_source(), Some(Source::SoundCloud));
        queue.next();
        assert_eq!(queue.current_source(), Some(Source::SoundCloud));
        queue.set_repeat(Repeat::Off);
        for expected in [Source::Spotify, Source::SoundCloud, Source::Spotify] {
            assert_eq!(queue.upcoming_source(), Some(expected));
            queue.next();
            assert_eq!(queue.current_source(), Some(expected));
        }
        assert_eq!(queue.upcoming_source(), None);
        queue.next();
        assert_eq!(queue.current_source(), None);
        assert_eq!(queue.next_source(), None);
    }

    #[test]
    fn next_source_skips_unavailable_rows_and_wraps_only_in_context_repeat() {
        let mut queue = HybridQueue::new(42);
        queue.start(
            Context {
                tracks: vec![
                    unavailable("bad"),
                    track("soundcloud:track:2"),
                    track("last"),
                ],
                ..context(&[])
            },
            None,
            false,
            Repeat::Off,
        );
        assert_eq!(queue.current_source(), Some(Source::SoundCloud));
        assert_eq!(queue.next_source(), Some(Source::Spotify));
        queue.next();
        assert_eq!(queue.next_source(), None);
        queue.set_repeat(Repeat::Track);
        assert_eq!(queue.next_source(), None);
        assert_eq!(queue.upcoming_source(), Some(Source::Spotify));
        queue.set_repeat(Repeat::Context);
        assert_eq!(queue.next_source(), Some(Source::SoundCloud));
        assert_eq!(queue.upcoming_source(), Some(Source::SoundCloud));
    }

    #[test]
    fn lookahead_limits_do_not_mutate_state() {
        let mut queue = queue(&["a", "b", "c"]);
        queue.add_to_queue(tracks(&["x", "y"]));
        let snapshot = queue.snapshot();
        assert!(queue.playing_next(0).is_empty());
        assert!(queue.next_up(0).is_empty());
        assert_eq!(names(queue.playing_next(1)), ["x"]);
        assert_eq!(names(queue.next_up(1)), ["b"]);
        assert_eq!(queue.snapshot(), snapshot);
    }

    #[test]
    fn empty_queue_and_stopped_manual_playback_round_trip() {
        let mut queue = HybridQueue::new(42);
        round_trip(&queue);
        assert_eq!(queue.next(), Transition::Stopped);
        assert_eq!(queue.previous(0), Transition::Stopped);
        queue.set_repeat(Repeat::Context);
        assert_eq!(queue.end_of_track(), Transition::Stopped);
        queue.set_position_ms(1234);
        assert_eq!(queue.position_ms(), 0);
        queue.set_shuffle(true);
        queue.add_to_queue(tracks(&["soundcloud:track:1"]));
        assert_eq!(queue.upcoming_source(), Some(Source::SoundCloud));
        queue.next();
        round_trip(&queue);
        queue.next();
        round_trip(&queue);
    }

    #[test]
    fn occurrence_ids_are_never_reused_on_context_changes_or_removal() {
        let mut queue = queue(&["a", "a"]);
        let mut ids: HashSet<_> = queue
            .context()
            .unwrap()
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect();
        let removed = queue.add_to_queue(tracks(&["a"]))[0];
        assert!(ids.insert(removed));
        queue.remove(&[removed]);
        queue.start(context(&["a", "a"]), None, true, Repeat::Off);
        for entry in &queue.context().unwrap().entries {
            assert!(ids.insert(entry.id));
        }
        let added = round_trip(&queue).add_to_queue(tracks(&["a"]))[0];
        assert!(ids.insert(added));
    }

    #[test]
    fn restore_rejects_unsupported_or_inconsistent_snapshots() {
        let mut queue = queue(&["a", "b"]);
        queue.add_to_queue(tracks(&["manual"]));
        let mut snapshot = queue.snapshot();
        snapshot.version = 2;
        assert_eq!(
            HybridQueue::restore(snapshot).unwrap_err(),
            RestoreError::UnsupportedVersion(2)
        );
        for corruption in 0..8 {
            let mut snapshot = queue.snapshot();
            match corruption {
                0 => snapshot.order.push(999),
                1 => snapshot.order[1] = snapshot.order[0],
                2 => snapshot.cursor = 999,
                3 => snapshot.manual[0].id = snapshot.current.as_ref().unwrap().entry.id,
                4 => snapshot.anchor = Some(EntryId(999)),
                5 => snapshot.next_id = 1,
                6 => snapshot.cursor = 0,
                _ => snapshot.current.as_mut().unwrap().entry.track.is_playable = Some(false),
            }
            assert!(
                HybridQueue::restore(snapshot).is_err(),
                "corruption {corruption}"
            );
        }
    }

    #[test]
    fn command_sequences_keep_snapshots_and_shown_play_order_consistent() {
        use rand::Rng;

        for seed in 0..16 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let mut queue = HybridQueue::new(seed);
            for _ in 0..128 {
                match rng.random_range(0..13) {
                    0 => {
                        queue.add_to_queue(tracks(&["x", "soundcloud:track:7", "x"]));
                    }
                    1 => {
                        queue.play_next(vec![track("y"), unavailable("bad"), track("y")]);
                    }
                    2 => {
                        let expected = queue.next_entry().map(|entry| entry.id);
                        let transition = queue.next();
                        assert_eq!(queue.current().map(|entry| entry.id), expected);
                        assert_eq!(
                            transition,
                            expected.map_or(Transition::Stopped, Transition::Play)
                        );
                    }
                    3 => {
                        let expected = queue.upcoming().map(|entry| entry.id);
                        queue.end_of_track();
                        assert_eq!(queue.current().map(|entry| entry.id), expected);
                    }
                    4 => {
                        queue.previous(rng.random_range(0..6001));
                    }
                    5 => queue.set_shuffle(!queue.shuffle()),
                    6 => queue.set_repeat(
                        [Repeat::Off, Repeat::Context, Repeat::Track][rng.random_range(0..3)],
                    ),
                    7 => {
                        let ids = queue
                            .playing_next(2)
                            .iter()
                            .map(|entry| entry.id)
                            .collect::<Vec<_>>();
                        queue.remove(&ids);
                    }
                    8 => {
                        if let Some(id) = queue.playing_next(3).last().map(|entry| entry.id) {
                            queue.move_entry(id, 0);
                        }
                    }
                    9 => queue.clear_queue(),
                    10 => {
                        let mut context = context(&["a", "soundcloud:track:1", "a", "bad", "b"]);
                        context.tracks[3].is_playable = Some(false);
                        queue.start(
                            context,
                            Some(rng.random_range(0..6)),
                            rng.random(),
                            queue.repeat(),
                        );
                    }
                    11 => {
                        if let Some(id) = queue.next_up(3).last().map(|entry| entry.id) {
                            queue.skip_to(id);
                        }
                    }
                    _ => queue.set_position_ms(rng.random_range(0..10000)),
                }
                round_trip(&queue);
            }
        }
    }
}
