//! A beat's own name, read out of the upload title it hides in.
//!
//! Producers pack several ideas into one title: `[FREE]` tags, who a beat is
//! "for", its genre, its tempo. The player wants the beat itself in front:
//! its name as the title, who it's for or its style on the second line, and
//! its tempo kept on the track for the rest of the app. The producer is
//! never touched: the channel stays where the uploader is shown, and the
//! track keeps its uri, its search and its matching.
//!
//! Pure string handling only, so a beat's name never waits on the network.
//! Where a beat is drawn, `for_line` and `tempo_key` turn the reading into
//! what the interface shows.
//!
//! It also decides whether an upload is a beat at all. YouTube has no such
//! label, and Home must not fill with concerts, songs and tutorials: only a
//! title that plainly says a beat, a producer's channel, or a beat's genre
//! makes one.

use serde::{Deserialize, Serialize};

use crate::api::models::Track;

/// The shortest upload that can be a beat; anything shorter is a Short, a
/// preview or a trailer, not a beat.
pub const SHORTEST_BEAT_MS: u32 = 45_000;
/// The longest upload that can be a beat; beyond it is a mix, a set or a
/// compilation, not one beat.
pub const LONGEST_BEAT_MS: u32 = 10 * 60_000;

/// The words that say an upload is something other than a beat: a concert or
/// a song's video, a reaction, an interview, a tutorial, a mix, a compilation.
/// These win over a beat word in the title, so "Type Beat Breakdown" and
/// "Type Beat (Official Video)" never reach Home.
const NOT_A_BEAT: &[&str] = &[
    "official video",
    "official music video",
    "official audio",
    "official visualizer",
    "music video",
    "lyric video",
    "lyrics video",
    "lyric",
    "lyrics",
    "visualizer",
    "visualizers",
    "visualiser",
    "live at",
    "live from",
    "live session",
    "tiny desk",
    "concert",
    "concerts",
    "full set",
    "full sets",
    "festival",
    "festivals",
    "live performance",
    "reaction",
    "reactions",
    "react",
    "reacts",
    "reacting",
    "interview",
    "interviews",
    "podcast",
    "podcasts",
    "tutorial",
    "tutorials",
    "how to",
    "how i",
    "breakdown",
    "breakdowns",
    "lesson",
    "lessons",
    "masterclass",
    "masterclasses",
    "explained",
    "review",
    "reviews",
    "beat saber",
    "beatbox",
    "beatboxing",
    "beat it",
    "beat the",
    "beat of",
    "beatles",
    "metronome",
    "metronomes",
    "karaoke",
    "cover",
    "covers",
    "mix",
    "mixes",
    "mixtape",
    "mixtapes",
    "compilation",
    "compilations",
    "full album",
    "album stream",
    "full tape",
    "playlist",
    "playlists",
    "1 hour",
    "one hour",
];

/// The words that say an upload is about making a beat, not one to listen to.
/// They only reject a title that says nothing else, so "Cook Up Type Beat"
/// stays a beat.
const ABOUT_BEATS: &[&str] = &["cook up", "cookup"];

/// Whether a track is a beat (an instrumental made for others to rap or sing
/// on) rather than a song, a video or a set. Beats have a place of their own
/// on Home, and YouTube is only for finding them.
///
/// A beat has to say so: a title naming a beat, a producer's channel, or a
/// beat's genre. A length that is known must fit a single beat too. A
/// YouTube upload that says nothing is left out, because Home would rather
/// miss a beat than recommend a song or a concert.
pub fn is_beat(track: &Track) -> bool {
    if track.duration_ms > 0 && !(SHORTEST_BEAT_MS..=LONGEST_BEAT_MS).contains(&track.duration_ms) {
        return false;
    }
    let title = spaced_words(&track.name);
    if NOT_A_BEAT.iter().any(|said| holds(&title, said)) {
        return false;
    }
    let plainly = says_beat(&track.name);
    if !plainly && ABOUT_BEATS.iter().any(|said| holds(&title, said)) {
        return false;
    }
    if plainly {
        return true;
    }
    let channel = track
        .artists
        .first()
        .map(|artist| artist.name.as_str())
        .unwrap_or_default();
    if named_by_producer(channel) {
        return true;
    }
    crate::stats::soundcloud_genre(&track.uri).is_some_and(|genre| {
        let genre = genre.to_lowercase();
        genre.contains("beat") || genre.contains("instrumental")
    })
}

/// Lower-cased words between single spaces, so a phrase is matched whole:
/// `(Prod. X)` holds ` prod `, while `Product` does not.
fn spaced_words(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(' ');
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if !word.is_empty() {
            out.extend(word.chars().flat_map(char::to_lowercase));
            out.push(' ');
        }
    }
    out
}

fn holds(haystack: &str, needle: &str) -> bool {
    haystack.contains(&format!(" {needle} "))
}

/// Whether a title itself names a beat: a beat word, a producer credit, or a
/// tempo or key.
fn says_beat(title: &str) -> bool {
    const BEAT_SAID: &[&str] = &[
        "type beat",
        "typebeat",
        "type instrumental",
        "freestyle beat",
        "free beat",
        "free for profit",
        "instrumental",
        "instrumentals",
        "beat",
        "beats",
        "prod",
        "prod by",
        "produced by",
    ];
    let words = spaced_words(title);
    BEAT_SAID.iter().any(|said| holds(&words, said))
        || bpm_of(title).is_some()
        || key_of(title).is_some()
}

/// Whether a channel's name is a producer's: "Sanche Beats", "Prod. Someone".
/// "Beats by Dre" is not.
fn named_by_producer(channel: &str) -> bool {
    let name = spaced_words(channel);
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    let mut parts = name.split(' ');
    let first = parts.next().unwrap_or_default();
    let last = parts.next_back().unwrap_or_default();
    matches!(
        last,
        "beat" | "beats" | "beatz" | "instrumental" | "instrumentals"
    ) || (first == "prod" && !last.is_empty())
}

/// How a beat's title was read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeatTitle {
    /// The beat's own name: `tic tac`, `Smoked`. Never empty.
    pub name: String,
    /// The artists it's made in the style of, without "type beat".
    pub for_artists: Vec<String>,
    /// Its genre or mood, the first one the title states: `Trap`, `UK rap`.
    pub style: Option<String>,
    /// The tempo the producer stated.
    pub bpm: Option<u16>,
    /// The key the producer stated, normalised: `C♯m`.
    pub key: Option<String>,
}

impl BeatTitle {
    /// The second line under the name: who it's for, else its style.
    pub fn secondary(&self) -> Option<String> {
        if self.for_artists.is_empty() {
            return self.style.clone();
        }
        Some(format!("for {}", self.for_artists.join(" × ")))
    }
}

/// The words that say how a beat sounds rather than who it's for.
const GENRE: [&str; 57] = [
    "rap",
    "trap",
    "drill",
    "jazz",
    "soul",
    "boom",
    "bap",
    "funk",
    "rnb",
    "lofi",
    "lo-fi",
    "ambient",
    "phonk",
    "dancehall",
    "reggaeton",
    "reggae",
    "latin",
    "afro",
    "afrobeat",
    "afrobeats",
    "pop",
    "hyperpop",
    "rock",
    "metal",
    "punk",
    "emo",
    "cloud",
    "plugg",
    "rage",
    "jersey",
    "club",
    "house",
    "techno",
    "edm",
    "dubstep",
    "dnb",
    "garage",
    "grime",
    "uk",
    "us",
    "gospel",
    "orchestral",
    "cinematic",
    "classical",
    "piano",
    "guitar",
    "strings",
    "london",
    "chill",
    "dark",
    "melodic",
    "emotional",
    "moody",
    "dreamy",
    "bounce",
    "sample",
    "samples",
];

/// The words that carry nothing once the title's ideas are apart: tags,
/// shout-outs and the keyword stacks producers repeat after "|".
const FILLER: [&str; 39] = [
    "type",
    "beat",
    "beats",
    "instrumental",
    "instrumentals",
    "freestyle",
    "free",
    "profit",
    "for",
    "dl",
    "download",
    "new",
    "prod",
    "prods",
    "produced",
    "hard",
    "raw",
    "real",
    "best",
    "hot",
    "fire",
    "dope",
    "super",
    "ultimate",
    "insane",
    "crazy",
    "ft",
    "feat",
    "remix",
    "mix",
    "edit",
    "official",
    "audio",
    "video",
    "hq",
    "hd",
    "4k",
    "vol",
    "part",
];

/// The bracketed tags a beat's title opens or trails with.
const TAG_NOISE: [&str; 14] = [
    "free",
    "profit",
    "dl",
    "download",
    "bpm",
    "key",
    "keys",
    "prod",
    "remix",
    "edit",
    "instrumental",
    "new",
    "type",
    "beat",
];

/// The phrases that stand for a beat rather than name its content.
const BEAT_WORDS: [&str; 7] = [
    "type beat",
    "typebeat",
    "type instrumental",
    "freestyle beat",
    "instrumental",
    "instrumentals",
    "beat",
];

/// Reads a beat's title, and its description where the producer stated the
/// tempo or the key. The channel, when known, is dropped from who the beat
/// is "for": the producer is already shown as the uploader. Nothing leaves
/// this function and the result never depends on anything outside it.
pub fn read(title: &str, description: Option<&str>, channel: Option<&str>) -> BeatTitle {
    let mut beat = BeatTitle {
        name: own_name(title, channel),
        ..Default::default()
    };
    // Who the beat is for and how it sounds come from the title's ideas that
    // describe a beat. One style, the first it states; the same artist is not
    // named twice.
    for idea in ideas(title) {
        let Some(head) = before_beat_words(&idea) else {
            continue;
        };
        for phrase in phrases(head) {
            if said_by_the_producer(&phrase, channel) {
                continue;
            }
            let words = kept(&phrase);
            if words.is_empty() {
                continue;
            }
            let how = words.iter().any(|word| is_genre(word));
            if how {
                if beat.style.is_none() {
                    beat.style = Some(styled(&words));
                }
            } else {
                let artist = titled(&words);
                if !beat
                    .for_artists
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(&artist))
                {
                    beat.for_artists.push(artist);
                }
            }
        }
    }
    (beat.bpm, beat.key) = tempo(title, description);
    // A name is never empty when the title offered anything at all.
    if beat.name.trim().is_empty() && !title.trim().is_empty() {
        beat.name = title.trim().to_owned();
    }
    beat
}

/// Reads a beat's own name onto a track just built from YouTube or
/// SoundCloud, if it is one. The uri and the artists stay: the producer
/// remains who made it, and nothing that matches by uri or title is moved.
pub fn apply(track: &mut Track, description: Option<&str>) {
    if !is_beat(track) {
        return;
    }
    let channel = track
        .artists
        .first()
        .map(|artist| artist.name.as_str())
        .filter(|name| !name.is_empty());
    let beat = read(&track.name, description, channel);
    track.name = beat.name.clone();
    track.beat = Some(Box::new(beat));
}

/// What the interface shows under a beat's name, "for Young Nudy × Lucki" or
/// its style. `None` when the title holds nothing beyond the beat's name.
pub fn for_line(track: &Track) -> Option<String> {
    track.beat.as_deref()?.secondary()
}

/// The tempo and key a beat states, as one small label: `140 BPM · C♯m`.
pub fn tempo_key(track: &Track) -> Option<String> {
    let beat = track.beat.as_deref()?;
    match (beat.bpm, beat.key.as_deref()) {
        (Some(bpm), Some(key)) => Some(format!("{bpm} BPM · {key}")),
        (Some(bpm), None) => Some(format!("{bpm} BPM")),
        (None, Some(key)) => Some(key.to_owned()),
        (None, None) => None,
    }
}

/// The words of a phrase, as they were written, its noise dropped.
type Words = Vec<String>;

/// The title apart into its ideas: dividers cut, tags and the quoted name
/// removed, so what is left describes the beat rather than repeating
/// everything its name already says.
fn ideas(title: &str) -> Vec<String> {
    without_quoted(title)
        .replace(['|', '–', '—', '/', '~'], " - ")
        .split(" - ")
        .map(strip_tags)
        .filter(|idea| !idea.trim().is_empty())
        .collect()
}

/// The idea in front of a beat keyword, when the segment holds one: who or
/// what the beat is described by. `None` when the segment is not describing
/// a beat.
fn before_beat_words(idea: &str) -> Option<&str> {
    let lower = idea.to_lowercase();
    BEAT_WORDS
        .iter()
        .filter_map(|words| lower.find(words))
        .min()
        .map(|at| &idea[..at])
}

/// An idea without its bracketed tags; a tag holding only what says nothing
/// (`[FREE]`, `(prod. by ...)`) goes, other brackets stay.
fn strip_tags(idea: &str) -> String {
    let mut out = String::with_capacity(idea.len());
    let mut rest = idea;
    while let Some(open) = rest.find(['(', '[', '{']) {
        let Some(close) = rest[open + 1..].find([')', ']', '}']) else {
            break;
        };
        let close = open + close + 1;
        let inside = rest[open + 1..close].to_lowercase();
        let words: Vec<&str> = inside.split(|c: char| !c.is_alphanumeric()).collect();
        let noise = TAG_NOISE.iter().any(|word| words.contains(word))
            || BEAT_WORDS.iter().any(|beat| words.contains(beat));
        if !noise {
            break;
        }
        out.push_str(&rest[..open]);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The title without its quoted spans, which name the beat itself. An
/// apostrophe inside a word (`80's`) is not a quote.
fn without_quoted(title: &str) -> String {
    const PAIRS: [(char, char); 4] = [
        ('"', '"'),
        ('\u{201C}', '\u{201D}'),
        ('\'', '\''),
        ('\u{2018}', '\u{2019}'),
    ];
    let mut out = String::with_capacity(title.len());
    let mut chars = title.chars().peekable();
    let mut previous = ' ';
    while let Some(c) = chars.next() {
        let opens = |previous: char| {
            PAIRS
                .iter()
                .find(|(open, _)| *open == c)
                .filter(|_| c != '\'' || !previous.is_alphanumeric())
        };
        if let Some((_, close)) = opens(previous) {
            for next in chars.by_ref() {
                if next == *close {
                    out.push(' ');
                    break;
                }
            }
        } else {
            out.push(c);
        }
        previous = c;
    }
    out
}

/// A beat's own name: the quoted span when the title holds one, else the
/// first idea left over that is not a beat keyword or the producer. An
/// apostrophe inside a word (`80's`) is not a quote.
fn own_name(title: &str, channel: Option<&str>) -> String {
    const PAIRS: [(char, char); 4] = [
        ('"', '"'),
        ('\u{201C}', '\u{201D}'),
        ('\'', '\''),
        ('\u{2018}', '\u{2019}'),
    ];
    let chars = title.chars().collect::<Vec<_>>();
    let mut previous = ' ';
    for (index, &c) in chars.iter().enumerate() {
        let Some((_, close)) = PAIRS
            .iter()
            .find(|(open, _)| *open == c)
            .filter(|_| c != '\'' || !previous.is_alphanumeric())
        else {
            previous = c;
            continue;
        };
        let mut name = String::new();
        let mut closed = false;
        for &next in &chars[index + 1..] {
            if next == *close {
                closed = true;
                break;
            }
            name.push(next);
        }
        if closed && !name.trim().is_empty() {
            return sentence_case(&name);
        }
        previous = c;
    }
    for idea in ideas(title) {
        if before_beat_words(&idea).is_some() {
            // The idea describes the beat; what it says goes under the name.
            continue;
        }
        let words: Words = kept(&idea)
            .into_iter()
            .filter(|word| !said_by_the_producer(word, channel))
            .collect();
        if !words.is_empty() {
            return sentence_case(&words.join(" "));
        }
    }
    // Nothing of its own to find: the title stands, minus its tags.
    sentence_case(&strip_tags(title))
}

/// Whether a phrase is the producer naming themselves.
fn said_by_the_producer(phrase: &str, channel: Option<&str>) -> bool {
    channel.is_some_and(|who| phrase.trim().eq_ignore_ascii_case(who.trim()))
}

/// A name in caps is a producer shouting, not part of the name: `FAMIGLIA`
/// reads as `Famiglia`. A name that already mixes its case is left alone.
fn sentence_case(name: &str) -> String {
    let shouting = name.chars().any(char::is_alphabetic)
        && !name.chars().any(|c| c.is_lowercase() && c.is_alphabetic());
    if !shouting {
        return name.to_owned();
    }
    name.split_whitespace()
        .map(|word| capitalise(word, true))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first letter up, the rest as they are (`true`) or lower (`false`).
fn capitalise(word: &str, lower: bool) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => {
            let rest = chars.as_str();
            format!(
                "{}{}",
                first.to_uppercase(),
                if lower {
                    rest.to_lowercase()
                } else {
                    rest.to_owned()
                }
            )
        }
        None => String::new(),
    }
}

/// The phrases an idea holds: who the beat is for, or how it sounds, apart
/// at "x", "&" and commas.
fn phrases(head: &str) -> Vec<String> {
    let chars: Vec<char> = head.chars().collect();
    let mut parts: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    for i in 0..chars.len() {
        let apart = matches!(chars[i], 'x' | 'X' | '\u{D7}' | '&')
            && (i == 0 || chars[i - 1] == ' ')
            && (i == chars.len() - 1 || chars[i + 1] == ' ');
        if chars[i] == ',' || chars[i] == '\u{D7}' || apart {
            if i > start {
                parts.push((start, i));
            }
            start = i + 1;
        }
    }
    if chars.len() > start {
        parts.push((start, chars.len()));
    }
    parts
        .into_iter()
        .map(|(from, to)| chars[from..to].iter().collect::<String>())
        .filter(|part| !part.trim().is_empty())
        .collect()
}

/// The words that stay: tags, fillers, years, tempos, keys, producer credits
/// and emoji dropped. The words keep the case they were written in, so
/// names show as they were.
fn kept(phrase: &str) -> Words {
    let mut words = Vec::new();
    'words: for word in phrase.split_whitespace() {
        let bare: String = word
            .trim_matches(|c: char| !c.is_alphanumeric() && !keeps(c))
            .chars()
            .filter(|c| c.is_alphanumeric() || keeps(*c))
            .collect();
        if bare.is_empty() {
            continue;
        }
        let lower = bare.to_lowercase();
        if FILLER.contains(&lower.as_str()) {
            continue;
        }
        for lead in ["prod", "produced"] {
            if let Some(rest) = lower.strip_prefix(lead) {
                let rest = rest.trim_start_matches(['.', ' ', 'b', 'y']);
                if rest.is_empty() {
                    // The producer's credit runs to the end of the phrase.
                    break 'words;
                }
            }
        }
        if says_tempo(&lower) || is_generic_key(&lower) {
            continue;
        }
        words.push(bare);
    }
    words
}

/// The characters that survive inside a kept word, with the alphanumerics.
fn keeps(c: char) -> bool {
    matches!(c, '&' | '#' | '\u{266F}' | '\u{266D}')
}

/// Whether a word states a tempo: a year, "BPM", "140 BPM" as one word.
fn says_tempo(lower: &str) -> bool {
    if lower == "bpm" || lower == "key" || lower == "keys" {
        return true;
    }
    if let Some(digits) = lower.strip_suffix("bpm") {
        return !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
    }
    // A year the title carries as a number of its own.
    lower.len() == 4 && lower.starts_with(['1', '2']) && lower.chars().all(|c| c.is_ascii_digit())
}

/// Whether a word is a key signature on its own: `c#m`, `f#`, `cmin`. A bare
/// note letter is not, for it is half the alphabet's words.
fn is_generic_key(lower: &str) -> bool {
    let mut chars = lower.chars();
    if !matches!(chars.next(), Some('a'..='g')) {
        return false;
    }
    let rest: Vec<char> = chars.collect();
    let rest: String = match rest.first() {
        Some('#' | 'b' | '\u{266F}' | '\u{266D}') => rest[1..].iter().collect(),
        Some(_) => rest.iter().collect(),
        None => return false,
    };
    matches!(rest.as_str(), "" | "m" | "min" | "minor" | "maj" | "major")
}

/// Whether a word says how a beat sounds rather than who it is for.
fn is_genre(word: &str) -> bool {
    let word = word.to_lowercase();
    GENRE.contains(&word.as_str()) || word == "r&b"
}

/// Artist names as they show: a word already written with its case keeps it,
/// a shouted word settles down (`BABY KEEM` reads as `Baby Keem`, an
/// initialism like `JID` stays), a lowercase one is capitalised.
fn titled(words: &Words) -> String {
    words
        .iter()
        .map(|word| {
            let shouted =
                word.chars().any(char::is_alphabetic) && !word.chars().any(char::is_lowercase);
            if shouted && word.chars().count() > 3 {
                capitalise(word, true)
            } else if word.chars().any(char::is_lowercase) {
                capitalise(word, false)
            } else {
                word.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A style as it shows: `Chill rap`, `UK rap`, `Soul boom bap`.
fn styled(words: &Words) -> String {
    words
        .iter()
        .enumerate()
        .map(|(index, word)| match word.to_lowercase().as_str() {
            "uk" => "UK".to_owned(),
            "us" => "US".to_owned(),
            "rnb" => "R&B".to_owned(),
            lower if index == 0 => capitalise(lower, true),
            lower => lower.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The tempo and key a title or its description states. The title wins.
fn tempo(title: &str, description: Option<&str>) -> (Option<u16>, Option<String>) {
    let mut bpm = bpm_of(title);
    let mut key = key_of(title);
    if let Some(text) = description {
        bpm = bpm.or_else(|| bpm_of(text));
        key = key.or_else(|| key_of(text));
    }
    (bpm, key)
}

/// The tempo a text states: `140 BPM`, `BPM: 140` or `140BPM`.
fn bpm_of(text: &str) -> Option<u16> {
    let lower = text.to_lowercase();
    let mut at = 0;
    while let Some(found) = lower[at..].find("bpm") {
        let start = at + found;
        let read = number_before(&lower, start).or_else(|| number_after(&lower, start + 3));
        if read.is_some() {
            return read;
        }
        at = start + 3;
    }
    None
}

/// The digits that end just before `at`, a little space or punctuation in
/// between: the `140` of `140 BPM`.
fn number_before(text: &str, at: usize) -> Option<u16> {
    let bytes = text.as_bytes();
    let mut idx = at.min(bytes.len());
    let mut skipped = 0;
    while idx > 0
        && skipped < 4
        && matches!(bytes[idx - 1], b' ' | b'\t' | b':' | b'.' | b',' | b'-')
    {
        idx -= 1;
        skipped += 1;
    }
    let end = idx;
    while idx > 0 && bytes[idx - 1].is_ascii_digit() {
        idx -= 1;
    }
    (idx < end)
        .then(|| text[idx..end].parse().ok())
        .flatten()
        .filter(|bpm| (20..=400).contains(bpm))
}

/// The digits that start just after `at`: the `140` of `BPM: 140`.
fn number_after(text: &str, at: usize) -> Option<u16> {
    let bytes = text.as_bytes();
    let mut idx = at;
    let mut skipped = 0;
    while idx < bytes.len()
        && skipped < 4
        && matches!(bytes[idx], b' ' | b'\t' | b':' | b'.' | b',' | b'-')
    {
        idx += 1;
        skipped += 1;
    }
    let start = idx;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    (start < idx)
        .then(|| text[start..idx].parse().ok())
        .flatten()
        .filter(|bpm| (20..=400).contains(bpm))
}

/// The key a text states: `Key: C# minor`, `F#m`, `Cmin`, `C#`.
fn key_of(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let says = |at: usize, word: &str| {
        word.chars()
            .enumerate()
            .all(|(offset, c)| chars.get(at + offset) == Some(&c))
    };
    // Said after "key", a bare note or a lone "m" is enough: `Key: C`, `Key:
    // Am`. Nowhere else: ordinary words would read as keys.
    let mut at = 0;
    while let Some(found) = chars[at..]
        .windows(3)
        .position(|window| window == ['k', 'e', 'y'].as_slice())
    {
        let start = at + found;
        at = start + 3;
        if chars.get(start + 3).is_some_and(|c| c.is_alphanumeric()) {
            // "keyboard", "keys" said another way.
            continue;
        }
        let mut idx = start + 3;
        let mut skipped = 0;
        while idx < chars.len() && skipped < 16 {
            if chars[idx].is_alphabetic() && !says(idx, "signature") {
                break;
            }
            if !chars[idx].is_alphanumeric() {
                skipped += 1;
            }
            idx += 1;
        }
        if let Some(key) = read_key(&chars, idx, true) {
            return Some(key);
        }
    }
    // Without "key" having said it, the note must state itself: an accidental
    // (`C#`, `Bb`) or a mode spelled out (`F#m`, `Cmin`, `B♭ major`).
    for idx in 0..chars.len() {
        if idx > 0 && (chars[idx - 1].is_alphanumeric() || chars[idx - 1] == '#') {
            continue;
        }
        if let Some(key) = read_key(&chars, idx, false) {
            return Some(key);
        }
    }
    None
}

/// A key signature at `idx`, when the letters there say one: a note A to G,
/// an optional accidental, an optional mode, ending at a word boundary.
/// A bare note, or one with a lone `m`, only counts where "key" said it.
fn read_key(chars: &[char], idx: usize, name_only: bool) -> Option<String> {
    let letter = chars.get(idx)?.to_ascii_uppercase();
    if !matches!(letter, 'A'..='G') {
        return None;
    }
    let mut next = idx + 1;
    let mut accidental = ' ';
    let flat = matches!(chars.get(next), Some('b' | '\u{266D}'))
        && (next + 1 >= chars.len() || !chars.get(next + 1).is_some_and(|c| c.is_alphanumeric()));
    match chars.get(next) {
        Some('#' | '\u{266F}') => accidental = '\u{266F}',
        Some(_) if flat => accidental = '\u{266D}',
        _ => {}
    }
    if accidental != ' ' {
        next += 1;
    }
    if chars.get(next) == Some(&' ') {
        next += 1;
    }
    let mut mode = ' ';
    let says = |chars: &[char], at: usize, word: &str| {
        word.chars()
            .enumerate()
            .all(|(offset, c)| chars.get(at + offset) == Some(&c))
    };
    if says(chars, next, "minor") {
        mode = 'm';
        next += 5;
    } else if says(chars, next, "min") {
        mode = 'm';
        next += 3;
    } else if says(chars, next, "major") {
        next += 5;
    } else if says(chars, next, "maj") {
        next += 3;
    } else if says(chars, next, "m") && (name_only || accidental != ' ') {
        mode = 'm';
        next += 1;
    }
    if chars.get(next).is_some_and(|c| c.is_alphanumeric()) {
        return None;
    }
    let enough = name_only || accidental != ' ' || mode == 'm';
    if !enough {
        return None;
    }
    let mut key = String::new();
    key.push(letter);
    if accidental != ' ' {
        key.push(accidental);
    }
    if mode != ' ' {
        key.push(mode);
    }
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name_of(title: &str) -> String {
        read(title, None, None).name
    }

    fn line_of(title: &str) -> Option<String> {
        read(title, None, None).secondary()
    }

    #[test]
    fn a_beat_shows_its_own_name_and_who_it_is_for() {
        let beat = read(
            r#"[FREE] young nudy x lucki type beat "tic tac""#,
            None,
            Some("ayohighflyer"),
        );
        assert_eq!(beat.name, "tic tac");
        assert_eq!(beat.for_artists, ["Young Nudy", "Lucki"]);
        assert_eq!(beat.style, None);
        assert_eq!(beat.secondary().as_deref(), Some("for Young Nudy × Lucki"));
    }

    #[test]
    fn a_keyword_stack_leaves_one_style_and_the_quoted_name() {
        let beat = read(
            r#"TRAP TYPE BEAT - "SNAKE" | FREESTYLE BEAT | HARD TYPE BEAT 2026"#,
            None,
            Some("STORM"),
        );
        assert_eq!(beat.name, "Snake");
        assert!(beat.for_artists.is_empty());
        assert_eq!(beat.style.as_deref(), Some("Trap"));
        assert_eq!(beat.secondary().as_deref(), Some("Trap"));

        let beat = read(
            r#""GOSPEL" | Trap Type Beat | Freestyle Beat | Type Beat | Hard ..."#,
            None,
            None,
        );
        assert_eq!(beat.name, "Gospel");
        assert_eq!(beat.style.as_deref(), Some("Trap"));
    }

    #[test]
    fn caps_names_lose_the_shouting_not_the_name() {
        let beat = read(
            r#"(FREE) BABY KEEM x JID TYPE BEAT "FAMIGLIA" | CHILL RAP TYPE BEAT"#,
            None,
            Some("nolzy"),
        );
        assert_eq!(beat.name, "Famiglia");
        assert_eq!(beat.for_artists, ["Baby Keem", "JID"]);
        assert_eq!(beat.style.as_deref(), Some("Chill rap"));
        assert_eq!(beat.secondary().as_deref(), Some("for Baby Keem × JID"));
    }

    #[test]
    fn a_producer_named_as_the_one_its_for_stays_out() {
        let beat = read(
            r#"[FREE] Veeze x Lucki Type Beat "Tweaker""#,
            None,
            Some("99LA"),
        );
        assert_eq!(beat.name, "Tweaker");
        assert_eq!(beat.for_artists, ["Veeze", "Lucki"]);

        let alone = read(
            "Ayohighflyer type beat \"Tic Tac\"",
            None,
            Some("Ayohighflyer"),
        );
        assert_eq!(alone.name, "Tic Tac");
        assert!(alone.for_artists.is_empty());
        assert_eq!(alone.secondary(), None);
    }

    #[test]
    fn a_style_blend_reads_as_a_style_not_as_artists() {
        let beat = read(
            r#"(FREE FOR PROFIT) Soul Boom Bap x Chill Jazz Type Beat - "Smoked""#,
            None,
            Some("Rama Low"),
        );
        assert_eq!(beat.name, "Smoked");
        assert!(beat.for_artists.is_empty());
        assert_eq!(beat.style.as_deref(), Some("Soul boom bap"));
        assert_eq!(beat.secondary().as_deref(), Some("Soul boom bap"));
    }

    #[test]
    fn a_style_drops_its_fillers_years_and_the_keyword() {
        let beat = read(
            r#""Losing You" | UK Real Rap Instrumental 2026"#,
            None,
            None,
        );
        assert_eq!(beat.name, "Losing You");
        assert!(beat.for_artists.is_empty());
        assert_eq!(beat.style.as_deref(), Some("UK rap"));

        let beat = read(r#"Dark Trap Type Beat "Aurora" 🔥🔥 2026"#, None, None);
        assert_eq!(beat.name, "Aurora");
        assert_eq!(beat.style.as_deref(), Some("Dark trap"));
    }

    #[test]
    fn an_apostrophe_inside_a_word_is_not_a_quote() {
        let beat = read("90's Boom Bap Type Beat \"Rewind\"", None, None);
        assert_eq!(beat.name, "Rewind");
        assert_eq!(beat.secondary().as_deref(), Some("90s boom bap"));

        let beat = read("Smoky Type Beat - 'Till Dawn'", None, None);
        assert_eq!(beat.name, "Till Dawn");

        let beat = read("80's Vibes", None, None);
        assert_eq!(beat.name, "80s Vibes");
    }

    #[test]
    fn a_short_title_still_has_its_name() {
        assert_eq!(name_of(r#"Type Beat "Churka""#), "Churka");
        assert_eq!(line_of(r#"Type Beat "Churka""#), None);
    }

    #[test]
    fn curly_quotes_and_ampersands_read_the_same() {
        let beat = read(
            "\u{201C}Midnight\u{201D} | Veeze \u{D7} Lucki Type Beat \u{2013} \u{2018}freestyle\u{2019}",
            None,
            None,
        );
        assert_eq!(beat.name, "Midnight");
        assert_eq!(beat.for_artists, ["Veeze", "Lucki"]);

        let beat = read(
            "Young Nudy & Lucki Type Beat \u{201C}Dreams\u{201D}",
            None,
            None,
        );
        assert_eq!(beat.for_artists, ["Young Nudy", "Lucki"]);
        assert_eq!(beat.name, "Dreams");
    }

    #[test]
    fn a_producer_credit_is_dropped() {
        let beat = read(
            "Trap Type Beat | Dreams (prod. by Ayohighflyer)",
            None,
            Some("Ayohighflyer"),
        );
        assert_eq!(beat.name, "Dreams");
        assert_eq!(beat.style.as_deref(), Some("Trap"));
        assert!(beat.for_artists.is_empty());
    }

    #[test]
    fn without_a_quoted_name_the_best_phrase_left_stands() {
        let beat = read("Tic Tac - free trap type beat 2026", None, None);
        assert_eq!(beat.name, "Tic Tac");
        assert_eq!(beat.style.as_deref(), Some("Trap"));
        assert_eq!(beat.secondary().as_deref(), Some("Trap"));

        // Nothing to read: the title itself stands, never an empty name.
        assert_eq!(name_of("midnight"), "midnight");
        assert_eq!(name_of(" "), " ");
        assert_eq!(name_of(""), "");
    }

    #[test]
    fn the_tempo_and_key_are_read_from_the_title_and_the_description() {
        let from_title = read(
            r#"Chill Type Beat "Aurora" 140 BPM"#,
            Some("Made with love."),
            None,
        );
        assert_eq!(from_title.bpm, Some(140));
        assert_eq!(from_title.style.as_deref(), Some("Chill"));

        let from_text = read(
            "aurora - trap type beat",
            Some("BPM: 140\nKey: C# minor\nLeave a like!"),
            None,
        );
        assert_eq!(from_text.bpm, Some(140));
        assert_eq!(from_text.key.as_deref(), Some("C\u{266F}m"));

        let compact = read("aurora type beat", Some("90bpm | F#m"), None);
        assert_eq!(compact.bpm, Some(90));
        assert_eq!(compact.key.as_deref(), Some("F\u{266F}m"));

        let spelled = read("aurora", Some("tempo 140 bpm, key Cmin"), None);
        assert_eq!(spelled.bpm, Some(140));
        assert_eq!(spelled.key.as_deref(), Some("Cm"));

        let flat = read("aurora", Some("key: bb major"), None);
        assert_eq!(flat.key.as_deref(), Some("B\u{266D}"));
    }

    #[test]
    fn multibyte_text_around_a_key_is_read_without_panicking() {
        // Real upload descriptions carry zero-width spaces and curly quotes
        // wrapped around "key"; walking them by byte once cut a character in
        // half and crashed the app seconds after start-up.
        let description = "\u{200b}140 bpm \u{200b}Key\u{200b}\u{200b}: C# minor\nprod. by me";
        let beat = read("aurora", Some(description), None);
        assert_eq!(beat.bpm, Some(140));
        assert_eq!(beat.key.as_deref(), Some("C\u{266F}m"));

        assert_eq!(
            key_of("prod \u{201c}key\u{201d}: F#m\u{201d}\u{2026}").as_deref(),
            Some("F\u{266F}m")
        );
    }

    #[test]
    fn the_tempo_and_key_lead_one_small_label() {
        let mut track = Track {
            beat: Some(Box::new(BeatTitle {
                bpm: Some(140),
                key: Some("C\u{266F}m".into()),
                ..Default::default()
            })),
            ..Default::default()
        };
        assert_eq!(
            tempo_key(&track).as_deref(),
            Some("140 BPM \u{b7} C\u{266F}m")
        );
        track.beat.as_mut().unwrap().key = None;
        assert_eq!(tempo_key(&track).as_deref(), Some("140 BPM"));
        track.beat.as_mut().unwrap().bpm = None;
        assert_eq!(tempo_key(&track), None);
        track.beat = None;
        assert_eq!(tempo_key(&track), None);
    }

    #[test]
    fn years_and_ordinary_words_are_not_tempos() {
        assert_eq!(bpm_of("Trap Type Beat 2026"), None);
        assert_eq!(bpm_of("Made in 2026"), None);
        assert_eq!(key_of("Never Too Much (Official Audio)"), None);
        assert_eq!(key_of("copyright 2026 am"), None);
        assert_eq!(bpm_of("BPM: 2026"), None);
    }

    #[test]
    fn a_plain_upload_title_reads_its_own_name() {
        // Where `is_beat` never says beat none of this runs, and when it does,
        // a plain title keeps its own name.
        let beat = read(
            "Night Loop (Instrumental) - extended arrangement",
            None,
            Some("Fixture Producer"),
        );
        assert_eq!(beat.name, "Night Loop");
        assert_eq!(beat.for_artists, Vec::<String>::new());
        assert_eq!(beat.style, None);
        assert_eq!(beat.secondary(), None);
    }

    /// Titles and lengths from YouTube as they really are: beats on one side,
    /// and the songs, concerts, tutorials and sets that used to slip through on
    /// the other. A beat has to say so, and a known length has to fit one beat.
    #[test]
    fn only_a_beat_reads_as_a_beat() {
        struct Case {
            title: &'static str,
            channel: &'static str,
            seconds: u32,
            beat: bool,
            why: &'static str,
        }
        let cases = [
            // Beats, said by the title.
            Case {
                title: "[FREE] Rod Wave Type Beat \"Nostalgia\"",
                channel: "Ayohighflyer",
                seconds: 186,
                beat: true,
                why: "type beat",
            },
            Case {
                title: "Drake x Future TYPE BEAT 2026",
                channel: "STORM",
                seconds: 201,
                beat: true,
                why: "type beat",
            },
            Case {
                title: "Dark Trap Instrumental",
                channel: "XarBeats",
                seconds: 174,
                beat: true,
                why: "instrumental",
            },
            Case {
                title: "[FREE FOR PROFIT] Soul Boom Bap x Chill Jazz Type Beat",
                channel: "Rama Low",
                seconds: 192,
                beat: true,
                why: "free for profit and type beat",
            },
            Case {
                title: "Hard Freestyle Instrumental 140 BPM",
                channel: "STORM",
                seconds: 158,
                beat: true,
                why: "instrumental and tempo",
            },
            Case {
                title: "sad piano beat in C minor key",
                channel: "someone",
                seconds: 165,
                beat: true,
                why: "beat and key",
            },
            Case {
                title: "Midnight (prod. Someone)",
                channel: "someone",
                seconds: 180,
                beat: true,
                why: "prod.",
            },
            Case {
                title: "Midnight [Prod by Someone]",
                channel: "someone",
                seconds: 181,
                beat: true,
                why: "prod by",
            },
            Case {
                title: "Free Drill Beat",
                channel: "someone",
                seconds: 168,
                beat: true,
                why: "beat",
            },
            Case {
                title: "Cook Up Type Beat",
                channel: "someone",
                seconds: 150,
                beat: true,
                why: "cook up is plainly a type beat",
            },
            Case {
                title: "[FREE] Melodic Type Beat \"VIPER\" | Dark Type Beat",
                channel: "VEEDY BEATS",
                seconds: 210,
                beat: true,
                why: "type beat",
            },
            Case {
                title: "Type Beat \"Churka\"",
                channel: "someone",
                seconds: 145,
                beat: true,
                why: "type beat",
            },
            Case {
                title: "Boom Bap Instrumentals Vol. 2",
                channel: "someone",
                seconds: 200,
                beat: true,
                why: "instrumentals",
            },
            Case {
                title: "Trap Beat 130 BPM C# minor",
                channel: "someone",
                seconds: 160,
                beat: true,
                why: "beat, tempo and key",
            },
            Case {
                title: "Piano Beat 90bpm F#m",
                channel: "someone",
                seconds: 155,
                beat: true,
                why: "beat, compact tempo and key",
            },
            Case {
                title: "Free Beat - \"Pressure\"",
                channel: "someone",
                seconds: 175,
                beat: true,
                why: "free beat",
            },
            Case {
                title: "Jersey Club Instrumental",
                channel: "someone",
                seconds: 140,
                beat: true,
                why: "instrumental",
            },
            Case {
                title: "Acoustic Guitar Type Beat \"On The Edge\"",
                channel: "GAXILLIC",
                seconds: 220,
                beat: true,
                why: "type beat",
            },
            // Beats, said by the producer's channel.
            Case {
                title: "Smoked",
                channel: "Sanche Beats",
                seconds: 190,
                beat: true,
                why: "a producer's channel",
            },
            Case {
                title: "Night Loop",
                channel: "Prod. Someone",
                seconds: 170,
                beat: true,
                why: "a producer's channel",
            },
            Case {
                title: "Afrobeat Groove",
                channel: "Sanche Beats",
                seconds: 200,
                beat: true,
                why: "a producer's channel",
            },
            // Songs and videos, which were the complaint.
            Case {
                title: "Kehlani - Folded [Official Music Video]",
                channel: "Kehlani",
                seconds: 195,
                beat: false,
                why: "official music video",
            },
            Case {
                title: "Blinding Lights (Official Audio)",
                channel: "The Weeknd",
                seconds: 200,
                beat: false,
                why: "official audio",
            },
            Case {
                title: "Song (Lyric Video)",
                channel: "Label",
                seconds: 210,
                beat: false,
                why: "lyric video",
            },
            Case {
                title: "Song (Official Visualizer)",
                channel: "Label",
                seconds: 204,
                beat: false,
                why: "visualizer",
            },
            Case {
                title: "Rick Astley - Never Gonna Give You Up",
                channel: "Rick Astley",
                seconds: 213,
                beat: false,
                why: "a plain song with no beat signal",
            },
            Case {
                title: "Michael Jackson - Beat It",
                channel: "Michael Jackson",
                seconds: 258,
                beat: false,
                why: "a song with beat in its name",
            },
            Case {
                title: "Beatbox cover",
                channel: "someone",
                seconds: 120,
                beat: false,
                why: "a beatbox performance",
            },
            // Concerts and sets.
            Case {
                title: "Tiny Desk Concert: Full Set",
                channel: "NPR Music",
                seconds: 1500,
                beat: false,
                why: "a concert",
            },
            Case {
                title: "Live at Coachella 2026",
                channel: "Festival",
                seconds: 2400,
                beat: false,
                why: "live at a festival",
            },
            Case {
                title: "Summer Festival Full Set",
                channel: "DJ",
                seconds: 3600,
                beat: false,
                why: "a festival set",
            },
            Case {
                title: "Live from the studio - Type Beat session",
                channel: "someone",
                seconds: 600,
                beat: false,
                why: "live from a session",
            },
            // Everything about beats rather than a beat.
            Case {
                title: "Reacting to the hardest type beats",
                channel: "Producer Reacts",
                seconds: 600,
                beat: false,
                why: "a reaction",
            },
            Case {
                title: "Interview: How I Made Type Beat \"X\"",
                channel: "Studio",
                seconds: 480,
                beat: false,
                why: "an interview",
            },
            Case {
                title: "The Beat Podcast Episode 12",
                channel: "Beat Podcast",
                seconds: 3600,
                beat: false,
                why: "a podcast",
            },
            Case {
                title: "How To Make A Travis Scott Type Beat",
                channel: "shonci",
                seconds: 420,
                beat: false,
                why: "a tutorial",
            },
            Case {
                title: "FL Studio type beat tutorial",
                channel: "someone",
                seconds: 500,
                beat: false,
                why: "a tutorial",
            },
            Case {
                title: "Type Beat Breakdown: Dark Trap",
                channel: "someone",
                seconds: 380,
                beat: false,
                why: "a breakdown",
            },
            Case {
                title: "Cook Up",
                channel: "someone",
                seconds: 180,
                beat: false,
                why: "cook up with nothing else",
            },
            Case {
                title: "Beat Saber - Free Songs",
                channel: "someone",
                seconds: 200,
                beat: false,
                why: "a game",
            },
            Case {
                title: "Metronome 140 BPM beat",
                channel: "someone",
                seconds: 300,
                beat: false,
                why: "a metronome",
            },
            // Mixes, compilations and lengths that are not one beat.
            Case {
                title: "Type Beat Mix 2026 | 1 Hour",
                channel: "someone",
                seconds: 3600,
                beat: false,
                why: "a mix",
            },
            Case {
                title: "Beat Compilation Vol. 3",
                channel: "someone",
                seconds: 1200,
                beat: false,
                why: "a compilation",
            },
            Case {
                title: "Night Drive Mix (Live DJ Set)",
                channel: "DJ",
                seconds: 3000,
                beat: false,
                why: "a mix",
            },
            Case {
                title: "Trap Type Beat 2026",
                channel: "someone",
                seconds: 900,
                beat: false,
                why: "longer than ten minutes",
            },
            Case {
                title: "Type Beat Preview",
                channel: "someone",
                seconds: 25,
                beat: false,
                why: "shorter than 45 seconds",
            },
            Case {
                title: "Quick Beat Sketch",
                channel: "someone",
                seconds: 40,
                beat: false,
                why: "shorter than 45 seconds",
            },
            // A YouTube upload that says nothing is not a beat.
            Case {
                title: "Late Night Loop",
                channel: "Fixture Producer",
                seconds: 180,
                beat: false,
                why: "no beat signal on YouTube",
            },
        ];
        for case in cases {
            let track = Track {
                uri: "youtube:video:aB9_-012xyz".into(),
                name: case.title.into(),
                artists: vec![crate::api::models::ArtistRef {
                    name: case.channel.into(),
                    ..Default::default()
                }],
                duration_ms: case.seconds * 1000,
                ..Default::default()
            };
            assert_eq!(
                is_beat(&track),
                case.beat,
                "{} / {} / {}s: {}",
                case.title,
                case.channel,
                case.seconds,
                case.why
            );
        }
    }

    /// A length that is not known cannot rule a beat out; the title still has
    /// to say it is one.
    #[test]
    fn an_unknown_length_is_decided_by_the_title() {
        let beat = Track {
            uri: "youtube:video:aB9_-012xyz".into(),
            name: "Dark Trap Type Beat".into(),
            ..Default::default()
        };
        assert!(is_beat(&beat));
        let unknown = Track {
            uri: "youtube:video:aB9_-012xyz".into(),
            name: "Some upload".into(),
            ..Default::default()
        };
        assert!(!is_beat(&unknown));
    }
}
