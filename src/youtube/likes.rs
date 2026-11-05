//! Account-scoped local likes. Encoding is pure; the app's private writer owns
//! atomic persistence. Reading is backend work and never runs during drawing.
use std::collections::HashMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::models::Track;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Likes {
    #[serde(default = "version")]
    pub version: u32,
    pub tracks: Vec<Track>,
    /// Channels followed here, newest first. There is no Google account behind
    /// them; they live only in this file, like the likes.
    #[serde(default)]
    pub channels: Vec<Channel>,
    /// Which of `tracks` are liked on the connected YouTube account, as last read.
    /// The rest are likes kept only here.
    #[serde(default)]
    pub remote_likes: Vec<String>,
}

/// A channel followed, as much of it as the library shows without asking YouTube.
impl Likes {
    /// Takes in what the account holds. The account decides everything it knows
    /// about, so something undone on YouTube goes here too. What was only ever kept
    /// here stays, until the listener chooses to send it up. Returns whether the
    /// channels changed.
    pub fn apply_remote(&mut self, channels: Vec<Channel>, tracks: Vec<Track>) -> bool {
        let before: Vec<String> = self.channels.iter().map(|each| each.id.clone()).collect();
        let local: Vec<Channel> = std::mem::take(&mut self.channels)
            .into_iter()
            .filter(|each| {
                each.subscription.is_none() && !channels.iter().any(|new| new.id == each.id)
            })
            .collect();
        let mut merged = channels;
        merged.extend(local);
        self.channels = merged;
        let remote: Vec<String> = tracks.iter().map(|track| track.uri.clone()).collect();
        let old_remote = std::mem::replace(&mut self.remote_likes, remote);
        let kept: Vec<Track> = std::mem::take(&mut self.tracks)
            .into_iter()
            .filter(|track| {
                !old_remote.contains(&track.uri) && !self.remote_likes.contains(&track.uri)
            })
            .collect();
        self.tracks = tracks;
        self.tracks.extend(kept);
        let after: Vec<&String> = self.channels.iter().map(|each| &each.id).collect();
        before.iter().collect::<Vec<_>>() != after
    }

    /// Likes kept only here, which the account doesn't have yet.
    pub fn local_likes(&self) -> Vec<Track> {
        self.tracks
            .iter()
            .filter(|track| !self.remote_likes.contains(&track.uri))
            .cloned()
            .collect()
    }

    /// Songs saved before channels were kept have no link to theirs. It is found
    /// through the video when the link is opened.
    fn link_channels(&mut self) {
        for track in &mut self.tracks {
            let Some(id) = super::video_id(&track.uri).map(str::to_owned) else {
                continue;
            };
            if let Some(artist) = track.artists.first_mut()
                && artist.uri.is_none()
            {
                artist.uri = Some(super::browse::video_channel_uri(&id));
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub image: Option<String>,
    /// The subscription on the connected YouTube account. Without one, the follow
    /// is kept only here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<String>,
}

const MAX_BYTES: u64 = 16 * 1024 * 1024;

fn version() -> u32 {
    1
}

pub const SAVE_ERROR: &str =
    "Could not save YouTube likes. Your changes have not been saved. Open YouTube Likes to retry.";
pub const RESTORE_WARNING: &str =
    "The previous YouTube likes save failed. Showing your last saved likes.";

#[derive(Clone, Debug, Default)]
pub struct Restored {
    pub likes: Likes,
    pub missed_save: bool,
}

impl From<Likes> for Restored {
    fn from(likes: Likes) -> Self {
        Self {
            likes,
            missed_save: false,
        }
    }
}

/// Status only, never track data. It outlives account switches so a failed
/// pending save cannot be mistaken for a successful restore after A -> B -> A.
#[derive(Default)]
pub struct Writes(Mutex<WriteState>);

#[derive(Default)]
struct WriteState {
    next: u64,
    accounts: HashMap<String, (u64, bool)>,
}

impl Writes {
    pub fn begin(&self, account: &str) -> u64 {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        state.next = state.next.wrapping_add(1);
        let revision = state.next;
        state
            .accounts
            .insert(account_key(account), (revision, false));
        revision
    }

    /// A failure from a replaced/coalesced save cannot poison a newer save.
    pub fn failed(&self, account: &str, revision: u64) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((current, failed)) = state.accounts.get_mut(&account_key(account))
            && *current == revision
        {
            *failed = true;
            return true;
        }
        false
    }

    /// Must run on a blocking worker. The same writer owns saves and the
    /// application's suspend/exit flush; no second persistence queue is added.
    pub fn load_after_writes(
        &self,
        writer: &fastframe_file::Writer,
        path: &Path,
        account: &str,
    ) -> Result<Restored, String> {
        if !writer.flush(fastframe_file::FLUSH_TIMEOUT) {
            return Err("YouTube likes are still being saved. Try again shortly.".into());
        }
        let failed = self
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .accounts
            .get(&account_key(account))
            .is_some_and(|(_, failed)| *failed);
        let mut likes = Likes::load(path).map_err(|_| {
            "Could not read YouTube likes. Your saved file has been kept.".to_owned()
        })?;
        likes.link_channels();
        Ok(Restored {
            likes,
            missed_save: failed,
        })
    }
}

impl Default for Likes {
    fn default() -> Self {
        Self {
            version: 1,
            tracks: Vec::new(),
            channels: Vec::new(),
            remote_likes: Vec::new(),
        }
    }
}

pub fn account_key(account: &str) -> String {
    format!("{:x}", Sha256::digest(account.as_bytes()))
}

pub fn path(state: &Path, account: &str) -> PathBuf {
    state
        .join("youtube-likes")
        .join(format!("{}.json", account_key(account)))
}

impl Likes {
    pub fn load(path: &Path) -> io::Result<Self> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() > MAX_BYTES {
            return Err(io::Error::other("YouTube likes file is too large"));
        }
        // Bound the actual read too: the file could grow after metadata().
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("YouTube likes file is too large"));
        }
        let mut likes: Self = serde_json::from_slice(&bytes).map_err(|_| {
            io::Error::other("YouTube likes file is unreadable; the original was kept")
        })?;
        if likes.version != 1 {
            return Err(io::Error::other(
                "YouTube likes file needs a newer Encore version",
            ));
        }
        if likes
            .tracks
            .iter()
            .any(|track| !super::is_track_uri(&track.uri))
        {
            return Err(io::Error::other(
                "YouTube likes contain unreadable tracks; the original was kept",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        likes.tracks.retain(|track| seen.insert(track.uri.clone()));
        Ok(likes)
    }

    pub fn encode(&self) -> serde_json::Result<Vec<u8>> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(serde_json::Error::io(io::Error::other(
                "YouTube likes file is too large",
            )));
        }
        Ok(bytes)
    }
}
