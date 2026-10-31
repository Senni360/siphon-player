//! A friend's share to carry a session on with, picked quietly by Jev.
//!
//! When the last song of a list starts and autoplay is on, the app asks the
//! group service which friend's Spotify share best follows what just played.
//! If Jev clearly prefers one, the list flows into that share when it ends
//! and Spotify's autoplay continues from it; otherwise autoplay continues
//! from the last song as before. Nothing is shown or asked.

use std::collections::HashMap;

use serde::Serialize;

use crate::credentials::{Slot, Store as CredentialStore};
use crate::http::Http;

use super::{Client, Identity, Post, Source};

/// How many shares Jev chooses between.
const CANDIDATES: usize = 50;
/// How many songs describe the session.
pub const RECENT: usize = 5;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Song {
    pub title: String,
    pub artists: Vec<String>,
}

impl Song {
    pub fn new<'a>(title: &str, artists: impl Iterator<Item = &'a str>) -> Self {
        Self {
            title: clean(title, 300),
            artists: artists
                .map(|artist| clean(artist, 200))
                .filter(|artist| !artist.is_empty())
                .take(5)
                .collect(),
        }
    }
}

/// Text the service accepts: no control characters, within its limit as
/// JavaScript counts length, in UTF-16 units.
fn clean(text: &str, max: usize) -> String {
    let mut units = 0;
    text.trim()
        .chars()
        .filter(|c| !c.is_control())
        .take_while(|c| {
            units += c.len_utf16();
            units <= max
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Candidate {
    /// The share's Spotify track URI.
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
}

/// The Spotify track a share links to, as a URI librespot can load.
pub fn track_uri(post: &Post) -> Option<String> {
    if post.item.source != Source::Spotify {
        return None;
    }
    let url = reqwest::Url::parse(&post.item.url).ok()?;
    let mut parts = url.path_segments()?.filter(|p| !p.starts_with("intl-"));
    let (Some("track"), Some(id), None) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then(|| format!("spotify:track:{id}"))
}

/// Friends' Spotify track shares, newest first, without the reader's own,
/// the ones they marked as quiet, or any song in `skip`.
pub fn candidates(posts: &[Post], me: &str, skip: &[&str]) -> Vec<Candidate> {
    let mut seen = std::collections::HashSet::new();
    posts
        .iter()
        .filter(|post| post.author_id != me && post.affinity.is_none_or(|p| p >= 35))
        .filter_map(|post| {
            let id = track_uri(post)?;
            let song = Song::new(
                &post.item.title,
                post.item.artists.iter().map(String::as_str),
            );
            (!song.title.is_empty() && !skip.contains(&id.as_str()) && seen.insert(id.clone()))
                .then_some(Candidate {
                    id,
                    title: song.title,
                    artists: song.artists,
                })
        })
        .take(CANDIDATES)
        .collect()
}

/// The share Jev clearly prefers: well above an even split, and ahead of
/// the runner-up. A close call carries on with Spotify's own autoplay.
pub fn choose(probabilities: &HashMap<String, f64>, candidates: usize) -> Option<String> {
    let mut ranked: Vec<(&String, f64)> = probabilities.iter().map(|(id, p)| (id, *p)).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    let (best, p) = *ranked.first()?;
    let runner_up = ranked.get(1).map_or(0.0, |r| r.1);
    // Between few shares a favourite must be clearer: three quarters of
    // the weight between two, a half from three on.
    let needed = (1.5 / candidates.max(1) as f64).clamp(0.5, 0.75);
    (p >= needed && p >= 2.0 * runner_up).then(|| best.clone())
}

/// This device's group identity and a client for its service, when it has
/// joined one.
pub async fn connect(credentials: &CredentialStore, http: Http) -> Option<(Client, Identity)> {
    let client = Client::configured(http).ok()?;
    let bytes = credentials
        .lease(Slot::Friends)
        .load_secret()
        .await
        .ok()??;
    Some((client, serde_json::from_slice(&bytes).ok()?))
}

/// Asks the group service, with this device's group identity. Any failure,
/// or no group at all, is no pick.
pub async fn ask(
    credentials: &CredentialStore,
    http: Http,
    recent: &[Song],
    candidates: &[Candidate],
) -> Option<String> {
    if recent.is_empty() || candidates.len() < 2 {
        return None;
    }
    let (client, identity) = connect(credentials, http).await?;
    let probabilities = client.next(&identity, recent, candidates).await.ok()??;
    choose(&probabilities, candidates.len())
}
