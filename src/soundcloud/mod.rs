//! SoundCloud beside Spotify.
//!
//! A SoundCloud track travels through the app as an ordinary
//! [`crate::api::models::Track`] whose `uri` is `soundcloud:track:<id>`, the
//! way a local file travels as `spotify:local:…`. Rows, the queue, drag and
//! drop, history and the player bar need no second track type; whatever
//! must behave differently asks [`is_track_uri`] and dispatches on it.
//!
//! - `api`: the SoundCloud API v2 client (search, tracks, playlists, users,
//!   likes, stream resolution). Runs on the backend runtime, never the UI
//!   thread.
//! - `engine`: plays a SoundCloud stream into the same audio sink librespot
//!   writes to, so EQ, visualisers, volume and the output device are shared.
//!
//! Mixed contexts (a playlist holding both sources) are played by the app's
//! own queue in `crate::hybrid_queue`, which hands each track to librespot or
//! to `engine` in turn. A context of Spotify tracks only keeps playing
//! through Spotify Connect exactly as before.
//!
//! No ads are requested: the stream URLs carry audio only, and the web
//! player's ad requests are never made. Go+ tracks that SoundCloud serves
//! only as a preview are shown and played as previews; the paywall is not
//! bypassed.

pub mod account;
pub mod api;
pub mod browse;
pub mod connect;
pub mod engine;
pub mod following;
pub mod library;
pub mod models;
pub mod reposts;

/// The URI scheme prefix of a SoundCloud track.
pub const TRACK_PREFIX: &str = "soundcloud:track:";
/// The URI scheme prefix of a SoundCloud playlist or album ("set").
pub const PLAYLIST_PREFIX: &str = "soundcloud:playlist:";
/// The URI scheme prefix of a SoundCloud user.
pub const USER_PREFIX: &str = "soundcloud:user:";

/// The URI of SoundCloud track `id`.
pub fn track_uri(id: u64) -> String {
    format!("{TRACK_PREFIX}{id}")
}

/// The URI of SoundCloud playlist `id`.
pub fn playlist_uri(id: u64) -> String {
    format!("{PLAYLIST_PREFIX}{id}")
}

/// The URI of SoundCloud user `id`.
pub fn user_uri(id: u64) -> String {
    format!("{USER_PREFIX}{id}")
}

/// Discover: what's trending, and SoundCloud's own selections.
pub const HOME: &str = "soundcloud:home";
/// What the people you follow posted and reposted.
pub const FEED: &str = "soundcloud:feed";
const CHARTS_PREFIX: &str = "soundcloud:charts:";
const SYSTEM_PREFIX: &str = "soundcloud:system:";
const RELATED_PREFIX: &str = "soundcloud:related:";

/// A profile's tabs other than its uploads, which are its plain URI.
pub const USER_TABS: [&str; 6] = [
    "popular",
    "albums",
    "playlists",
    "reposts",
    "likes",
    "following",
];

/// The genres SoundCloud keeps a trending playlist for, by its own key, for
/// Discover's chips.
pub const GENRES: [(&str, &str); 12] = [
    ("all-genres", "All"),
    ("hip-hop", "Hip-hop"),
    ("trap", "Trap"),
    ("r-n-b", "R&B"),
    ("electronic", "Electronic"),
    ("house", "House"),
    ("techno", "Techno"),
    ("dubstep", "Dubstep"),
    ("pop", "Pop"),
    ("indie", "Indie"),
    ("rock", "Rock"),
    ("ambient", "Ambient"),
];

/// The page of the chart of `genre`, one of [`GENRES`].
pub fn charts_uri(genre: &str) -> String {
    format!("{CHARTS_PREFIX}{genre}")
}

/// The genre a chart's page shows.
pub fn charts_genre(uri: &str) -> Option<&str> {
    let genre = uri.strip_prefix(CHARTS_PREFIX)?;
    GENRES.iter().any(|(key, _)| *key == genre).then_some(genre)
}

/// The page of one of SoundCloud's generated playlists.
pub fn system_uri(urn: &str) -> String {
    format!("{SYSTEM_PREFIX}{urn}")
}

/// The URN of a generated playlist's page. Only SoundCloud's own form of it is
/// taken, as it goes into a request's path.
pub fn system_urn(uri: &str) -> Option<&str> {
    let urn = uri.strip_prefix(SYSTEM_PREFIX)?;
    (urn.starts_with("soundcloud:system-playlists:")
        && urn.len() <= 200
        && urn
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_')))
    .then_some(urn)
}

/// The page of the songs SoundCloud relates to track `id`.
pub fn related_uri(id: u64) -> String {
    format!("{RELATED_PREFIX}{id}")
}

pub fn related_id(uri: &str) -> Option<u64> {
    uri.strip_prefix(RELATED_PREFIX)?.parse().ok()
}

/// A profile's URI and which of its tabs: `""` for its uploads.
pub fn user_tab(uri: &str) -> Option<(u64, &str)> {
    let rest = uri.strip_prefix(USER_PREFIX)?;
    let (id, tab) = rest.split_once(':').unwrap_or((rest, ""));
    (tab.is_empty() || USER_TABS.contains(&tab)).then_some((id.parse().ok()?, tab))
}

/// Whether `uri` is a page of SoundCloud browsing.
pub fn is_page(uri: &str) -> bool {
    uri == HOME
        || uri == FEED
        || playlist_id(uri).is_some()
        || user_tab(uri).is_some()
        || charts_genre(uri).is_some()
        || system_urn(uri).is_some()
        || related_id(uri).is_some()
}

/// Whether `uri` names a SoundCloud track.
pub fn is_track_uri(uri: &str) -> bool {
    track_id(uri).is_some()
}

/// Whether `uri` names anything on SoundCloud.
pub fn is_uri(uri: &str) -> bool {
    uri.starts_with("soundcloud:")
}

/// Only returned public SoundCloud permalinks may be opened or copied.
pub fn permalink(url: Option<&str>) -> Option<&str> {
    let url = url?;
    let parsed = reqwest::Url::parse(url).ok()?;
    (parsed.scheme() == "https"
        && matches!(
            parsed.host_str(),
            Some("soundcloud.com" | "www.soundcloud.com")
        ))
    .then_some(url)
}

/// The numeric id in a SoundCloud track URI.
pub fn track_id(uri: &str) -> Option<u64> {
    uri.strip_prefix(TRACK_PREFIX)?.parse().ok()
}

/// The numeric id in a SoundCloud playlist URI.
pub fn playlist_id(uri: &str) -> Option<u64> {
    uri.strip_prefix(PLAYLIST_PREFIX)?.parse().ok()
}

/// The numeric id in a SoundCloud user URI.
pub fn user_id(uri: &str) -> Option<u64> {
    uri.strip_prefix(USER_PREFIX)?.parse().ok()
}

/// Which service plays a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Spotify,
    YouTube,
    SoundCloud,
}

impl Source {
    pub fn of(uri: &str) -> Self {
        if crate::youtube::is_track_uri(uri) {
            Self::YouTube
        } else if is_uri(uri) {
            Self::SoundCloud
        } else {
            Self::Spotify
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_track_uri_round_trips_its_id() {
        let uri = track_uri(123456);
        assert_eq!(uri, "soundcloud:track:123456");
        assert_eq!(track_id(&uri), Some(123456));
        assert!(is_track_uri(&uri));
        assert_eq!(Source::of(&uri), Source::SoundCloud);
    }

    #[test]
    fn spotify_and_local_uris_are_not_soundcloud() {
        for uri in ["spotify:track:abc", "spotify:local:a:b:c:1", ""] {
            assert!(!is_uri(uri));
            assert_eq!(Source::of(uri), Source::Spotify);
        }
        assert_eq!(track_id("soundcloud:track:nope"), None);
        assert_eq!(track_id("soundcloud:playlist:5"), None);
        assert_eq!(playlist_id("soundcloud:playlist:5"), Some(5));
        assert_eq!(user_id(&user_uri(9)), Some(9));
    }
}
