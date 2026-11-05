//! One artist across the sources. An artist's Spotify page also lists what they
//! have only on SoundCloud or YouTube, so the page has to work out which accounts
//! are theirs and which uploads Spotify doesn't already have.
//!
//! Nothing here replaces a Spotify song: an upload that Spotify has is left out, and
//! each song plays from where it came from. The matching is plain rules over names
//! and titles, kept here apart from the reading so they can be tested, and it's
//! allowed to say it isn't sure: the page then asks instead of guessing.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::api::models::{Album, Track};
use crate::model::SourceCard;
use crate::soundcloud::api::Client;
use crate::soundcloud::models;
use crate::youtube::Resolver;
use crate::youtube::browse;

const USER_SEARCH: u32 = 12;
const SOUNDCLOUD_LOOKS: usize = 3;
const YOUTUBE_LOOKS: usize = 2;
const SOUNDCLOUD_UPLOADS: u32 = 100;
const EDIT_SEARCH: u32 = 30;
const MOST_CANDIDATES: usize = 4;
const MOST_EDITS: usize = 30;
/// Shorter than this on YouTube is a Short or a trailer, not a song.
const SHORTEST_VIDEO_MS: u32 = 45_000;
/// Longer than this on YouTube is an album upload, a set or an interview.
const LONGEST_VIDEO_MS: u32 = 15 * 60_000;
/// How long a video with nothing else to go on has to be, to count as a song.
const SONG_MS: std::ops::RangeInclusive<u32> = 90_000..=7 * 60_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    SoundCloud,
    YouTube,
}

/// Someone's account on a source other than Spotify.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// The page it opens.
    pub uri: String,
    /// The source's own id, to follow it by.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub followers: Option<u64>,
    #[serde(default)]
    pub verified: bool,
}

/// How sure the account is the artist's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Confidence {
    Unsure,
    Likely,
    Sure,
    /// The listener said so, which nothing overrides.
    Chosen,
}

/// What one source has of an artist.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    #[serde(default)]
    pub account: Option<Account>,
    #[serde(default)]
    pub confidence: Option<Confidence>,
    /// Who else it might be, or, with no account, who to ask about.
    #[serde(default)]
    pub candidates: Vec<Account>,
    /// The listener said this artist isn't on this source.
    #[serde(default)]
    pub not_there: bool,
    /// Their uploads that Spotify doesn't have.
    #[serde(default)]
    pub exclusives: Vec<Track>,
    /// Remixes, sped up versions and the like, mostly by others.
    #[serde(default)]
    pub edits: Vec<Track>,
    /// How many of their uploads Spotify does have.
    #[serde(default)]
    pub on_spotify: usize,
    #[serde(default)]
    pub error: Option<String>,
}

/// What the listener or a search settled for one source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Link {
    Found(Account),
    Chosen(Account),
    NotThere,
}

/// Which accounts belong to which Spotify artists, kept readable and additive in
/// `artist-links.json`. A listener's choice is never replaced by a search.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Links {
    #[serde(default)]
    pub artists: HashMap<String, Linked>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Linked {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub soundcloud: Option<Link>,
    #[serde(default)]
    pub youtube: Option<Link>,
}

impl Linked {
    pub fn side(&self, side: Side) -> Option<&Link> {
        match side {
            Side::SoundCloud => self.soundcloud.as_ref(),
            Side::YouTube => self.youtube.as_ref(),
        }
    }

    pub fn side_mut(&mut self, side: Side) -> &mut Option<Link> {
        match side {
            Side::SoundCloud => &mut self.soundcloud,
            Side::YouTube => &mut self.youtube,
        }
    }
}

impl Links {
    /// Whose page each account's page leads back to: its URI to the Spotify artist
    /// id and name. Only accounts a search was sure of or the listener chose are here.
    pub fn back(&self) -> HashMap<String, (String, String)> {
        let mut back = HashMap::new();
        for (id, linked) in &self.artists {
            for link in [&linked.soundcloud, &linked.youtube].into_iter().flatten() {
                if let Link::Found(account) | Link::Chosen(account) = link {
                    back.insert(account.uri.clone(), (id.clone(), linked.name.clone()));
                }
            }
        }
        back
    }
}

/// Lowercased, accents off, `$` read as the S it stands for, and anything else that
/// isn't a letter or digit a space.
fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        for lower in character.to_lowercase() {
            out.push(match lower {
                'á' | 'à' | 'ä' | 'â' | 'ã' | 'å' | 'ā' => 'a',
                'é' | 'è' | 'ë' | 'ê' | 'ē' => 'e',
                'í' | 'ì' | 'ï' | 'î' | 'ī' => 'i',
                'ó' | 'ò' | 'ö' | 'ô' | 'õ' | 'ø' | 'ō' => 'o',
                'ú' | 'ù' | 'ü' | 'û' | 'ū' => 'u',
                'ç' => 'c',
                'ñ' => 'n',
                'ß' => 's',
                '$' => 's',
                // Dropped, not spaced: don't -> dont.
                '\'' | '’' | '‘' | '`' => continue,
                other if other.is_alphanumeric() => other,
                _ => ' ',
            });
        }
    }
    out
}

fn tokens(text: &str) -> Vec<String> {
    fold(text).split_whitespace().map(str::to_owned).collect()
}

/// Words around a name that don't make it another name.
const NAME_FLUFF: &[&str] = &[
    "official", "music", "band", "vevo", "topic", "tv", "channel", "the", "real", "officiel",
];

/// How much an account's name is the artist's: 1 the same name, 0.6 the name with
/// one more word, 0 something else.
pub fn name_match(artist: &str, account: &str) -> f32 {
    let artist = tokens(artist);
    if artist.is_empty() {
        return 0.0;
    }
    let account = tokens(account);
    let plain: Vec<&String> = account
        .iter()
        .filter(|word| !NAME_FLUFF.contains(&word.as_str()))
        .collect();
    let wanted: Vec<&String> = artist
        .iter()
        .filter(|word| !NAME_FLUFF.contains(&word.as_str()))
        .collect();
    // A band called "The Music" is all fluff, so it's compared as it is.
    if wanted.is_empty() {
        return if account == artist { 1.0 } else { 0.0 };
    }
    if plain == wanted {
        return 1.0;
    }
    // Written together, or with the fluff stuck on: slayerofficial, SLAYERVEVO.
    let together: String = artist.concat();
    let written: String = account.concat();
    if written == together
        || NAME_FLUFF
            .iter()
            .any(|fluff| written.strip_suffix(fluff) == Some(together.as_str()))
    {
        return 1.0;
    }
    let all_there = wanted.iter().all(|word| plain.contains(word));
    if all_there && plain.len() == wanted.len() + 1 {
        return 0.6;
    }
    0.0
}

/// A bracket's words that say nothing about which song it is.
const BRACKET_NOISE: &[&str] = &[
    "official",
    "video",
    "audio",
    "lyric",
    "lyrics",
    "visualizer",
    "visualiser",
    "hd",
    "hq",
    "4k",
    "mv",
    "explicit",
    "clean",
    "free",
    "dl",
    "download",
    "prod",
    "produced",
    "feat",
    "ft",
    "featuring",
    "remaster",
    "remastered",
    "premiere",
    "uncut",
];

/// Takes the bracketed parts out of a title, keeping the words of those that could
/// tell two songs apart: "(Live)" stays, "[Official Video]" goes.
fn without_noise(title: &str) -> String {
    let mut kept = String::new();
    let mut inside = String::new();
    let mut depth = 0usize;
    for character in title.chars() {
        match character {
            '(' | '[' | '{' => {
                if depth > 0 {
                    inside.push(' ');
                }
                depth += 1;
            }
            ')' | ']' | '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let words = tokens(&inside);
                    let noise = words.first().is_some_and(|word| word == "with")
                        || words.iter().all(|word| word.chars().all(char::is_numeric))
                        || words
                            .iter()
                            .any(|word| BRACKET_NOISE.contains(&word.as_str()));
                    if !noise {
                        kept.push(' ');
                        kept.push_str(&inside);
                        kept.push(' ');
                    }
                    inside.clear();
                }
            }
            other if depth > 0 => inside.push(other),
            other => kept.push(other),
        }
    }
    kept
}

/// What a title calls the song, as words: without the artist, the tags, the guests
/// and the producer. "Slayer - Raining Blood (Official Video)" is "raining blood".
pub fn song_words(title: &str, artist: &str) -> Vec<String> {
    let title = without_noise(title);
    let artist_words = tokens(artist);
    let mut song = title.as_str();
    for separator in [" - ", " – ", " — ", " | ", " // "] {
        if let Some((left, right)) = title.split_once(separator) {
            let has = |part: &str| {
                let words = tokens(part);
                !artist_words.is_empty() && artist_words.iter().all(|word| words.contains(word))
            };
            if has(left) {
                song = right;
                break;
            }
            if has(right) {
                song = left;
                break;
            }
        }
    }
    // Spotify writes "Song - Remastered 2011" where an upload has only "Song".
    let song = [" - ", " – ", " — "]
        .iter()
        .find_map(|separator| {
            let (name, tag) = song.rsplit_once(separator)?;
            let noise = tokens(tag)
                .iter()
                .any(|word| BRACKET_NOISE.contains(&word.as_str()));
            noise.then_some(name)
        })
        .unwrap_or(song);
    let mut words = tokens(song);
    if let Some(cut) = words
        .iter()
        .position(|word| matches!(word.as_str(), "ft" | "feat" | "featuring" | "prod"))
        .filter(|cut| *cut > 0)
    {
        words.truncate(cut);
    }
    for phrase in [
        &["official", "music", "video"][..],
        &["official", "video"],
        &["official", "audio"],
        &["music", "video"],
        &["lyric", "video"],
        &["lyrics"],
        &["visualizer"],
    ] {
        if words.len() > phrase.len()
            && words[words.len() - phrase.len()..]
                .iter()
                .zip(phrase)
                .all(|(word, wanted)| word == wanted)
        {
            words.truncate(words.len() - phrase.len());
        }
    }
    words
}

/// What kind of upload a title describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Song,
    /// A version of a song someone made: remixes, sped up, covers.
    Edit,
    /// Not a song at all: interviews, trailers, type beats.
    NotMusic,
}

pub fn kind_of(title: &str) -> Kind {
    let words = tokens(title);
    let has = |wanted: &str| words.iter().any(|word| word == wanted);
    let pair = |first: &str, second: &str| {
        words
            .windows(2)
            .any(|pair| pair[0] == first && pair[1] == second)
    };
    if [
        "interview",
        "trailer",
        "reaction",
        "reacts",
        "vlog",
        "documentary",
        "podcast",
        "tutorial",
        "review",
        "shorts",
        "unboxing",
        "episode",
        "giveaway",
        "teaser",
        "announcement",
        "promo",
        "contest",
    ]
    .iter()
    .any(|word| has(word))
        || pair("type", "beat")
        || pair("behind", "the")
        || pair("making", "of")
        || pair("full", "album")
        || pair("short", "film")
        || pair("in", "theaters")
        || pair("coming", "soon")
        || pair("tour", "dates")
    {
        return Kind::NotMusic;
    }
    if [
        "edit",
        "edits",
        "remix",
        "rmx",
        "flip",
        "bootleg",
        "mashup",
        "slowed",
        "reverb",
        "reverbed",
        "spedup",
        "nightcore",
        "cover",
        "karaoke",
        "8d",
    ]
    .iter()
    .any(|word| has(word))
        || pair("sped", "up")
    {
        return Kind::Edit;
    }
    Kind::Song
}

/// The songs Spotify has of an artist.
#[derive(Clone, Debug, Default)]
pub struct Catalogue {
    artist: String,
    songs: Vec<Vec<String>>,
    exact: HashSet<String>,
}

impl Catalogue {
    pub fn new(artist: &str, titles: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let mut catalogue = Self {
            artist: artist.to_owned(),
            ..Default::default()
        };
        for title in titles {
            let words = song_words(title.as_ref(), artist);
            if !words.is_empty() && catalogue.exact.insert(words.concat()) {
                catalogue.songs.push(words);
            }
        }
        catalogue
    }

    pub fn len(&self) -> usize {
        self.songs.len()
    }

    /// These songs and an artist's own uploads, which an edit may be a version of.
    fn with_uploads(&self, uploads: &[Track]) -> Self {
        let mut known = self.clone();
        for upload in uploads {
            if kind_of(&upload.name) == Kind::Song {
                let words = song_words(&upload.name, &self.artist);
                if !words.is_empty() && known.exact.insert(words.concat()) {
                    known.songs.push(words);
                }
            }
        }
        known
    }

    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
    }

    /// Whether a title names one of these songs as the artist's: its words in a row,
    /// and the artist's name there too or `by` them, on its own rather than the end
    /// of a longer name. "Slayer - Raining Blood (sped up)" does; "Demon Slayer - Raining
    /// Blood" doesn't, since nothing says "Demon" is part of the song.
    fn mentions(&self, title: &str, by: &[String]) -> bool {
        let words = tokens(title);
        let artist = tokens(&self.artist);
        let song_at = self.songs.iter().find_map(|song| {
            // One short word is too easily part of something else.
            if song.len() == 1 && song[0].chars().count() < 5 {
                return None;
            }
            words
                .windows(song.len())
                .position(|window| window == song.as_slice())
                .map(|at| at..at + song.len())
        });
        let Some(song_at) = song_at else {
            return false;
        };
        if artist.iter().all(|word| by.contains(word)) {
            return true;
        }
        let Some(name_at) = words
            .windows(artist.len().max(1))
            .position(|window| window == artist.as_slice())
        else {
            return false;
        };
        name_at == 0
            || song_at.contains(&(name_at - 1))
            || matches!(
                words[name_at - 1].as_str(),
                "by" | "of" | "x" | "and" | "from"
            )
    }

    /// Whether an upload is a song Spotify already has.
    pub fn has(&self, title: &str) -> bool {
        let words = song_words(title, &self.artist);
        if words.is_empty() {
            return false;
        }
        if self.exact.contains(&words.concat()) {
            return true;
        }
        // A remix isn't the song it remixes, so only its exact title counts.
        if kind_of(title) != Kind::Song {
            return false;
        }
        let upload: HashSet<&String> = words.iter().collect();
        self.songs.iter().any(|song| {
            let known: HashSet<&String> = song.iter().collect();
            let shared = known.intersection(&upload).count();
            let union = known.union(&upload).count();
            // All of a title of two words or more, with a word or two around it.
            (known.len() >= 2 && shared == known.len() && upload.len() - shared <= 2)
                || (union > 0 && shared as f32 / union as f32 >= 0.8)
        })
    }
}

/// An account being considered, with what it has uploaded.
#[derive(Clone, Debug, Default)]
pub struct Candidate {
    pub account: Account,
    pub uploads: Vec<Track>,
}

/// How sure `candidate` is the artist's. `rival` is the most followers another
/// account of the very same name has.
fn rate(artist: &str, catalogue: &Catalogue, candidate: &Candidate, rival: u64) -> Confidence {
    let name = name_match(artist, &candidate.account.name);
    if name <= 0.0 {
        return Confidence::Unsure;
    }
    let shared = candidate
        .uploads
        .iter()
        .filter(|upload| kind_of(&upload.name) == Kind::Song && catalogue.has(&upload.name))
        .count();
    let same_name = name >= 1.0;
    let verified = candidate.account.verified;
    if (same_name && shared >= 2) || shared >= 3 || (verified && shared >= 1) {
        return Confidence::Sure;
    }
    let followers = candidate.account.followers.unwrap_or(0);
    let stands_out = followers >= 1_000 && followers > rival.saturating_mul(10);
    if same_name && (shared == 1 || verified || (stands_out && !candidate.uploads.is_empty())) {
        return Confidence::Likely;
    }
    Confidence::Unsure
}

/// The account that's the artist's, if one is likely enough, and who else it could be.
pub fn pick(
    artist: &str,
    catalogue: &Catalogue,
    candidates: Vec<Candidate>,
) -> (Option<(Candidate, Confidence)>, Vec<Account>) {
    let same_name: Vec<(usize, u64)> = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| name_match(artist, &candidate.account.name) >= 1.0)
        .map(|(index, candidate)| (index, candidate.account.followers.unwrap_or(0)))
        .collect();
    let mut rated: Vec<(Confidence, Candidate)> = candidates
        .into_iter()
        .enumerate()
        .map(|(index, candidate)| {
            let rival = same_name
                .iter()
                .filter(|(other, _)| *other != index)
                .map(|(_, followers)| *followers)
                .max()
                .unwrap_or(0);
            (rate(artist, catalogue, &candidate, rival), candidate)
        })
        .collect();
    rated.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            b.1.account
                .followers
                .unwrap_or(0)
                .cmp(&a.1.account.followers.unwrap_or(0))
        })
    });
    let mut rated = rated.into_iter();
    let first = rated.next();
    let others = |first: Option<&Candidate>,
                  rest: &mut dyn Iterator<Item = (Confidence, Candidate)>| {
        first
            .map(|candidate| candidate.account.clone())
            .into_iter()
            .chain(rest.map(|(_, candidate)| candidate.account))
            .filter(|account| name_match(artist, &account.name) > 0.0)
            .take(MOST_CANDIDATES)
            .collect::<Vec<_>>()
    };
    match first {
        Some((confidence, candidate)) if confidence >= Confidence::Likely => {
            let rest = others(None, &mut rated);
            (Some((candidate, confidence)), rest)
        }
        Some((_, candidate)) => {
            let all = others(Some(&candidate), &mut rated);
            (None, all)
        }
        None => (None, Vec::new()),
    }
}

/// An upload's plays, for ordering. None goes last.
fn plays(track: &Track) -> u64 {
    track.play_count.unwrap_or(0)
}

fn fits(track: &Track, side: Side) -> bool {
    side != Side::YouTube
        || track.duration_ms == 0
        || (SHORTEST_VIDEO_MS..=LONGEST_VIDEO_MS).contains(&track.duration_ms)
}

/// Whether a video is a song. A channel also posts films, updates and clips, so a
/// title has to say so, by a music tag or by the "Artist - Song" form, or else be
/// a song's length.
fn sounds_like_a_song(track: &Track, artist: &str) -> bool {
    let words = tokens(&track.name);
    let tagged = [
        "audio",
        "lyric",
        "lyrics",
        "visualizer",
        "visualiser",
        "prod",
        "mv",
    ]
    .iter()
    .any(|tag| words.iter().any(|word| word == tag))
        || words
            .windows(2)
            .any(|pair| pair[1] == "video" && matches!(pair[0].as_str(), "music" | "official"));
    let wanted = tokens(artist);
    let named = !wanted.is_empty() && words.starts_with(&wanted) && track.name.contains('-');
    tagged || named || (SONG_MS.contains(&track.duration_ms) && !track.name.contains(':'))
}

/// Sorts an artist's own uploads: what Spotify lacks, their edits, and how many
/// Spotify has. The most played come first, and a song uploaded twice appears once.
pub fn sort_uploads(
    artist: &str,
    catalogue: &Catalogue,
    uploads: Vec<Track>,
    side: Side,
) -> (Vec<Track>, Vec<Track>, usize) {
    let mut exclusives = Vec::new();
    let mut edits = Vec::new();
    let mut on_spotify = 0;
    let mut seen = HashSet::new();
    for upload in uploads {
        if kind_of(&upload.name) == Kind::NotMusic || !fits(&upload, side) {
            continue;
        }
        if side == Side::YouTube
            && kind_of(&upload.name) == Kind::Song
            && !sounds_like_a_song(&upload, artist)
        {
            continue;
        }
        if catalogue.has(&upload.name) {
            on_spotify += 1;
            continue;
        }
        if !seen.insert(song_words(&upload.name, artist).concat()) {
            continue;
        }
        match kind_of(&upload.name) {
            Kind::Edit => edits.push(upload),
            _ => exclusives.push(upload),
        }
    }
    exclusives.sort_by_key(|track| std::cmp::Reverse(plays(track)));
    edits.sort_by_key(|track| std::cmp::Reverse(plays(track)));
    (exclusives, edits, on_spotify)
}

/// What a search for the artist's edits found: only versions, by anyone, of a song
/// of theirs (`known` holds Spotify's and their own uploads). A name alone isn't
/// enough, as searching for Slayer's edits finds plenty of Demon Slayer.
pub fn sort_edits(artist: &str, known: &Catalogue, found: Vec<Track>, side: Side) -> Vec<Track> {
    debug_assert_eq!(artist, known.artist);
    let mut seen = HashSet::new();
    let mut edits: Vec<Track> = found
        .into_iter()
        .filter(|track| {
            let by = track
                .artists
                .iter()
                .flat_map(|artist| tokens(&artist.name))
                .collect::<Vec<_>>();
            kind_of(&track.name) == Kind::Edit
                && fits(track, side)
                && !known.has(&track.name)
                && known.mentions(&track.name, &by)
                && seen.insert(tokens(&track.name).concat())
        })
        .collect();
    edits.sort_by_key(|track| std::cmp::Reverse(plays(track)));
    edits.truncate(MOST_EDITS);
    edits
}

/// One list from what each source found, without a song twice. SoundCloud is read
/// first, so its audio upload wins over a video of the same song.
pub fn merged<'a>(artist: &str, lists: impl IntoIterator<Item = &'a [Track]>) -> Vec<Track> {
    let mut seen = HashSet::new();
    let mut all = Vec::new();
    for list in lists {
        for track in list {
            if seen.insert(song_words(&track.name, artist).concat()) {
                all.push(track.clone());
            }
        }
    }
    all.sort_by_key(|track| std::cmp::Reverse(plays(track)));
    all
}

/// Edits from each source as one list: the title has to match for two to be the
/// same, as two people's edits of one song aren't one edit.
pub fn merged_edits<'a>(lists: impl IntoIterator<Item = &'a [Track]>) -> Vec<Track> {
    let mut seen = HashSet::new();
    let mut all = Vec::new();
    for list in lists {
        for track in list {
            if seen.insert(tokens(&track.name).concat()) {
                all.push(track.clone());
            }
        }
    }
    all.sort_by_key(|track| std::cmp::Reverse(plays(track)));
    all
}

fn soundcloud_account(user: &models::User) -> Account {
    Account {
        uri: crate::soundcloud::user_uri(user.id),
        id: user.id.to_string(),
        name: user.username.clone(),
        image: user.avatar_url.clone(),
        followers: user.followers_count,
        verified: user.verified,
    }
}

fn youtube_account(card: &SourceCard, followers: Option<u64>) -> Account {
    Account {
        uri: card.uri.clone(),
        id: browse::channel_of(&card.uri).unwrap_or_default().to_owned(),
        name: card.title.clone(),
        image: card.image.clone(),
        followers,
        verified: false,
    }
}

/// What the listener or an earlier search settled about a source.
fn settled(link: Option<&Link>) -> (Option<Account>, Option<Confidence>, bool) {
    match link {
        Some(Link::Chosen(account)) => (Some(account.clone()), Some(Confidence::Chosen), false),
        Some(Link::NotThere) => (None, None, true),
        _ => (None, None, false),
    }
}

/// Reads what SoundCloud has of an artist.
pub async fn soundcloud(
    client: &Client,
    artist: &str,
    catalogue: &Catalogue,
    link: Option<&Link>,
) -> Finding {
    let (chosen, confidence, not_there) = settled(link);
    if not_there {
        return Finding {
            not_there,
            ..Default::default()
        };
    }
    let users = match client.search_users(artist, USER_SEARCH, 0).await {
        Ok(page) => page.collection,
        Err(error) if chosen.is_none() => {
            return Finding {
                error: Some(error.to_string()),
                ..Default::default()
            };
        }
        Err(_) => Vec::new(),
    };
    let mut users: Vec<models::User> = users
        .into_iter()
        .filter(|user| name_match(artist, &user.username) > 0.0)
        .collect();
    users.sort_by_key(|user| {
        std::cmp::Reverse((
            name_match(artist, &user.username) >= 1.0,
            user.followers_count.unwrap_or(0),
        ))
    });
    let others: Vec<Account> = users.iter().map(soundcloud_account).collect();
    let (account, confidence, uploads, candidates) = if let Some(account) = chosen {
        let uploads = read_soundcloud(client, account.id.parse().unwrap_or(0)).await;
        (Some(account), confidence, uploads, others)
    } else {
        let looks = users.iter().take(SOUNDCLOUD_LOOKS).map(|user| async {
            Candidate {
                account: soundcloud_account(user),
                uploads: read_soundcloud(client, user.id).await,
            }
        });
        let candidates = futures_util::future::join_all(looks).await;
        match pick(artist, catalogue, candidates) {
            (Some((candidate, confidence)), rest) => (
                Some(candidate.account),
                Some(confidence),
                candidate.uploads,
                rest,
            ),
            (None, rest) => (None, None, Vec::new(), rest),
        }
    };
    let Some(account) = account else {
        return Finding {
            candidates,
            ..Default::default()
        };
    };
    let known = catalogue.with_uploads(&uploads);
    let (exclusives, mut edits, on_spotify) =
        sort_uploads(artist, catalogue, uploads, Side::SoundCloud);
    if let Ok(page) = client
        .search_tracks(&format!("{artist} edit"), EDIT_SEARCH, 0)
        .await
    {
        let found = page
            .collection
            .iter()
            .map(|track| track.to_track(None))
            .collect();
        edits.extend(sort_edits(artist, &known, found, Side::SoundCloud));
    }
    let candidates = candidates
        .into_iter()
        .filter(|other| other.uri != account.uri)
        .take(MOST_CANDIDATES)
        .collect();
    Finding {
        account: Some(account),
        confidence,
        candidates,
        exclusives,
        edits,
        on_spotify,
        ..Default::default()
    }
}

async fn read_soundcloud(client: &Client, id: u64) -> Vec<Track> {
    client
        .user_tracks(id, SOUNDCLOUD_UPLOADS, 0)
        .await
        .map(|page| {
            page.collection
                .iter()
                .map(|track| track.to_track(None))
                .collect()
        })
        .unwrap_or_default()
}

/// Reads what YouTube has of an artist. A "- Topic" channel is one YouTube makes
/// from releases, so it's Spotify's catalogue again and never the artist's channel.
pub async fn youtube(
    resolver: &Resolver,
    artist: &str,
    catalogue: &Catalogue,
    link: Option<&Link>,
) -> Finding {
    if !resolver.is_installed() {
        return Finding::default();
    }
    let (chosen, confidence, not_there) = settled(link);
    if not_there {
        return Finding {
            not_there,
            ..Default::default()
        };
    }
    let cards = browse::channels(resolver, artist).await;
    let cards = match cards {
        Ok(cards) => cards,
        Err(error) if chosen.is_none() => {
            return Finding {
                error: Some(error),
                ..Default::default()
            };
        }
        Err(_) => Vec::new(),
    };
    let mut cards: Vec<(SourceCard, Option<u64>)> = cards
        .into_iter()
        .filter(|(card, _)| {
            !card.title.ends_with("- Topic") && name_match(artist, &card.title) > 0.0
        })
        .collect();
    cards.sort_by_key(|(card, followers)| {
        std::cmp::Reverse((
            name_match(artist, &card.title) >= 1.0,
            followers.unwrap_or(0),
        ))
    });
    let others: Vec<Account> = cards
        .iter()
        .map(|(card, followers)| youtube_account(card, *followers))
        .collect();
    // Each look is a run of yt-dlp. The search for edits starts alongside them.
    let edit_search = format!("{artist} edit");
    let edit_read = browse::videos_for(resolver, &edit_search);
    let most = cards
        .iter()
        .filter_map(|(_, followers)| *followers)
        .max()
        .unwrap_or(0);
    let accounts = async {
        if let Some(account) = chosen {
            let uploads = read_youtube(resolver, &account.uri).await;
            return (Some(account), confidence, uploads, others);
        }
        let looks = cards
            .iter()
            .filter(|(_, followers)| followers.unwrap_or(0) * 100 >= most)
            .take(YOUTUBE_LOOKS)
            .map(|(card, followers)| async {
                Candidate {
                    account: youtube_account(card, *followers),
                    uploads: read_youtube(resolver, &card.uri).await,
                }
            });
        let candidates = futures_util::future::join_all(looks).await;
        match pick(artist, catalogue, candidates) {
            (Some((candidate, confidence)), rest) => (
                Some(candidate.account),
                Some(confidence),
                candidate.uploads,
                rest,
            ),
            (None, rest) => (None, None, Vec::new(), rest),
        }
    };
    let ((account, confidence, uploads, candidates), edit_found) =
        tokio::join!(accounts, edit_read);
    let Some(account) = account else {
        return Finding {
            candidates,
            ..Default::default()
        };
    };
    let known = catalogue.with_uploads(&uploads);
    let (exclusives, mut edits, on_spotify) =
        sort_uploads(artist, catalogue, uploads, Side::YouTube);
    if let Ok(found) = edit_found {
        edits.extend(sort_edits(artist, &known, found, Side::YouTube));
    }
    let candidates = candidates
        .into_iter()
        .filter(|other| other.uri != account.uri)
        .take(MOST_CANDIDATES)
        .collect();
    Finding {
        account: Some(account),
        confidence,
        candidates,
        exclusives,
        edits,
        on_spotify,
        ..Default::default()
    }
}

async fn read_youtube(resolver: &Resolver, uri: &str) -> Vec<Track> {
    browse::page(resolver, uri, None)
        .await
        .map(|page| page.tracks)
        .unwrap_or_default()
}

/// What the app asks about an artist.
#[derive(Clone, Debug)]
pub struct Request {
    pub id: String,
    pub name: String,
    /// Identifies the page's latest ask, so an older answer can't undo a choice.
    pub ask: u64,
    pub titles: Vec<String>,
    pub albums: Vec<Album>,
    /// Read it again even if what's saved is fresh.
    pub fresh: bool,
}

#[cfg(test)]
#[path = "artist_sources/tests.rs"]
mod tests;
