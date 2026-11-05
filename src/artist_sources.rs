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
const BEYOND_SEARCH: u32 = 30;
const SETS_READ: u32 = 50;
const MOST_CANDIDATES: usize = 4;
const MOST_EDITS: usize = 30;
const MOST_UNRELEASED: usize = 60;
const MOST_PRODUCED: usize = 40;
/// How many of the accounts posting an artist's unreleased songs are read through.
const ARCHIVE_LOOKS: usize = 3;
/// How many posts make an account one that follows the artist.
const POSTS_TO_LOOK: usize = 3;
/// Fewer than this and the search only happened on a stray credit.
const FEWEST_PRODUCED: usize = 3;
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
    /// Snippets, leaks and scrapped songs, mostly posted by other accounts.
    #[serde(default)]
    pub unreleased: Vec<Track>,
    /// Accounts that keep posting those, to follow up on.
    #[serde(default)]
    pub archives: Vec<Account>,
    /// Songs by others that credit this artist as producer.
    #[serde(default)]
    pub produced: Vec<Track>,
    /// Their SoundCloud sets and mixtapes that Spotify doesn't have.
    #[serde(default)]
    pub sets: Vec<Set>,
    /// Other accounts the artist posts on, and what's there that Spotify lacks.
    #[serde(default)]
    pub alternates: Vec<Account>,
    #[serde(default)]
    pub alternate_songs: Vec<Track>,
    #[serde(default)]
    pub error: Option<String>,
}

/// A set or mixtape on an artist's account.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Set {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub songs: Option<u32>,
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
    /// Other accounts they post on.
    #[serde(default)]
    pub alternates: Vec<Account>,
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
            for account in &linked.alternates {
                back.insert(account.uri.clone(), (id.clone(), linked.name.clone()));
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

/// Words saying an upload hasn't been released.
const UNRELEASED_WORDS: &[&str] = &[
    "snippet",
    "snippets",
    "leak",
    "leaks",
    "leaked",
    "unreleased",
    "unrlsd",
    "scrapped",
    "throwaway",
];

/// Words saying an account collects an artist's unreleased songs.
const ARCHIVE_WORDS: &[&str] = &[
    "archive",
    "archives",
    "vault",
    "leak",
    "leaks",
    "leaked",
    "unreleased",
    "unrlsd",
    "snippets",
];

fn says_unreleased(title: &str) -> bool {
    tokens(title)
        .iter()
        .any(|word| UNRELEASED_WORDS.contains(&word.as_str()))
}

fn is_snippet(track: &Track) -> bool {
    tokens(&track.name)
        .iter()
        .any(|word| word == "snippet" || word == "snippets")
}

/// A title's words, each with whether something other than a space came before it,
/// which is how "Yeat & Slayr" differs from "Demon Slayer".
fn words_and_breaks(title: &str) -> Vec<(String, bool)> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut broken = true;
    let mut next_broken = false;
    for character in title.chars() {
        let folded = fold(&character.to_string());
        let spacing = character.is_whitespace();
        if !folded.trim().is_empty() {
            word.push_str(folded.trim());
        } else if matches!(character, '\'' | '’' | '‘' | '`') {
            // Part of the word it's in.
        } else {
            if !word.is_empty() {
                words.push((std::mem::take(&mut word), broken));
                broken = false;
            }
            if !spacing {
                next_broken = true;
            }
            if next_broken {
                broken = true;
                next_broken = false;
            }
        }
    }
    if !word.is_empty() {
        words.push((word, broken));
    }
    words
}

/// Whether a title names this artist, rather than ending in their name. "Slayr -
/// Song", "Song (ft. Slayr)", "Yeat & Slayr" and "Song - Slayr" do; "Demon Slayer"
/// doesn't name Slayer, since the name runs straight on from another word.
pub fn names_artist(title: &str, artist: &str) -> bool {
    let wanted = tokens(artist);
    if wanted.is_empty() {
        return false;
    }
    let words = words_and_breaks(title);
    words.windows(wanted.len()).enumerate().any(|(at, window)| {
        window
            .iter()
            .zip(&wanted)
            .all(|((word, _), want)| word == want)
            && (at == 0
                || window[0].1
                || matches!(
                    words[at - 1].0.as_str(),
                    "ft" | "feat" | "featuring" | "with" | "by" | "prod" | "x" | "and" | "w"
                ))
    })
}

/// What's left of a title to tell a song by: outside its brackets, without the
/// artist or the words about what kind of upload it is.
fn plain_song(title: &str, artist: &str) -> Vec<String> {
    let mut outside = String::new();
    let mut depth = 0usize;
    for character in title.chars() {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            other if depth == 0 => outside.push(other),
            _ => {}
        }
    }
    let name = tokens(artist);
    song_words(&outside, artist)
        .into_iter()
        .filter(|word| {
            !UNRELEASED_WORDS.contains(&word.as_str())
                && !name.contains(word)
                && !matches!(
                    word.as_str(),
                    "extended" | "full" | "song" | "new" | "version" | "best" | "og" | "cdq" | "hq"
                )
        })
        .collect()
}

/// Whether an account's whole purpose is this artist's unreleased songs, as "slayr
/// archives" or "$layrarchives" is.
pub fn is_archive(account: &str, artist: &str) -> bool {
    let together = tokens(account).concat();
    let name = tokens(artist).concat();
    !name.is_empty()
        && together.contains(&name)
        && together != name
        && ARCHIVE_WORDS.iter().any(|word| together.contains(word))
}

/// Sorts what searches and archive accounts found into one list of unreleased songs:
/// a song once, full versions before snippets, and nothing Spotify has since released.
/// `archives` are accounts about the artist, whose uploads need no label to count.
pub fn sort_unreleased(
    artist: &str,
    catalogue: &Catalogue,
    found: Vec<Track>,
    archives: &[String],
    side: Side,
) -> Vec<Track> {
    let mut best: HashMap<String, Track> = HashMap::new();
    for track in found {
        let from_archive = track
            .artists
            .iter()
            .filter_map(|artist| artist.uri.as_ref())
            .any(|uri| archives.contains(uri));
        let labelled = says_unreleased(&track.name) && names_artist(&track.name, artist);
        if !(labelled || from_archive)
            || kind_of(&track.name) == Kind::NotMusic
            || !fits(&track, side)
        {
            continue;
        }
        let words = plain_song(&track.name, artist);
        if words.is_empty() || catalogue.has(&words.join(" ")) {
            continue;
        }
        let better = |new: &Track, old: &Track| {
            (!is_snippet(new), plays(new)) > (!is_snippet(old), plays(old))
        };
        match best.entry(words.concat()) {
            std::collections::hash_map::Entry::Occupied(mut held) => {
                if better(&track, held.get()) {
                    held.insert(track);
                }
            }
            std::collections::hash_map::Entry::Vacant(place) => {
                place.insert(track);
            }
        }
    }
    let mut all: Vec<Track> = best.into_values().collect();
    all.sort_by_key(|track| (is_snippet(track), std::cmp::Reverse(plays(track))));
    all.truncate(MOST_UNRELEASED);
    all
}

/// Whether a title credits the artist as producer, alone or among others:
/// "(prod. waera)", "prod by waera + x", "produced by someone & waera".
pub fn credits_producer(title: &str, artist: &str) -> bool {
    let words = tokens(title);
    let wanted = tokens(artist);
    if wanted.is_empty() {
        return false;
    }
    words.iter().enumerate().any(|(at, word)| {
        let credit = matches!(word.as_str(), "prod" | "produced" | "producer" | "prodby");
        credit
            && words[at + 1..]
                .iter()
                .take(8)
                .collect::<Vec<_>>()
                .windows(wanted.len())
                .any(|window| window.iter().zip(&wanted).all(|(word, want)| *word == want))
    })
}

/// What searching for an artist's productions found: songs by others that credit
/// them, a song once, the most played first. A handful means a stray credit rather
/// than a producer, so it's nothing.
pub fn sort_produced(artist: &str, found: Vec<Track>, side: Side) -> Vec<Track> {
    let wanted = tokens(artist);
    let mut seen = HashSet::new();
    let mut all: Vec<Track> = found
        .into_iter()
        .filter(|track| {
            let by_them = track
                .artists
                .iter()
                .any(|artist| tokens(&artist.name) == wanted);
            credits_producer(&track.name, artist)
                && !by_them
                && kind_of(&track.name) == Kind::Song
                && !says_unreleased(&track.name)
                && fits(track, side)
                && seen.insert(song_words(&track.name, artist).concat())
        })
        .collect();
    all.sort_by_key(|track| std::cmp::Reverse(plays(track)));
    all.truncate(MOST_PRODUCED);
    all
}

/// What connects an artist's main account to another one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccountTies {
    /// Their bio has the other's name as a handle or at the end of a link.
    pub named_in_bio: bool,
    pub follows_main: bool,
    /// How many of the main account's recent reposts are from the other.
    pub reposts: usize,
}

/// Fewest letters in a name to be read as a handle in a bio: short ones turn up by
/// chance inside other words and links.
const SHORTEST_HANDLE: usize = 5;
/// Fewest of the main account's reposts that count as a tie.
const REPOSTS_FOR_TIE: usize = 2;
/// Names of labels, managers and the like, which a bio names for other reasons.
const NOT_A_PERSON: &[&str] = &[
    "records",
    "recordings",
    "label",
    "mgmt",
    "management",
    "booking",
    "publishing",
    "radio",
    "collective",
    "official",
];

/// Whether a bio gives `name` as a handle: `@name`, or the last part of a link such
/// as `instagram.com/name`. A plain word in a sentence doesn't count.
pub fn bio_names(bio: &str, name: &str) -> bool {
    let wanted = name.to_lowercase();
    if wanted.chars().count() < SHORTEST_HANDLE
        || !wanted
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '.' | '-'))
    {
        return false;
    }
    bio.to_lowercase().split_whitespace().any(|word| {
        let word = word.trim_matches(|character: char| {
            !character.is_alphanumeric() && !matches!(character, '@' | '_')
        });
        let word = word.split(['?', '#']).next().unwrap_or(word);
        word.strip_prefix('@') == Some(wanted.as_str())
            || (word.contains('/') && word.rsplit('/').next() == Some(wanted.as_str()))
    })
}

/// Whether another account is the artist's own. A collaborator also reposts them,
/// follows them and credits them, so only the artist's own bio giving that name can
/// say so. The account then has to show a tie of its own, as a bio can name anyone,
/// and be smaller than the main one, which a label or a crew usually isn't.
pub fn is_alternate(main: &Account, other: &Account, ties: AccountTies) -> bool {
    let words = tokens(&other.name);
    ties.named_in_bio
        && (ties.follows_main || ties.reposts >= REPOSTS_FOR_TIE)
        && other.uri != main.uri
        && other.followers.unwrap_or(0) <= main.followers.unwrap_or(0)
        && !words
            .iter()
            .any(|word| NOT_A_PERSON.contains(&word.as_str()))
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

/// Tags that say what kind of upload it is, gone from a title once it's a song row.
const DISPLAY_NOISE: &[&str] = &[
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
];

/// Makes an upload read as a song: "slayr - Holding (OFFICIAL MUSIC VIDEO)" posted by
/// anyone shows as Holding by slayr. The name written in the title is who the song is
/// by, so it doesn't lead to the account that posted it.
pub fn tidied(mut track: Track, artist: &str) -> Track {
    let mut name = String::new();
    let mut inside = String::new();
    let mut depth = 0usize;
    for character in track.name.chars() {
        match character {
            '(' | '[' | '{' => {
                if depth == 0 {
                    inside.clear();
                }
                inside.push(character);
                depth += 1;
            }
            ')' | ']' | '}' if depth > 0 => {
                inside.push(character);
                depth -= 1;
                if depth == 0
                    && !tokens(&inside)
                        .iter()
                        .any(|word| DISPLAY_NOISE.contains(&word.as_str()))
                {
                    name.push_str(&inside);
                }
            }
            other if depth > 0 => inside.push(other),
            other => name.push(other),
        }
    }
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let wanted = tokens(artist);
    let by_artist = |part: &str| {
        let words = tokens(part);
        !wanted.is_empty() && wanted.iter().all(|word| words.contains(word))
    };
    // "Slayr- Song" is written by hand often enough, but only the artist's own name
    // makes that a separator rather than a word like "T-shirt".
    let split = [" - ", " – ", " — "]
        .iter()
        .find_map(|separator| name.split_once(separator))
        .or_else(|| {
            ["- ", " -"].iter().find_map(|separator| {
                name.split_once(separator)
                    .filter(|(left, _)| by_artist(left))
            })
        });
    let (song, performer) = match split {
        Some((left, right)) if !right.trim().is_empty() && !left.trim().is_empty() => {
            if by_artist(left) || (!by_artist(right) && tokens(left).len() <= 5) {
                (right.trim().to_owned(), Some(left.trim().to_owned()))
            } else if by_artist(right) {
                (left.trim().to_owned(), Some(right.trim().to_owned()))
            } else {
                (name.clone(), None)
            }
        }
        _ => (name.clone(), None),
    };
    if !song.is_empty() {
        track.name = song;
    }
    // Whatever's in brackets beside a name isn't the name: "(NO AI) slayr".
    let performer = performer.map(|written| {
        let mut plain = String::new();
        let mut depth = 0usize;
        for character in written.chars() {
            match character {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.saturating_sub(1),
                other if depth == 0 => plain.push(other),
                _ => {}
            }
        }
        plain.split_whitespace().collect::<Vec<_>>().join(" ")
    });
    if let Some(performer) = performer.filter(|name| !name.is_empty()) {
        track.artists = vec![crate::api::models::ArtistRef {
            id: None,
            name: performer,
            uri: None,
        }];
    }
    track
}

impl Finding {
    /// Every list made to read as songs, for this artist.
    pub fn tidy(&mut self, artist: &str) {
        for list in [
            &mut self.exclusives,
            &mut self.edits,
            &mut self.unreleased,
            &mut self.produced,
            &mut self.alternate_songs,
        ] {
            *list = std::mem::take(list)
                .into_iter()
                .map(|track| tidied(track, artist))
                .collect();
        }
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

/// An artist's sets that aren't a release Spotify has under that name, albums first.
pub fn sort_sets(catalogue: &Catalogue, sets: Vec<models::Playlist>) -> Vec<Set> {
    let mut seen = HashSet::new();
    let mut kept: Vec<models::Playlist> = sets
        .into_iter()
        .filter(|set| {
            set.public != Some(false)
                && set.track_count.unwrap_or(1) > 1
                && !set.title.trim().is_empty()
                && !catalogue.has(&set.title)
                && seen.insert(tokens(&set.title).concat())
        })
        .collect();
    kept.sort_by_key(|set| std::cmp::Reverse(set.is_album.unwrap_or(false)));
    kept.into_iter()
        .map(|set| {
            let converted = set.to_playlist();
            Set {
                uri: converted.uri.clone(),
                name: converted.name.clone(),
                image: crate::api::models::pick_image(&converted.images, 300).map(str::to_owned),
                songs: set.track_count,
            }
        })
        .collect()
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

/// Unreleased songs apart from any the artist has posted themselves.
pub fn without_songs(artist: &str, unreleased: Vec<Track>, songs: &[Track]) -> Vec<Track> {
    let known: HashSet<String> = songs
        .iter()
        .map(|song| plain_song(&song.name, artist).concat())
        .collect();
    unreleased
        .into_iter()
        .filter(|track| !known.contains(&plain_song(&track.name, artist).concat()))
        .collect()
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
    let sets = match account.id.parse() {
        Ok(id) => client
            .user_playlists(id, SETS_READ, 0)
            .await
            .map(|page| sort_sets(catalogue, page.collection))
            .unwrap_or_default(),
        Err(_) => Vec::new(),
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
        sets,
        ..Default::default()
    }
}

/// An account's uploads, following its pages as far as `MOST_PAGES`.
async fn read_soundcloud(client: &Client, id: u64) -> Vec<Track> {
    const MOST_PAGES: usize = 3;
    let Ok(mut page) = client.user_tracks(id, SOUNDCLOUD_UPLOADS, 0).await else {
        return Vec::new();
    };
    let mut tracks: Vec<Track> = page
        .collection
        .iter()
        .map(|track| track.to_track(None))
        .collect();
    for _ in 1..MOST_PAGES {
        let Some(next) = page.next_href.clone() else {
            break;
        };
        let Ok(following) = client.next_page::<models::Track>(&next).await else {
            break;
        };
        tracks.extend(
            following
                .collection
                .iter()
                .map(|track| track.to_track(None)),
        );
        page = following;
    }
    tracks
}

/// How many of an account's recent reposts came from whom.
async fn reposted_by(client: &Client, id: u64) -> Vec<(models::User, usize)> {
    const REPOSTS_READ: u32 = 100;
    let url =
        format!("https://api-v2.soundcloud.com/stream/users/{id}/reposts?limit={REPOSTS_READ}");
    let Ok(page) = client.next_page::<serde_json::Value>(&url).await else {
        return Vec::new();
    };
    let mut counts: HashMap<u64, (models::User, usize)> = HashMap::new();
    for item in &page.collection {
        let user = item
            .pointer("/track/user")
            .or_else(|| item.pointer("/playlist/user"))
            .and_then(|user| serde_json::from_value::<models::User>(user.clone()).ok());
        if let Some(user) = user {
            counts.entry(user.id).or_insert_with(|| (user, 0)).1 += 1;
        }
    }
    counts.into_values().collect()
}

/// Other accounts the artist posts on, found from their own main account.
pub async fn soundcloud_alternates(client: &Client, main: &Account) -> Vec<(Account, AccountTies)> {
    let Ok(id) = main.id.parse::<u64>() else {
        return Vec::new();
    };
    let (profile, reposted, followed) = tokio::join!(
        client.user(id),
        reposted_by(client, id),
        client.user_followings(id, 200, 0),
    );
    let bio = profile
        .ok()
        .and_then(|profile| profile.description)
        .unwrap_or_default();
    if bio.trim().is_empty() {
        return Vec::new();
    }
    let mut named: Vec<(models::User, usize)> = reposted
        .into_iter()
        .filter(|(user, _)| bio_names(&bio, &user.username))
        .collect();
    if let Ok(page) = followed {
        for user in page.collection {
            if bio_names(&bio, &user.username)
                && !named.iter().any(|(known, _)| known.id == user.id)
            {
                named.push((user, 0));
            }
        }
    }
    let mut alternates = Vec::new();
    for (user, reposts) in named {
        let follows_main = client
            .user_followings(user.id, 200, 0)
            .await
            .is_ok_and(|page| page.collection.iter().any(|followed| followed.id == id));
        let account = soundcloud_account(&user);
        let ties = AccountTies {
            named_in_bio: true,
            follows_main,
            reposts,
        };
        if is_alternate(main, &account, ties) {
            alternates.push((account, ties));
        }
    }
    alternates
}

/// What people other than the artist post: SoundCloud's snippets, leaks and scrapped
/// songs, the accounts that keep posting them, and songs crediting the artist.
pub async fn soundcloud_beyond(
    client: &Client,
    artist: &str,
    catalogue: &Catalogue,
    main: Option<&Account>,
) -> Beyond {
    let search = |word: &'static str| {
        let query = format!("{artist} {word}");
        async move {
            client
                .search_tracks(&query, BEYOND_SEARCH, 0)
                .await
                .map(|page| page.collection)
                .unwrap_or_default()
        }
    };
    let produced_query = format!("prod {artist}");
    let also_query = format!("prod. by {artist}");
    let (snippets, leaks, unreleased, scrapped, produced, also_produced) = tokio::join!(
        search("snippet"),
        search("leak"),
        search("unreleased"),
        search("scrapped"),
        client.search_tracks(&produced_query, BEYOND_SEARCH, 0),
        client.search_tracks(&also_query, BEYOND_SEARCH, 0),
    );
    let found: Vec<models::Track> = [snippets, leaks, unreleased, scrapped].concat();
    let alternates = match main {
        Some(main) => soundcloud_alternates(client, main).await,
        None => Vec::new(),
    };
    let mut alternate_songs = Vec::new();
    for (account, _) in &alternates {
        let uploads = read_soundcloud(client, account.id.parse().unwrap_or(0)).await;
        let (songs, edits, _) = sort_uploads(artist, catalogue, uploads, Side::SoundCloud);
        alternate_songs.extend(songs);
        alternate_songs.extend(edits);
    }
    alternate_songs.sort_by_key(|track| std::cmp::Reverse(plays(track)));

    // Who keeps posting them.
    let mut posters: HashMap<u64, (models::User, usize)> = HashMap::new();
    for track in &found {
        if let Some(user) = &track.user {
            let title = track.title.as_deref().unwrap_or_default();
            if says_unreleased(title) && names_artist(title, artist) {
                posters
                    .entry(user.id)
                    .or_insert_with(|| (user.clone(), 0))
                    .1 += 1;
            }
        }
    }
    let mut posters: Vec<(models::User, usize)> = posters
        .into_values()
        .filter(|(user, posts)| *posts >= POSTS_TO_LOOK || is_archive(&user.username, artist))
        .collect();
    posters.sort_by_key(|(user, posts)| {
        std::cmp::Reverse((is_archive(&user.username, artist), *posts))
    });
    posters.truncate(ARCHIVE_LOOKS);
    let reads = posters
        .iter()
        .map(|(user, _)| read_soundcloud(client, user.id));
    let read: Vec<Track> = futures_util::future::join_all(reads)
        .await
        .into_iter()
        .flatten()
        .collect();
    let archive_uris: Vec<String> = posters
        .iter()
        .filter(|(user, _)| is_archive(&user.username, artist))
        .map(|(user, _)| crate::soundcloud::user_uri(user.id))
        .collect();
    let mut all: Vec<Track> = found.iter().map(|track| track.to_track(None)).collect();
    all.extend(read);
    let produced_found: Vec<Track> = [produced, also_produced]
        .into_iter()
        .flatten()
        .flat_map(|page| page.collection)
        .map(|track| track.to_track(None))
        .collect();
    Beyond {
        unreleased: sort_unreleased(artist, catalogue, all, &archive_uris, Side::SoundCloud),
        archives: posters
            .iter()
            .map(|(user, _)| soundcloud_account(user))
            .collect(),
        produced: sort_produced(artist, produced_found, Side::SoundCloud),
        alternates: alternates.into_iter().map(|(account, _)| account).collect(),
        alternate_songs,
    }
}

/// The same from YouTube, whose searches cost a yt-dlp run each, so there are fewer.
pub async fn youtube_beyond(resolver: &Resolver, artist: &str, catalogue: &Catalogue) -> Beyond {
    if !resolver.is_installed() {
        return Beyond::default();
    }
    let unreleased_query = format!("{artist} unreleased snippet");
    let produced_query = format!("prod {artist}");
    let (unreleased, produced) = tokio::join!(
        browse::videos_for(resolver, &unreleased_query),
        browse::videos_for(resolver, &produced_query),
    );
    Beyond {
        unreleased: sort_unreleased(
            artist,
            catalogue,
            unreleased.unwrap_or_default(),
            &[],
            Side::YouTube,
        ),
        archives: Vec::new(),
        produced: sort_produced(artist, produced.unwrap_or_default(), Side::YouTube),
        ..Default::default()
    }
}

/// What others post about an artist, as found on one source.
#[derive(Clone, Debug, Default)]
pub struct Beyond {
    pub unreleased: Vec<Track>,
    pub archives: Vec<Account>,
    pub produced: Vec<Track>,
    pub alternates: Vec<Account>,
    pub alternate_songs: Vec<Track>,
}

impl Finding {
    pub fn add(&mut self, beyond: Beyond) {
        self.unreleased = beyond.unreleased;
        self.archives = beyond.archives;
        self.produced = beyond.produced;
        self.alternates = beyond.alternates;
        self.alternate_songs = beyond.alternate_songs;
    }
}

/// Songs crediting an artist only say they're a producer if there are a few.
pub fn enough_produced(all: Vec<Track>) -> Vec<Track> {
    if all.len() < FEWEST_PRODUCED {
        Vec::new()
    } else {
        all
    }
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
