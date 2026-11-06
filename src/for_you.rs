//! What Home recommends, across the sources, from what this listener actually
//! plays and likes. Neither SoundCloud nor YouTube offers a personal feed to an app
//! like this, so the taste is worked out here, from the listening log on this
//! computer and the likes the accounts sync, and each source is then asked what is
//! related to the songs at the top of it. Runs on the backend runtime.

mod projects;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub use projects::{Kind, Project};

use crate::api::models::{Album, PlayHistory, Track};
use crate::soundcloud::api::Client;
use crate::stats::Play;
use crate::youtube::Resolver;
use crate::youtube::browse;

/// Listening counts half as much after this many days.
const HALF_LIFE_DAYS: f64 = 30.0;
/// Less than this of a song is a skip, which counts against it.
const SKIP_MS: u32 = 30_000;
/// A song heard this recently is pushed down, so Home isn't what was just played.
const RECENT_DAYS: i64 = 14;
const LIKE_WEIGHT: f64 = 1.5;
const SKIP_WEIGHT: f64 = -0.5;
/// How many songs of each source are asked about.
const SOUNDCLOUD_SEEDS: usize = 4;
const YOUTUBE_SEEDS: usize = 3;
/// How many liked or played YouTube videos are checked for music to find those.
const YOUTUBE_SEEDS_CHECKED: usize = 9;
const RELATED_EACH: u32 = 15;
/// No artist fills the page.
const PER_ARTIST: usize = 2;
const MOST_PICKS: usize = 48;

/// One recommendation, and why it's here.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub track: Track,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct ForYou {
    /// Songs picked for the listener, best first. Never beats.
    pub picks: Vec<Pick>,
    /// Beats, apart from the songs: new ones from who's followed first, then picked.
    pub beats: Vec<Pick>,
    /// New from the people and channels followed, newest first.
    pub following: Vec<Track>,
    /// New albums, EPs and singles from the artists played most, on any source,
    /// best first. Never beats.
    pub projects: Vec<Project>,
    /// Which request this answers, so an older answer can't replace a newer one.
    pub asked: u64,
}

/// Whether a track is a beat (an instrumental made for others to rap or sing
/// on) rather than a song. Beats have a place of their own on Home.
///
/// Producers say so in the title: "type beat", "instrumental", "[FREE]",
/// "free for profit", a tempo. On YouTube, where the songs people play are
/// mostly on Spotify or SoundCloud too, an upload is a beat unless it says
/// it's a song's video or audio.
pub fn is_beat(track: &Track) -> bool {
    let title = track.name.to_lowercase();
    let words: Vec<&str> = title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let said = [
        "type beat",
        "typebeat",
        "instrumental",
        "free for profit",
        "freestyle beat",
    ]
    .iter()
    .any(|phrase| title.contains(phrase))
        || words
            .iter()
            .any(|word| matches!(*word, "beat" | "beats" | "bpm"))
        || title.trim_start().starts_with("[free]")
        || title.trim_start().starts_with("(free)");
    if said {
        return true;
    }
    let by_producer = track.artists.iter().any(|artist| {
        let name = artist.name.to_lowercase();
        name.split(|c: char| !c.is_alphanumeric())
            .any(|word| matches!(word, "beats" | "beatz" | "instrumentals"))
            || name.ends_with("beats")
    });
    let genre = crate::stats::soundcloud_genre(&track.uri).is_some_and(|genre| {
        let genre = genre.to_lowercase();
        genre.contains("beat") || genre.contains("instrumental")
    });
    if by_producer || genre {
        return true;
    }
    if crate::youtube::is_track_uri(&track.uri) {
        let song = [
            "official video",
            "official audio",
            "music video",
            "official visualizer",
            "visualizer",
            "lyric",
            "(audio)",
            "[audio]",
            "live at",
            "live from",
        ]
        .iter()
        .any(|marker| title.contains(marker));
        return !song;
    }
    false
}

/// What asking Spotify about an artist takes: the playback session, and the
/// client its private requests go through.
#[derive(Clone)]
pub struct SpotifyReads {
    pub session: librespot_core::Session,
    pub web: Arc<crate::api::ApiClient>,
}

#[derive(Clone, Debug, PartialEq)]
struct Seed {
    uri: String,
    /// Who it's by, which says why better than an upload's long title.
    who: String,
    weight: f64,
}

/// What the listener's history says about them.
#[derive(Clone, Debug, Default)]
struct Taste {
    seeds: Vec<Seed>,
    /// How much each artist is listened to, skips counted against. By lowercase name.
    artists: HashMap<String, f64>,
    recent: HashSet<String>,
    liked: HashSet<String>,
    /// How much of the listening each source gets.
    shares: HashMap<Source, f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Source {
    Spotify,
    SoundCloud,
    YouTube,
}

fn source(uri: &str) -> Source {
    if crate::soundcloud::is_track_uri(uri) {
        Source::SoundCloud
    } else if crate::youtube::is_track_uri(uri) {
        Source::YouTube
    } else {
        Source::Spotify
    }
}

fn taste(plays: &[Play], likes: &[Track], now: i64) -> Taste {
    let mut out = Taste::default();
    let mut songs: HashMap<String, Seed> = HashMap::new();
    for play in plays {
        if play.uri.is_empty() {
            continue;
        }
        let age_days = (now - play.at).max(0) as f64 / 86_400.0;
        let recency = 0.5_f64.powf(age_days / HALF_LIFE_DAYS);
        let skipped = play.ms < SKIP_MS;
        let weight = recency * if skipped { SKIP_WEIGHT } else { 1.0 };
        if !skipped && now - play.at < RECENT_DAYS * 86_400 {
            out.recent.insert(play.uri.clone());
        }
        if !skipped {
            *out.shares.entry(source(&play.uri)).or_default() += recency;
        }
        for artist in &play.artists {
            *out.artists.entry(artist.name.to_lowercase()).or_default() += weight;
        }
        songs
            .entry(play.uri.clone())
            .or_insert_with(|| Seed {
                uri: play.uri.clone(),
                who: play
                    .artists
                    .first()
                    .map_or_else(|| play.title.clone(), |artist| artist.name.clone()),
                weight: 0.0,
            })
            .weight += weight;
    }
    for track in likes {
        out.liked.insert(track.uri.clone());
        for artist in &track.artists {
            *out.artists.entry(artist.name.to_lowercase()).or_default() += LIKE_WEIGHT / 2.0;
        }
        songs
            .entry(track.uri.clone())
            .or_insert_with(|| Seed {
                uri: track.uri.clone(),
                who: track
                    .artists
                    .first()
                    .map_or_else(|| track.name.clone(), |artist| artist.name.clone()),
                weight: 0.0,
            })
            .weight += LIKE_WEIGHT;
        *out.shares.entry(source(&track.uri)).or_default() += LIKE_WEIGHT / 4.0;
    }
    out.seeds = songs
        .into_values()
        .filter(|seed| seed.weight > 0.0)
        .collect();
    out.seeds.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.uri.cmp(&b.uri))
    });
    out
}

/// A song on offer, how strongly it came recommended, and why.
#[derive(Clone, Debug)]
struct Candidate {
    track: Track,
    strength: f64,
    reason: String,
}

fn candidates_from(tracks: Vec<Track>, weight: f64, reason: &str) -> Vec<Candidate> {
    tracks
        .into_iter()
        .enumerate()
        .map(|(position, track)| Candidate {
            track,
            // A source's first answers are its best.
            strength: weight / (1.0 + 0.12 * position as f64),
            reason: reason.to_owned(),
        })
        .collect()
}

/// How much a song is worth to this listener.
fn score(candidate: &Candidate, taste: &Taste) -> f64 {
    let affinity: f64 = candidate
        .track
        .artists
        .iter()
        .filter_map(|artist| taste.artists.get(&artist.name.to_lowercase()))
        .sum();
    // Knowing an artist helps with diminishing returns; skipping them hurts fully.
    let affinity = if affinity > 0.0 {
        (1.0 + affinity).ln()
    } else {
        affinity
    };
    let mut value = candidate.strength + 0.5 * affinity;
    if taste.recent.contains(&candidate.track.uri) {
        value *= 0.2;
    }
    if taste.liked.contains(&candidate.track.uri) {
        value *= 0.3;
    }
    value
}

/// The page's order: the best first, no artist repeating too much, and each source
/// getting about the room it gets in the listening.
fn rank(candidates: Vec<Candidate>, taste: &Taste) -> Vec<Pick> {
    let mut best: HashMap<String, (f64, Candidate)> = HashMap::new();
    for candidate in candidates {
        if candidate.track.is_playable == Some(false) || candidate.track.uri.is_empty() {
            continue;
        }
        let value = score(&candidate, taste);
        match best.get(&candidate.track.uri) {
            Some((known, _)) if *known >= value => {}
            _ => {
                best.insert(candidate.track.uri.clone(), (value, candidate));
            }
        }
    }
    let total: f64 = taste.shares.values().sum::<f64>() + 1.0;
    let share = |source: Source| (taste.shares.get(&source).copied().unwrap_or(0.0) + 0.34) / total;
    let mut left: Vec<(f64, Candidate)> = best.into_values().collect();
    left.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.track.uri.cmp(&b.1.track.uri))
    });
    let mut picks = Vec::new();
    let mut by_artist: HashMap<String, usize> = HashMap::new();
    let mut by_source: HashMap<Source, usize> = HashMap::new();
    while picks.len() < MOST_PICKS {
        let lead = |candidate: &Candidate| {
            candidate
                .track
                .artists
                .first()
                .map(|artist| artist.name.to_lowercase())
                .unwrap_or_default()
        };
        let next = left
            .iter()
            .enumerate()
            .filter(|(_, (_, each))| by_artist.get(&lead(each)).copied().unwrap_or(0) < PER_ARTIST)
            .max_by(|(_, (a_value, a)), (_, (b_value, b))| {
                let room = |value: f64, each: &Candidate| {
                    let source = source(&each.track.uri);
                    let taken = by_source.get(&source).copied().unwrap_or(0) as f64;
                    // The further a source is past its share, the more it must earn.
                    value.max(0.01) * share(source) / (1.0 + taken)
                };
                room(*a_value, a)
                    .partial_cmp(&room(*b_value, b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(index, _)| index);
        let Some(index) = next else {
            break;
        };
        let (_, chosen) = left.remove(index);
        *by_artist.entry(lead(&chosen)).or_default() += 1;
        *by_source.entry(source(&chosen.track.uri)).or_default() += 1;
        picks.push(Pick {
            track: chosen.track,
            reason: chosen.reason,
        });
    }
    picks
}

/// What the app knows that isn't in the listening log.
pub struct Request {
    /// Which request this is; the answer says so.
    pub asked: u64,
    /// New SoundCloud uploads from the artists followed on Spotify.
    pub artist_news: Vec<Track>,
    pub likes: Vec<Track>,
    /// What Spotify suggests, which the taste then orders among the rest.
    pub spotify: Vec<Track>,
    /// New posts from the people followed on SoundCloud.
    pub soundcloud_feed: Vec<Track>,
    /// Of those, the songs they put out themselves rather than reposted.
    pub soundcloud_posts: Vec<Track>,
    pub channels: Vec<String>,
    /// The play history, local and Spotify's, with every song's accounts.
    pub heard: Vec<PlayHistory>,
    /// New releases from the artists followed on Spotify.
    pub spotify_new: Vec<Album>,
}

/// Gathers and orders the recommendations. Whatever a source fails to answer is
/// simply missing; the page shows what the others said.
pub async fn gather(
    client: Client,
    http: Option<reqwest::Client>,
    spotify: Option<SpotifyReads>,
    plays: Vec<Play>,
    request: Request,
    now: i64,
) -> ForYou {
    let taste = taste(&plays, &request.likes, now);
    let fans = projects::fans(&plays, &request.heard, &request.likes, now);
    let reads = projects::Reads {
        soundcloud: client.clone(),
        http: http.clone(),
        spotify,
    };
    let asked = projects::to_look_up(&fans, |source| reads.can_ask(source));
    let releases = tokio::spawn(projects::look_up_all(reads, asked));
    let resolver = Resolver::default();
    let mut reads = tokio::task::JoinSet::new();
    let of = |wanted: Source| {
        taste
            .seeds
            .iter()
            .filter(move |seed| source(&seed.uri) == wanted)
    };
    let soundcloud: Vec<Seed> = of(Source::SoundCloud)
        .take(SOUNDCLOUD_SEEDS)
        .cloned()
        .collect();
    // Only music is a seed: a mix made from a football clip is more football.
    let youtube: Vec<Seed> = match &http {
        Some(http) => {
            let checked: Vec<Seed> = of(Source::YouTube)
                .take(YOUTUBE_SEEDS_CHECKED)
                .cloned()
                .collect();
            let songs = checked
                .iter()
                .map(|seed| Track {
                    uri: seed.uri.clone(),
                    ..Default::default()
                })
                .collect();
            let music: HashSet<String> = crate::youtube::music::keep_music(http, songs)
                .await
                .into_iter()
                .map(|track| track.uri)
                .collect();
            checked
                .into_iter()
                .filter(|seed| music.contains(&seed.uri))
                .take(YOUTUBE_SEEDS)
                .collect()
        }
        None => Vec::new(),
    };
    // Only someone with nothing to go on at all starts from what's popular. Someone
    // who just doesn't use a source isn't new to music.
    let cold = taste.seeds.is_empty() && request.channels.is_empty() && request.likes.is_empty();
    let cold_soundcloud = cold;
    let cold_youtube = cold;
    for seed in soundcloud {
        let client = client.clone();
        reads.spawn(async move {
            let Some(id) = crate::soundcloud::track_id(&seed.uri) else {
                return Vec::new();
            };
            let Ok(page) = client.related_tracks(id, RELATED_EACH).await else {
                return Vec::new();
            };
            let tracks = page
                .collection
                .iter()
                .map(|track| track.to_track(None))
                .collect();
            candidates_from(
                tracks,
                seed.weight,
                &format!("Because you listen to {}", seed.who),
            )
        });
    }
    for seed in youtube {
        let resolver = resolver.clone();
        reads.spawn(async move {
            let Some(id) = crate::youtube::video_id(&seed.uri) else {
                return Vec::new();
            };
            let Ok(mix) = browse::page(&resolver, &browse::mix_uri(id), None).await else {
                return Vec::new();
            };
            // A mix opens on the song it was made from.
            let tracks = mix
                .tracks
                .into_iter()
                .filter(|track| track.uri != seed.uri)
                .collect();
            candidates_from(
                tracks,
                seed.weight,
                &format!("Because you listen to {}", seed.who),
            )
        });
    }
    // Nothing to go on yet: start from what's popular, and it learns from there.
    if cold_soundcloud {
        let client = client.clone();
        reads.spawn(async move {
            match client.trending(20).await {
                Ok(page) => {
                    let tracks = page
                        .collection
                        .iter()
                        .map(|item| item.track.to_track(None))
                        .collect();
                    candidates_from(tracks, 0.5, "Trending on SoundCloud")
                }
                Err(_) => Vec::new(),
            }
        });
    }
    if cold_youtube {
        let resolver = resolver.clone();
        reads.spawn(async move {
            let found = browse::search(&resolver, "type beat", None).await;
            candidates_from(found.videos, 0.5, "Popular beats on YouTube")
        });
    }
    let mut candidates = candidates_from(request.spotify, 1.0, "Suggested by Spotify");
    let mut following = request.soundcloud_feed;
    candidates.extend(candidates_from(
        following.clone(),
        1.2,
        "From someone you follow",
    ));
    while let Some(result) = reads.join_next().await {
        if let Ok(mut read) = result {
            candidates.append(&mut read);
        }
    }
    let mut offers: Vec<projects::Offer> = request
        .spotify_new
        .iter()
        .filter_map(projects::from_album)
        .chain(
            request
                .soundcloud_posts
                .into_iter()
                .chain(request.artist_news)
                .filter_map(projects::from_song),
        )
        .map(|project| projects::Offer {
            project,
            followed: true,
        })
        .collect();
    if let Some(http) = http
        && !request.channels.is_empty()
        && let Ok(uploads) = browse::feed(http.clone(), request.channels).await
    {
        // Channels are followed for all sorts of things; Home is for music.
        let uploads = crate::youtube::music::keep_music(&http, uploads.tracks).await;
        candidates.extend(candidates_from(
            uploads.clone(),
            1.2,
            "From a channel you follow",
        ));
        offers.extend(
            uploads
                .iter()
                .cloned()
                .filter_map(projects::from_song)
                .map(|project| projects::Offer {
                    project,
                    followed: true,
                }),
        );
        following.extend(uploads);
    }
    following.sort_by(|a, b| b.release_date.cmp(&a.release_date));
    offers.extend(
        releases
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|project| projects::Offer {
                project,
                followed: false,
            }),
    );
    // Beats and songs are ranked apart, so neither crowds the other out.
    let (beat_candidates, song_candidates): (Vec<Candidate>, Vec<Candidate>) = candidates
        .into_iter()
        .partition(|candidate| is_beat(&candidate.track));
    let (new_beats, projects): (Vec<Project>, Vec<Project>) = projects::rank(offers, &fans, now)
        .into_iter()
        .partition(|project| match &project.track {
            Some(track) => is_beat(track),
            None => is_beat(&Track {
                uri: project.uri.clone(),
                name: project.name.clone(),
                artists: project.artists.clone(),
                ..Default::default()
            }),
        });
    let mut beats: Vec<Pick> = new_beats
        .into_iter()
        // A beat put out as a set has no one song to play, so it stays out.
        .filter_map(|project| {
            Some(Pick {
                track: project.track?,
                reason: project.reason,
            })
        })
        .collect();
    for pick in rank(beat_candidates, &taste) {
        if !beats.iter().any(|known| known.track.uri == pick.track.uri) {
            beats.push(pick);
        }
    }
    beats.truncate(MOST_PICKS);
    ForYou {
        picks: rank(song_candidates, &taste),
        beats,
        following,
        projects,
        asked: request.asked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::ArtistRef;
    use crate::stats::PlayArtist;

    const NOW: i64 = 2_000_000_000;
    const DAY: i64 = 86_400;

    fn play(uri: &str, artist: &str, days_ago: i64, ms: u32) -> Play {
        Play {
            at: NOW - days_ago * DAY,
            ms,
            uri: uri.into(),
            title: uri.into(),
            artists: vec![PlayArtist {
                id: None,
                name: artist.into(),
            }],
            ..Default::default()
        }
    }

    fn track(uri: &str, artist: &str) -> Track {
        Track {
            uri: uri.into(),
            name: uri.into(),
            artists: vec![ArtistRef {
                name: artist.into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn offer(uri: &str, artist: &str, strength: f64) -> Candidate {
        Candidate {
            track: track(uri, artist),
            strength,
            reason: "why".into(),
        }
    }

    /// What `is_beat` decides, which the beat titles of `crate::beats` rely
    /// on: songs never read as beats, whatever the source, and beats say so.
    #[test]
    fn songs_say_so_and_beats_say_so_on_every_source() {
        let upload = |uri: &str, name: &str, artist: &str| Track {
            uri: uri.into(),
            name: name.into(),
            artists: vec![ArtistRef {
                name: artist.into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        // Songs.
        assert!(!is_beat(&upload(
            "spotify:track:beatTest",
            "Blinding Lights",
            "The Weeknd"
        )));
        assert!(!is_beat(&upload(
            "soundcloud:track:9200001",
            "Midnight City (feat. Rone)",
            "M83"
        )));
        assert!(!is_beat(&upload(
            "youtube:video:eHz0FSfSLP8",
            "Song (Official Video)",
            "Label"
        )));
        assert!(!is_beat(&upload(
            "youtube:video:eHz0FSfSLP8",
            "Song [Audio]",
            "Label"
        )));
        // Beats, said by the title.
        assert!(is_beat(&upload(
            "youtube:video:aB9_-012xyz",
            "[FREE] young nudy x lucki type beat \"tic tac\"",
            "ayohighflyer"
        )));
        assert!(is_beat(&upload(
            "youtube:video:aB9_-012xyz",
            "TRAP TYPE BEAT - \"SNAKE\"",
            "STORM"
        )));
        assert!(is_beat(&upload(
            "soundcloud:track:9200002",
            "(FREE FOR PROFIT) Soul Boom Bap x Chill Jazz Type Beat",
            "Rama Low"
        )));
        assert!(is_beat(&upload(
            "youtube:video:aB9_-012xyz",
            "Hard Freestyle Instrumental 140 BPM",
            "STORM"
        )));
        // Beats, said by the producer's own name or the upload's genre.
        assert!(is_beat(&upload(
            "soundcloud:track:9200003",
            "Smoked",
            "Sanche Beats"
        )));
        let genre = upload("soundcloud:track:9200004", "Smoked", "Rama Low");
        crate::stats::note_soundcloud_genre(&genre.uri, "Hip Hop Beats");
        assert!(is_beat(&genre));
        // A YouTube upload that says neither is the listener's beat to hear.
        assert!(is_beat(&upload(
            "youtube:video:aB9_-012xyz",
            "Late Night Loop",
            "Fixture Producer"
        )));
    }

    #[test]
    fn what_is_played_recently_and_finished_leads_the_seeds() {
        let taste = taste(
            &[
                play("soundcloud:track:1", "A", 200, 200_000),
                play("soundcloud:track:2", "B", 1, 200_000),
                play("soundcloud:track:2", "B", 2, 200_000),
                play("soundcloud:track:3", "C", 1, 5_000),
            ],
            &[],
            NOW,
        );
        let seeds: Vec<&str> = taste.seeds.iter().map(|seed| seed.uri.as_str()).collect();
        assert_eq!(seeds, ["soundcloud:track:2", "soundcloud:track:1"]);
        assert!(!seeds.contains(&"soundcloud:track:3"), "a skip is no seed");
        assert!(
            taste.artists["c"] < 0.0,
            "skipping counts against the artist"
        );
    }

    #[test]
    fn a_like_is_a_seed_before_any_play() {
        let taste = taste(&[], &[track("youtube:video:eHz0FSfSLP8", "Sanche")], NOW);
        assert_eq!(taste.seeds[0].uri, "youtube:video:eHz0FSfSLP8");
        assert!(taste.artists["sanche"] > 0.0);
    }

    #[test]
    fn artists_you_play_rise_and_artists_you_skip_sink() {
        let taste = taste(
            &[
                play("soundcloud:track:1", "Loved", 1, 200_000),
                play("soundcloud:track:2", "Loved", 2, 200_000),
                play("soundcloud:track:3", "Skipped", 1, 3_000),
                play("soundcloud:track:4", "Skipped", 2, 3_000),
            ],
            &[],
            NOW,
        );
        let picks = rank(
            vec![
                offer("soundcloud:track:10", "Skipped", 1.0),
                offer("soundcloud:track:11", "Stranger", 1.0),
                offer("soundcloud:track:12", "Loved", 1.0),
            ],
            &taste,
        );
        let order: Vec<&str> = picks.iter().map(|pick| pick.track.uri.as_str()).collect();
        assert_eq!(
            order,
            [
                "soundcloud:track:12",
                "soundcloud:track:11",
                "soundcloud:track:10"
            ]
        );
    }

    #[test]
    fn what_was_just_played_or_already_liked_gives_way() {
        let taste = taste(
            &[play("soundcloud:track:1", "A", 1, 200_000)],
            &[track("soundcloud:track:2", "B")],
            NOW,
        );
        let picks = rank(
            vec![
                offer("soundcloud:track:1", "X", 2.0),
                offer("soundcloud:track:2", "Y", 2.0),
                offer("soundcloud:track:3", "Z", 1.0),
            ],
            &taste,
        );
        assert_eq!(picks[0].track.uri, "soundcloud:track:3");
    }

    #[test]
    fn no_artist_fills_the_page_and_nothing_appears_twice() {
        let picks = rank(
            vec![
                offer("soundcloud:track:1", "Same", 3.0),
                offer("soundcloud:track:2", "Same", 2.9),
                offer("soundcloud:track:3", "Same", 2.8),
                offer("soundcloud:track:1", "Same", 1.0),
                offer("soundcloud:track:4", "Other", 0.1),
            ],
            &Taste::default(),
        );
        let same = picks
            .iter()
            .filter(|pick| pick.track.artists[0].name == "Same")
            .count();
        assert_eq!(same, PER_ARTIST);
        let uris: HashSet<&str> = picks.iter().map(|pick| pick.track.uri.as_str()).collect();
        assert_eq!(uris.len(), picks.len());
        assert!(uris.contains("soundcloud:track:4"));
    }

    #[test]
    fn the_source_listened_to_most_gets_the_most_room() {
        let plays: Vec<Play> = (0..12)
            .map(|n| {
                play(
                    &format!("soundcloud:track:{n}"),
                    &format!("a{n}"),
                    1,
                    200_000,
                )
            })
            .collect();
        let taste = taste(&plays, &[], NOW);
        let mut offers = Vec::new();
        for n in 0..10 {
            offers.push(offer(
                &format!("soundcloud:track:{}", 100 + n),
                &format!("s{n}"),
                1.0,
            ));
            offers.push(offer("", "nothing", 9.0));
            offers.push(offer(
                &format!("youtube:video:{n:0>11}"),
                &format!("y{n}"),
                1.0,
            ));
        }
        let picks = rank(offers, &taste);
        let first: Vec<_> = picks.iter().take(6).collect();
        let soundcloud = first
            .iter()
            .filter(|pick| crate::soundcloud::is_track_uri(&pick.track.uri))
            .count();
        assert!(soundcloud >= 4, "{soundcloud} of the first six");
        assert!(
            picks
                .iter()
                .any(|pick| crate::youtube::is_track_uri(&pick.track.uri)),
            "the other source still appears"
        );
        assert!(picks.iter().all(|pick| !pick.track.uri.is_empty()));
    }

    #[test]
    fn songs_that_cannot_play_are_left_out() {
        let mut blocked = offer("soundcloud:track:1", "A", 9.0);
        blocked.track.is_playable = Some(false);
        let picks = rank(
            vec![blocked, offer("soundcloud:track:2", "B", 1.0)],
            &Taste::default(),
        );
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].track.uri, "soundcloud:track:2");
    }

    /// Asks the real services what goes with one song on each:
    /// `cargo test --lib for_you_works_for_real -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the helper and the network"]
    async fn for_you_works_for_real() {
        let client = Client::new(reqwest::Client::new(), None, None);
        let found = client.search_tracks("flume", 1, 0).await.unwrap();
        let soundcloud = found.collection[0].to_track(None);
        let resolver = Resolver::default();
        let youtube = browse::search(&resolver, "dark type beat", None)
            .await
            .videos[0]
            .clone();
        let shown = gather(
            client,
            Some(reqwest::Client::new()),
            None,
            Vec::new(),
            Request {
                likes: vec![soundcloud, youtube],
                spotify: Vec::new(),
                soundcloud_feed: Vec::new(),
                soundcloud_posts: Vec::new(),
                channels: Vec::new(),
                heard: Vec::new(),
                spotify_new: Vec::new(),
                asked: 1,
                artist_news: Vec::new(),
            },
            NOW,
        )
        .await;
        for pick in shown.picks.iter().take(12) {
            println!(
                "{} — {} [{}]",
                pick.track.name, pick.track.artists[0].name, pick.reason
            );
        }
        assert!(
            shown
                .picks
                .iter()
                .any(|pick| crate::soundcloud::is_track_uri(&pick.track.uri))
        );
        assert!(
            shown
                .picks
                .iter()
                .any(|pick| crate::youtube::is_track_uri(&pick.track.uri))
        );
    }
}
