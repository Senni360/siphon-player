//! Account-scoped local likes. Encoding is pure; the app's private writer owns
//! atomic persistence. Reading is backend work and never runs during drawing.
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::models::Track;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Likes {
    pub version: u32,
    pub tracks: Vec<Track>,
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
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        if metadata.len() > 16 * 1024 * 1024 {
            return Err(io::Error::other("YouTube likes file is too large"));
        }
        let bytes = std::fs::read(path)?;
        let mut likes: Self = serde_json::from_slice(&bytes).map_err(|_| {
            io::Error::other("YouTube likes file is unreadable; the original was kept")
        })?;
        if likes.version != 1 {
            return Err(io::Error::other(
                "YouTube likes file needs a newer Siphon version",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        likes
            .tracks
            .retain(|track| super::is_track_uri(&track.uri) && seen.insert(track.uri.clone()));
        Ok(likes)
    }

    pub fn encode(&self) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec(self)
    }
}
