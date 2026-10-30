//! Secondary SoundCloud following feed. Independent of Siphon's friends feed.
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::api::models::{Artist, Playlist, Track};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Posted,
    Reposted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Content {
    Track(Track),
    Playlist(Playlist),
}

impl Content {
    pub fn uri(&self) -> &str {
        match self {
            Self::Track(track) => &track.uri,
            Self::Playlist(playlist) => &playlist.uri,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub kind: Kind,
    pub by: Artist,
    pub created_at: String,
    pub content: Content,
}

impl Item {
    pub(crate) fn key(&self) -> (String, String, String, bool) {
        (
            self.by.uri.clone(),
            self.content.uri().to_owned(),
            self.created_at.clone(),
            self.kind == Kind::Reposted,
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snapshot {
    pub items: Vec<Item>,
    pub next: Option<String>,
    pub loaded: bool,
}

impl Snapshot {
    pub fn absorb(&mut self, page: super::models::Page<Item>, more: bool) {
        if !more {
            self.items.clear();
        }
        let mut seen: HashSet<_> = self.items.iter().map(Item::key).collect();
        self.items.extend(
            page.collection
                .into_iter()
                .filter(|item| seen.insert(item.key())),
        );
        self.next = page.next_href;
        self.loaded = true;
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub enum Status {
    #[default]
    ConnectSoundCloud,
    Idle,
    Loading,
    Ready,
    Error(String),
}

#[derive(Default)]
pub struct State {
    pub status: Status,
    pub serial: u64,
}

/// Unknown stream cards (for example recommendations) are not social posts.
/// Missing/deleted objects are skipped while retaining the page's cursor.
#[derive(Deserialize)]
pub(crate) struct WireItem {
    #[serde(rename = "type")]
    pub kind: String,
    pub created_at: Option<String>,
    pub user: Option<super::models::User>,
    pub track: Option<super::models::Track>,
    pub playlist: Option<super::models::Playlist>,
}

impl WireItem {
    pub fn into_item(self) -> Option<Item> {
        let (kind, content) = match self.kind.as_str() {
            "track" => (Kind::Posted, Content::Track(self.track?.to_track(None))),
            "track-repost" => (Kind::Reposted, Content::Track(self.track?.to_track(None))),
            "playlist" => (
                Kind::Posted,
                Content::Playlist(self.playlist?.to_playlist()),
            ),
            "playlist-repost" => (
                Kind::Reposted,
                Content::Playlist(self.playlist?.to_playlist()),
            ),
            _ => return None,
        };
        Some(Item {
            kind,
            by: self.user?.to_artist(),
            created_at: self.created_at?,
            content,
        })
    }
}
