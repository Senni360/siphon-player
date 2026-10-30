//! Session-only repost membership and optimistic write overlays.
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Target {
    Track(u64),
    Playlist(u64),
}

impl Target {
    pub fn from_uri(uri: &str) -> Option<Self> {
        super::track_id(uri)
            .map(Self::Track)
            .or_else(|| super::playlist_id(uri).map(Self::Playlist))
    }

    pub fn id(self) -> u64 {
        match self {
            Self::Track(id) | Self::Playlist(id) => id,
        }
    }

    pub(crate) fn path(self) -> &'static str {
        match self {
            Self::Track(_) => "me/track_reposts",
            Self::Playlist(_) => "me/playlist_reposts",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub tracks: HashSet<u64>,
    pub playlists: HashSet<u64>,
    pub loaded: bool,
}

impl Snapshot {
    pub fn contains(&self, target: Target) -> bool {
        match target {
            Target::Track(id) => self.tracks.contains(&id),
            Target::Playlist(id) => self.playlists.contains(&id),
        }
    }

    fn set(&mut self, target: Target, reposted: bool) {
        let ids = match target {
            Target::Track(_) => &mut self.tracks,
            Target::Playlist(_) => &mut self.playlists,
        };
        if reposted {
            ids.insert(target.id());
        } else {
            ids.remove(&target.id());
        }
    }
}

#[derive(Default)]
pub struct Edits {
    // Keep successful writes for the connection's lifetime, just like likes:
    // an older concurrent read must never undo an acknowledged write.
    values: HashMap<Target, bool>,
    pending: HashMap<Target, Pending>,
    serial: u64,
}

struct Pending {
    serial: u64,
    before: bool,
}

impl Edits {
    pub fn pending(&self, target: Target) -> bool {
        self.pending.contains_key(&target)
    }

    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn overlay(&self, snapshot: &mut Snapshot) {
        for (&target, &reposted) in &self.values {
            snapshot.set(target, reposted);
        }
    }

    pub fn toggle(&mut self, snapshot: &mut Snapshot, target: Target) -> Option<(u64, bool)> {
        if !snapshot.loaded || self.pending(target) {
            return None;
        }
        let before = snapshot.contains(target);
        self.serial = self.serial.wrapping_add(1);
        self.pending.insert(
            target,
            Pending {
                serial: self.serial,
                before,
            },
        );
        self.values.insert(target, !before);
        snapshot.set(target, !before);
        Some((self.serial, !before))
    }

    /// Ignore stale completions. A failed write rolls back once, with an error.
    pub fn finish(
        &mut self,
        snapshot: &mut Snapshot,
        target: Target,
        serial: u64,
        success: bool,
    ) -> bool {
        if self
            .pending
            .get(&target)
            .is_none_or(|pending| pending.serial != serial)
        {
            return false;
        }
        let pending = self.pending.remove(&target).unwrap();
        if !success {
            self.values.insert(target, pending.before);
            snapshot.set(target, pending.before);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_hold_over_stale_reads_and_only_matching_failure_rolls_back() {
        let mut snapshot = Snapshot {
            loaded: true,
            ..Default::default()
        };
        let mut edits = Edits::default();
        let target = Target::Track(1);
        let (serial, reposted) = edits.toggle(&mut snapshot, target).unwrap();
        assert!(reposted && snapshot.contains(target));
        assert!(edits.toggle(&mut snapshot, target).is_none());
        let mut stale = Snapshot {
            loaded: true,
            ..Default::default()
        };
        edits.overlay(&mut stale);
        assert!(stale.contains(target));
        assert!(edits.finish(&mut snapshot, target, serial, true));
        let (next, reposted) = edits.toggle(&mut snapshot, target).unwrap();
        assert!(!reposted && !snapshot.contains(target));
        assert!(!edits.finish(&mut snapshot, target, serial, false));
        assert!(!snapshot.contains(target));
        assert!(edits.finish(&mut snapshot, target, next, false));
        assert!(snapshot.contains(target));
        edits.overlay(&mut stale);
        assert!(stale.contains(target));
        assert!(!stale.contains(Target::Playlist(1)));
    }

    #[test]
    fn unknown_membership_and_other_sources_cannot_be_toggled() {
        assert!(
            Edits::default()
                .toggle(&mut Snapshot::default(), Target::Playlist(1))
                .is_none()
        );
        for uri in [
            "spotify:track:1",
            "soundcloud:user:1",
            "soundcloud:track:bad",
        ] {
            assert_eq!(Target::from_uri(uri), None);
        }
    }
}
