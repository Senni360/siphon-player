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
        let likes = Likes::load(path).map_err(|_| {
            "Could not read YouTube likes. Your saved file has been kept.".to_owned()
        })?;
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
                "YouTube likes file needs a newer Siphon version",
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
