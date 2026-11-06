//! New projects for Home: the albums, EPs and singles just out from the artists
//! this listener plays, on whichever source they play them.
//!
//! Nothing here knows anyone's favourites. Each listener's plays and likes, on
//! Spotify, SoundCloud and YouTube alike, say which artists they care about and
//! where they listen to them. Each source is then asked about that listener's
//! artists there, in proportion to how much of their listening it gets, so
//! someone who lives on SoundCloud is served from SoundCloud and costs Spotify
//! nothing. What the accounts already follow comes for free on top. The newest
//! releases of the artists played most come first.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::api::models::{Album, ArtistRef, Image, PlayHistory, Track};
use crate::soundcloud::api::Client;
use crate::stats::Play;

/// Listening counts half as much after this many days.
const HALF_LIFE_DAYS: f64 = 30.0;
/// Less than this of a song is a skip, which counts against its artists.
const SKIP_MS: u32 = 30_000;
const SKIP_WEIGHT: f64 = -0.5;
const LIKE_WEIGHT: f64 = 0.75;
/// What a featured artist gets of a play, next to the lead's whole one.
const FEATURE_SHARE: f64 = 0.5;
/// A play in the listening log and one in the history this close together, of
/// the same song, are the same play.
const SAME_PLAY_SECONDS: i64 = 20 * 60;
/// How many artists are looked up at most, shared out among the sources by how
/// much of the listening each gets.
const ARTISTS_LOOKED_UP: usize = 64;
/// An artist heard less than this isn't worth asking about.
const LEAST_WEIGHT: f64 = 0.25;
/// What one artist's releases were found to be is kept this long: releases come
/// out once a day at most.
const LOOK_AGAIN: Duration = Duration::from_secs(3 * 60 * 60);
const MOST_AT_ONCE: usize = 4;
/// After this many days a release isn't new any more.
const NEW_DAYS: i64 = 42;
/// A release's pull halves every this many days, so this week's come first,
/// before last month's from someone played more.
const FRESH_HALF_LIFE_DAYS: f64 = 5.0;
/// What following an artist adds, next to how much they're played.
const FOLLOW_PULL: f64 = 0.5;
const PER_ARTIST: usize = 2;
const MOST_PROJECTS: usize = 24;
/// Shorter than this is a snippet, not a single.
const SHORTEST_SONG_MS: u32 = 60_000;
const SOUNDCLOUD_SETS: u32 = 4;
const SOUNDCLOUD_UPLOADS: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Album,
    Ep,
    Single,
}

/// A release Home recommends.
#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    /// What plays it: a Spotify album, a SoundCloud set, or a song on its own.
    pub uri: String,
    pub name: String,
    pub artists: Vec<ArtistRef>,
    pub images: Vec<Image>,
    pub kind: Kind,
    /// `YYYY-MM-DD`.
    pub released: String,
    pub songs: Option<u32>,
    /// A single that is one SoundCloud or YouTube song, played as itself.
    pub track: Option<Track>,
    pub reason: String,
}

impl Project {
    pub fn image(&self, px: u32) -> Option<&str> {
        crate::api::models::pick_image(&self.images, px)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Source {
    Spotify,
    SoundCloud,
    YouTube,
}

/// Where something plays from: songs, sets and accounts alike.
pub(super) fn source(uri: &str) -> Source {
    if crate::soundcloud::is_uri(uri) {
        Source::SoundCloud
    } else if uri.starts_with("youtube:") {
        Source::YouTube
    } else {
        Source::Spotify
    }
}

/// One artist as this listener knows them, across the sources.
#[derive(Clone, Debug, Default)]
pub(super) struct Fan {
    name: String,
    weight: f64,
    /// Their accounts on the sources they're played on.
    accounts: Vec<ArtistRef>,
}

/// What the listening says about the artists: who they are, by lowercase name,
/// and how much of it each source gets.
#[derive(Clone, Debug, Default)]
pub(super) struct Fans {
    by_name: HashMap<String, Fan>,
    shares: HashMap<Source, f64>,
}

fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// The account an artist credit points to, when a source can be asked about it.
fn account(artist: &ArtistRef) -> Option<ArtistRef> {
    let uri = artist.uri.as_deref()?;
    let usable = if let Some(id) = uri.strip_prefix("spotify:artist:") {
        id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
    } else if crate::soundcloud::user_id(uri).is_some() {
        true
    } else {
        crate::youtube::browse::channel_of(uri)
            .is_some_and(|id| id.starts_with("UC") && crate::youtube::browse::valid_channel(id))
    };
    usable.then(|| ArtistRef {
        id: artist
            .id
            .clone()
            .or_else(|| uri.strip_prefix("spotify:artist:").map(str::to_owned)),
        name: artist.name.clone(),
        uri: Some(uri.to_owned()),
    })
}

impl Fans {
    fn add(&mut self, artists: &[ArtistRef], weight: f64) {
        for (position, artist) in artists.iter().enumerate() {
            if artist.name.trim().is_empty() {
                continue;
            }
            let fan = self.by_name.entry(key(&artist.name)).or_default();
            if fan.name.is_empty() {
                fan.name = artist.name.clone();
            }
            fan.weight += if position == 0 {
                weight
            } else {
                weight * FEATURE_SHARE
            };
            if let Some(account) = account(artist)
                && !fan.accounts.iter().any(|known| known.uri == account.uri)
            {
                fan.accounts.push(account);
            }
        }
    }

    fn weight(&self, name: &str) -> f64 {
        self.by_name.get(&key(name)).map_or(0.0, |fan| fan.weight)
    }
}

fn recency(at: i64, now: i64) -> f64 {
    let age_days = (now - at).max(0) as f64 / 86_400.0;
    0.5_f64.powf(age_days / HALF_LIFE_DAYS)
}

/// The listening log, the play history and the likes, as artists. The log keeps
/// every play but names a Spotify artist only; the history keeps a song's
/// accounts on every source. A play in both counts once.
pub(super) fn fans(plays: &[Play], heard: &[PlayHistory], likes: &[Track], now: i64) -> Fans {
    let mut fans = Fans::default();
    let mut logged: HashMap<&str, Vec<i64>> = HashMap::new();
    for play in plays {
        if play.uri.is_empty() {
            continue;
        }
        logged.entry(play.uri.as_str()).or_default().push(play.at);
        let skipped = play.ms < SKIP_MS;
        let weight = recency(play.at, now) * if skipped { SKIP_WEIGHT } else { 1.0 };
        if !skipped {
            *fans.shares.entry(source(&play.uri)).or_default() += weight;
        }
        let artists: Vec<ArtistRef> = play
            .artists
            .iter()
            .map(|artist| ArtistRef {
                id: artist.id.clone(),
                name: artist.name.clone(),
                uri: artist
                    .id
                    .as_ref()
                    .filter(|_| play.uri.starts_with("spotify:"))
                    .map(|id| format!("spotify:artist:{id}")),
            })
            .collect();
        fans.add(&artists, weight);
    }
    for entry in heard {
        let track = &entry.track;
        let Some(at) = entry
            .played_at
            .as_deref()
            .and_then(|at| at.parse::<jiff::Timestamp>().ok())
            .map(|at| at.as_second())
        else {
            continue;
        };
        let counted = logged.get(track.uri.as_str()).is_some_and(|times| {
            times
                .iter()
                .any(|time| (time - at).abs() <= SAME_PLAY_SECONDS)
        });
        // Already counted, but its credits still say where its artists are.
        let weight = if counted { 0.0 } else { recency(at, now) };
        *fans.shares.entry(source(&track.uri)).or_default() += weight;
        fans.add(&track.artists, weight);
    }
    for track in likes {
        *fans.shares.entry(source(&track.uri)).or_default() += LIKE_WEIGHT / 4.0;
        fans.add(&track.artists, LIKE_WEIGHT);
    }
    fans
}

/// Which of the listener's artists to look up, on which source: the most played
/// first, each source getting as many as its share of the listening earns.
pub(super) fn to_look_up(fans: &Fans, can_ask: impl Fn(Source) -> bool) -> Vec<ArtistRef> {
    let total: f64 = fans.shares.values().filter(|share| **share > 0.0).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let mut ranked: Vec<&Fan> = fans
        .by_name
        .values()
        .filter(|fan| fan.weight >= LEAST_WEIGHT)
        .collect();
    ranked.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.name.cmp(&b.name))
    });
    let mut chosen = Vec::new();
    for wanted in [Source::Spotify, Source::SoundCloud, Source::YouTube] {
        let share = fans.shares.get(&wanted).copied().unwrap_or(0.0).max(0.0);
        if share <= 0.0 || !can_ask(wanted) {
            continue;
        }
        let room = (ARTISTS_LOOKED_UP as f64 * share / total).ceil() as usize;
        chosen.extend(
            ranked
                .iter()
                .filter_map(|fan| {
                    fan.accounts.iter().find(|account| {
                        account
                            .uri
                            .as_deref()
                            .is_some_and(|uri| source(uri) == wanted)
                    })
                })
                .take(room)
                .cloned(),
        );
    }
    chosen
}

/// A day as a number, from `YYYY-MM-DD`, `YYYY-MM` or a full timestamp. A year
/// alone says too little to call anything new.
fn day_of(date: &str) -> Option<i64> {
    let date = match date.len() {
        7 => format!("{date}-01"),
        n if n >= 10 => date[..10].to_owned(),
        _ => return None,
    };
    let date: jiff::civil::Date = date.parse().ok()?;
    Some(
        date.to_zoned(jiff::tz::TimeZone::UTC)
            .ok()?
            .timestamp()
            .as_second()
            .div_euclid(86_400),
    )
}

fn released(date: Option<&str>) -> Option<String> {
    let date = date?;
    day_of(date)?;
    Some(date.get(..10).unwrap_or(date).to_owned())
}

pub(super) fn from_album(album: &Album) -> Option<Project> {
    let songs = album.total_tracks;
    let kind = match album.album_type.as_deref()? {
        "album" => Kind::Album,
        "ep" => Kind::Ep,
        "single" if songs.is_some_and(|songs| songs >= 4) => Kind::Ep,
        "single" => Kind::Single,
        _ => return None,
    };
    Some(Project {
        uri: album.uri.clone(),
        name: album.name.clone(),
        artists: album.artists.clone(),
        images: album.images.clone(),
        kind,
        released: released(album.release_date.as_deref())?,
        songs,
        track: None,
        reason: String::new(),
    })
}

fn from_set(set: &crate::soundcloud::models::Playlist) -> Option<Project> {
    let kind = match set.set_type.as_deref() {
        Some("album") => Kind::Album,
        Some("ep") => Kind::Ep,
        Some("single") => Kind::Single,
        Some(_) => return None,
        None if set.is_album == Some(true) => Kind::Album,
        None => return None,
    };
    let shown = set.to_playlist();
    Some(Project {
        uri: shown.uri,
        name: set.title.clone(),
        artists: set
            .user
            .as_ref()
            .map(|user| vec![user.artist_ref(None)])
            .unwrap_or_default(),
        images: shown.images,
        kind,
        released: released(
            set.release_date
                .as_deref()
                .or(set.display_date.as_deref())
                .or(set.created_at.as_deref()),
        )?,
        songs: set.track_count,
        track: None,
        reason: String::new(),
    })
}

/// A song out on its own, as SoundCloud and YouTube put singles out.
pub(super) fn from_song(track: Track) -> Option<Project> {
    let name = track.name.to_lowercase();
    if track.is_playable == Some(false)
        || (track.duration_ms > 0 && track.duration_ms < SHORTEST_SONG_MS)
        || ["snippet", "preview"]
            .iter()
            .any(|word| name.contains(word))
    {
        return None;
    }
    Some(Project {
        uri: track.uri.clone(),
        name: track.name.clone(),
        artists: track.artists.clone(),
        images: track
            .album
            .as_ref()
            .map(|album| album.images.clone())
            .unwrap_or_default(),
        kind: Kind::Single,
        released: released(track.release_date.as_deref())?,
        songs: Some(1),
        track: Some(track),
        reason: String::new(),
    })
}

/// What the lookups can reach. Spotify's needs the playback session; YouTube's
/// needs a plain HTTP client.
#[derive(Clone)]
pub(super) struct Reads {
    pub soundcloud: Client,
    pub http: Option<reqwest::Client>,
    pub spotify: Option<super::SpotifyReads>,
}

impl Reads {
    pub fn can_ask(&self, wanted: Source) -> bool {
        match wanted {
            Source::Spotify => self.spotify.is_some(),
            Source::SoundCloud => true,
            Source::YouTube => self.http.is_some(),
        }
    }
}

/// Each account's releases as last found, by account URI.
type Found = (Instant, Vec<Project>);

static LOOKED_UP: LazyLock<Mutex<HashMap<String, Found>>> = LazyLock::new(Mutex::default);

fn remembered(uri: &str) -> Option<Vec<Project>> {
    LOOKED_UP
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(uri)
        .filter(|(at, _)| at.elapsed() < LOOK_AGAIN)
        .map(|(_, found)| found.clone())
}

/// One artist's newest releases on the source the account is on, or nothing if
/// the source didn't answer.
async fn look_up(reads: &Reads, artist: &ArtistRef) -> Option<Vec<Project>> {
    let uri = artist.uri.as_deref()?;
    match source(uri) {
        Source::Spotify => {
            let spotify = reads.spotify.as_ref()?;
            let albums =
                crate::session_reads::latest_releases(&spotify.session, &spotify.web, artist)
                    .await
                    .inspect_err(|error| log::debug!("artist releases unavailable: {error:#}"))
                    .ok()?;
            Some(albums.iter().filter_map(from_album).collect())
        }
        Source::SoundCloud => {
            let id = crate::soundcloud::user_id(uri)?;
            let (sets, uploads) = tokio::join!(
                reads.soundcloud.user_albums(id, SOUNDCLOUD_SETS),
                reads.soundcloud.user_tracks(id, SOUNDCLOUD_UPLOADS, 0)
            );
            if sets.is_err() && uploads.is_err() {
                return None;
            }
            let sets = sets.map(|page| page.collection).unwrap_or_default();
            // A song of a set came out with it, not on its own.
            let in_sets: HashSet<u64> = sets
                .iter()
                .flat_map(|set| set.tracks.iter().map(|track| track.id))
                .collect();
            let mut found: Vec<Project> = sets.iter().filter_map(from_set).collect();
            found.extend(
                uploads
                    .map(|page| page.collection)
                    .unwrap_or_default()
                    .iter()
                    .filter(|upload| !in_sets.contains(&upload.id))
                    .filter_map(|upload| from_song(upload.to_track(None))),
            );
            Some(found)
        }
        Source::YouTube => {
            let http = reads.http.as_ref()?;
            let channel = crate::youtube::browse::channel_of(uri)?.to_owned();
            let uploads = crate::youtube::browse::feed(http.clone(), vec![channel])
                .await
                .ok()?
                .tracks;
            let songs = crate::youtube::music::keep_music(http, uploads).await;
            Some(songs.into_iter().filter_map(from_song).collect())
        }
    }
}

/// The newest releases of the listener's artists, each source asked about the
/// artists played there. What was found lately is used again rather than asked.
pub(super) async fn look_up_all(reads: Reads, artists: Vec<ArtistRef>) -> Vec<Project> {
    let mut found = Vec::new();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(MOST_AT_ONCE));
    let mut asks = tokio::task::JoinSet::new();
    for artist in artists {
        let Some(uri) = artist.uri.clone() else {
            continue;
        };
        if let Some(mut known) = remembered(&uri) {
            found.append(&mut known);
            continue;
        }
        let reads = reads.clone();
        let semaphore = Arc::clone(&semaphore);
        asks.spawn(async move {
            let _permit = semaphore.acquire_owned().await.ok()?;
            let releases = look_up(&reads, &artist).await?;
            LOOKED_UP
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .insert(uri, (Instant::now(), releases.clone()));
            Some(releases)
        });
    }
    while let Some(asked) = asks.join_next().await {
        if let Ok(Some(mut releases)) = asked {
            found.append(&mut releases);
        }
    }
    found
}

/// A release on offer, and whether it came from something the account follows.
#[derive(Clone, Debug)]
pub(super) struct Offer {
    pub project: Project,
    pub followed: bool,
}

/// A title as it compares across sources: `Say So` and `say so!` are one.
fn plain(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn when(age: i64) -> String {
    match age {
        ..=0 => "Out today".into(),
        1 => "Out yesterday".into(),
        2..=13 => format!("Out {age} days ago"),
        _ => format!("Out {} weeks ago", age / 7),
    }
}

/// The page's order: the newest releases of the artists played most first, one
/// copy of each release, from the source the listener uses most, and no artist
/// more than twice.
pub(super) fn rank(offers: Vec<Offer>, fans: &Fans, now: i64) -> Vec<Project> {
    let today = now.div_euclid(86_400);
    let share = |uri: &str| fans.shares.get(&source(uri)).copied().unwrap_or(0.0);
    let mut kept: Vec<(f64, Offer)> = Vec::new();
    for offer in offers {
        let Some(day) = day_of(&offer.project.released) else {
            continue;
        };
        // A release out at midnight somewhere may be dated tomorrow here.
        let age = (today - day).max(0);
        if today - day < -1 || age > NEW_DAYS {
            continue;
        }
        let played = offer
            .project
            .artists
            .iter()
            .map(|artist| fans.weight(&artist.name))
            .fold(0.0, f64::max);
        let pull = played.max(0.0).ln_1p() + if offer.followed { FOLLOW_PULL } else { 0.0 };
        let size = match offer.project.kind {
            Kind::Album => 1.3,
            Kind::Ep => 1.15,
            Kind::Single => 1.0,
        };
        let value = 0.5_f64.powf(age as f64 / FRESH_HALF_LIFE_DAYS) * size * pull;
        if value <= 0.0 {
            continue;
        }
        let artist_keys: HashSet<String> = offer
            .project
            .artists
            .iter()
            .map(|artist| key(&artist.name))
            .collect();
        let same = kept.iter().position(|(_, known)| {
            known.project.uri == offer.project.uri
                || (plain(&known.project.name) == plain(&offer.project.name)
                    && known
                        .project
                        .artists
                        .iter()
                        .any(|artist| artist_keys.contains(&key(&artist.name))))
        });
        let Some(index) = same else {
            kept.push((value, offer));
            continue;
        };
        let (known_value, known) = &mut kept[index];
        let followed = known.followed || offer.followed;
        if known.project.uri == offer.project.uri {
            // The same release twice: a feed's credits are fuller than a lookup's.
            for artist in offer.project.artists {
                if !known
                    .project
                    .artists
                    .iter()
                    .any(|had| key(&had.name) == key(&artist.name))
                {
                    known.project.artists.push(artist);
                }
            }
        } else if share(&offer.project.uri) > share(&known.project.uri) {
            // On two sources: shown from the one this listener uses more.
            known.project = offer.project;
        }
        known.followed = followed;
        *known_value = known_value.max(value);
    }
    kept.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.project.uri.cmp(&b.1.project.uri))
    });
    let mut by_artist: HashMap<String, usize> = HashMap::new();
    let mut shown = Vec::new();
    for (_, offer) in kept {
        if shown.len() == MOST_PROJECTS {
            break;
        }
        let mut project = offer.project;
        let lead = project
            .artists
            .first()
            .map(|artist| key(&artist.name))
            .unwrap_or_default();
        let count = by_artist.entry(lead).or_default();
        if *count == PER_ARTIST {
            continue;
        }
        *count += 1;
        let age = day_of(&project.released).map_or(0, |day| (today - day).max(0));
        let favourite = project
            .artists
            .iter()
            .filter(|artist| fans.weight(&artist.name) > 0.0)
            .max_by(|a, b| {
                fans.weight(&a.name)
                    .partial_cmp(&fans.weight(&b.name))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        let why = match (favourite, project.artists.first()) {
            (Some(artist), _) => format!("you listen to {}", artist.name),
            (None, Some(artist)) => format!("you follow {}", artist.name),
            (None, None) => "from someone you follow".into(),
        };
        project.reason = format!("{} · {why}", when(age));
        shown.push(project);
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::PlayArtist;

    const DAY: i64 = 86_400;
    /// 2026-11-06, a Friday, at noon.
    const NOW: i64 = 1_793_966_400;

    fn date(days_ago: i64) -> String {
        jiff::Timestamp::from_second(NOW - days_ago * DAY)
            .unwrap()
            .to_zoned(jiff::tz::TimeZone::UTC)
            .date()
            .to_string()
    }

    fn artist(name: &str, uri: &str) -> ArtistRef {
        ArtistRef {
            id: uri.strip_prefix("spotify:artist:").map(str::to_owned),
            name: name.into(),
            uri: Some(uri.into()),
        }
    }

    fn heard(uri: &str, by: ArtistRef, days_ago: i64) -> PlayHistory {
        PlayHistory {
            track: Track {
                uri: uri.into(),
                name: uri.into(),
                artists: vec![by],
                ..Default::default()
            },
            played_at: Some(
                jiff::Timestamp::from_second(NOW - days_ago * DAY)
                    .unwrap()
                    .to_string(),
            ),
            context: None,
        }
    }

    fn release(uri: &str, name: &str, by: &str, kind: Kind, days_ago: i64) -> Offer {
        Offer {
            project: Project {
                uri: uri.into(),
                name: name.into(),
                artists: vec![ArtistRef {
                    name: by.into(),
                    ..Default::default()
                }],
                images: Vec::new(),
                kind,
                released: date(days_ago),
                songs: None,
                track: None,
                reason: String::new(),
            },
            followed: false,
        }
    }

    fn many(name: &str, uri: &str, times: i64, source_uri: &str) -> Vec<PlayHistory> {
        (0..times)
            .map(|n| heard(&format!("{source_uri}{n}"), artist(name, uri), n % 20))
            .collect()
    }

    const SPOTIFY_A: &str = "spotify:artist:0000000000000000000001";
    const SPOTIFY_B: &str = "spotify:artist:0000000000000000000002";

    #[test]
    fn the_artists_played_most_are_asked_about_on_the_sources_they_are_played_on() {
        let mut history = many("Big", SPOTIFY_A, 30, "spotify:track:a");
        history.extend(many("Cloud", "soundcloud:user:7", 10, "soundcloud:track:"));
        history.extend(many("Small", SPOTIFY_B, 1, "spotify:track:b"));
        let fans = fans(&[], &history, &[], NOW);
        let asked = to_look_up(&fans, |_| true);
        let uris: Vec<&str> = asked.iter().filter_map(|a| a.uri.as_deref()).collect();
        assert_eq!(uris, [SPOTIFY_A, SPOTIFY_B, "soundcloud:user:7"]);
        // Without Spotify's session, SoundCloud is still asked.
        let asked = to_look_up(&fans, |source| source != Source::Spotify);
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].uri.as_deref(), Some("soundcloud:user:7"));
    }

    #[test]
    fn a_source_gets_lookups_in_proportion_to_its_listening() {
        let mut history = Vec::new();
        for n in 0..90 {
            history.push(heard(
                &format!("soundcloud:track:{n}"),
                artist(&format!("sc{n}"), &format!("soundcloud:user:{n}")),
                1,
            ));
        }
        for n in 0..90 {
            history.push(heard(
                &format!("spotify:track:{n}"),
                artist(&format!("sp{n}"), &format!("spotify:artist:{n:0>22}")),
                1,
            ));
        }
        // Spotify gets a third of this listener's plays.
        history.retain(|entry| {
            !entry.track.uri.starts_with("spotify:")
                || entry.track.uri[14..].parse::<u32>().unwrap() < 45
        });
        let fans = fans(&[], &history, &[], NOW);
        let asked = to_look_up(&fans, |_| true);
        let spotify = asked
            .iter()
            .filter(|a| source(a.uri.as_deref().unwrap()) == Source::Spotify)
            .count();
        let soundcloud = asked.len() - spotify;
        assert!(
            soundcloud >= 2 * spotify - 2,
            "{soundcloud} against {spotify}"
        );
        assert!(asked.len() <= ARTISTS_LOOKED_UP + 3);
    }

    #[test]
    fn a_play_in_both_logs_counts_once_but_still_names_the_account() {
        let entry = heard(
            "soundcloud:track:1",
            artist("Gunnr", "soundcloud:user:9"),
            1,
        );
        let play = Play {
            at: NOW - DAY + 120,
            ms: 200_000,
            uri: "soundcloud:track:1".into(),
            title: "x".into(),
            artists: vec![PlayArtist {
                id: None,
                name: "Gunnr".into(),
            }],
            ..Default::default()
        };
        let fans = fans(&[play], &[entry], &[], NOW);
        let gunnr = &fans.by_name["gunnr"];
        assert!((gunnr.weight - recency(NOW - DAY + 120, NOW)).abs() < 1e-9);
        assert_eq!(gunnr.accounts[0].uri.as_deref(), Some("soundcloud:user:9"));
    }

    #[test]
    fn the_newest_from_the_artists_played_most_lead() {
        let mut history = many("Big", SPOTIFY_A, 40, "spotify:track:a");
        history.extend(many("Small", SPOTIFY_B, 2, "spotify:track:b"));
        let fans = fans(&[], &history, &[], NOW);
        let mut stranger = release("spotify:album:s", "Followed", "Stranger", Kind::Single, 0);
        stranger.followed = true;
        let shown = rank(
            vec![
                stranger,
                release("spotify:album:old", "Old", "Big", Kind::Album, 60),
                release("spotify:album:small", "Small one", "Small", Kind::Single, 2),
                release("spotify:album:big", "Big one", "Big", Kind::Single, 0),
                release("spotify:album:big2", "Big earlier", "Big", Kind::Single, 20),
                release(
                    "spotify:album:big3",
                    "Big earliest",
                    "Big",
                    Kind::Single,
                    30,
                ),
            ],
            &fans,
            NOW,
        );
        let names: Vec<&str> = shown.iter().map(|p| p.name.as_str()).collect();
        // Last week's from someone played a little beats last month's from a
        // favourite, and the earliest of three by one artist makes way.
        assert_eq!(names, ["Big one", "Small one", "Followed", "Big earlier"]);
        assert_eq!(shown[0].reason, "Out today · you listen to Big");
        assert_eq!(shown[2].reason, "Out today · you follow Stranger");
    }

    #[test]
    fn a_release_on_two_sources_shows_once_from_the_one_used_more() {
        let history = many("Gunnr", "soundcloud:user:9", 10, "soundcloud:track:");
        let fans = fans(&[], &history, &[], NOW);
        let shown = rank(
            vec![
                release("spotify:album:x", "stargirl", "Gunnr", Kind::Single, 0),
                release("soundcloud:track:5", "Stargirl!", "gunnr", Kind::Single, 0),
            ],
            &fans,
            NOW,
        );
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].uri, "soundcloud:track:5");
    }

    #[test]
    fn snippets_and_vague_dates_are_no_release() {
        let mut snippet = Track {
            uri: "soundcloud:track:1".into(),
            duration_ms: 20_000,
            release_date: Some(date(0)),
            ..Default::default()
        };
        assert!(from_song(snippet.clone()).is_none());
        snippet.duration_ms = 150_000;
        snippet.name = "No One But U (snippet version)".into();
        assert!(from_song(snippet.clone()).is_none());
        snippet.name = "No One But U".into();
        assert!(from_song(snippet.clone()).is_some());
        snippet.release_date = Some("2026".into());
        assert!(from_song(snippet).is_none());
        assert_eq!(day_of("2026-11-06T04:33:51Z"), day_of("2026-11-06"));
    }
}

/// Runs the lookups on this computer's own history, without Spotify's session:
/// `SIPHON_STATE=<state dir> cargo test --lib projects_for_this_computer -- --ignored --nocapture`
#[cfg(test)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a profile and the network"]
async fn projects_for_this_computer() {
    let state = std::path::PathBuf::from(std::env::var("SIPHON_STATE").unwrap());
    let heard: Vec<PlayHistory> =
        serde_json::from_str(&std::fs::read_to_string(state.join("history.json")).unwrap())
            .unwrap();
    let plays = crate::stats::read_all(&state.join("listening"));
    let now = jiff::Timestamp::now().as_second();
    let fans = fans(&plays, &heard, &[], now);
    let mut shares: Vec<_> = fans.shares.iter().collect();
    shares.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap());
    println!("shares {shares:?}");
    let all = to_look_up(&fans, |_| true);
    println!(
        "would ask {}: {:?}",
        all.len(),
        all.iter().map(|a| a.name.as_str()).collect::<Vec<_>>()
    );
    let reads = Reads {
        soundcloud: Client::new(reqwest::Client::new(), None, None),
        http: Some(reqwest::Client::new()),
        spotify: None,
    };
    let asked = to_look_up(&fans, |source| reads.can_ask(source));
    let found = look_up_all(reads, asked).await;
    let mut offers: Vec<Offer> = found
        .into_iter()
        .map(|project| Offer {
            project,
            followed: false,
        })
        .collect();
    // Spotify's answers, which need the session, can be stood in for by a file of
    // albums: `SIPHON_SPOTIFY=<albums.json>`, followed with `SIPHON_FOLLOWED`.
    for (variable, followed) in [("SIPHON_SPOTIFY", false), ("SIPHON_FOLLOWED", true)] {
        if let Ok(path) = std::env::var(variable) {
            let albums: Vec<Album> =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            offers.extend(
                albums
                    .iter()
                    .filter_map(from_album)
                    .map(|project| Offer { project, followed }),
            );
        }
    }
    for project in rank(offers, &fans, now) {
        println!(
            "{:<40} {:<20} {:?} {} [{}] {}",
            project.name,
            project.artists[0].name,
            project.kind,
            project.released,
            project.reason,
            project.uri
        );
    }
}
