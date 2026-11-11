//! Conservative metadata matching. Spotify supplies descriptions, never audio.
//!
//! A candidate passes only when its title is the same song and its version is
//! the one that was asked for. Credits, tags and spelling are read as metadata,
//! never as identity: `Sky (Prod. By Art Dealer)` is `Sky`, but `Sky (Live)` is
//! a different recording. When the evidence is thin the answer is no match.

use std::collections::BTreeSet;

use serde::Deserialize;

use crate::api::models::{ArtistRef, Track};

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Candidate {
    /// The public URL the Siphon helper downloads from.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub uploader: String,
    /// Names the source credits besides the uploader: the publisher artist, a
    /// label's credited artist, a channel. Evidence only, never identity.
    #[serde(default)]
    pub credits: Vec<String>,
    #[serde(default)]
    pub duration: Option<f64>,
    #[serde(default)]
    pub live: bool,
    /// The source verified the uploading account.
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub isrc: Option<String>,
}

/// The same song as a Spotify one: what the player calls it and where the
/// download helper fetches it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Match {
    /// The URI the phone plays: a YouTube link or a `soundcloud:track:` URI.
    pub uri: String,
    /// A public URL the Siphon helper accepts for downloading.
    pub source: String,
}

/// Both places that resolve a Spotify song share this one search: the phone's
/// player and the download queue. Prefer the listing's own title, uploader and
/// length; never fetch each candidate separately.
#[derive(Clone)]
pub(crate) struct Searcher {
    catalogue: crate::youtube::Catalogue,
    soundcloud: Option<crate::soundcloud::api::Client>,
}

impl Searcher {
    pub fn new(
        catalogue: crate::youtube::Catalogue,
        soundcloud: Option<crate::soundcloud::api::Client>,
    ) -> Self {
        Self {
            catalogue,
            soundcloud,
        }
    }

    /// One search on YouTube and SoundCloud; the best listing entry that
    /// passes [`score`] wins. None rather than a near miss.
    pub async fn same_song(&self, wanted: &Track) -> Option<Match> {
        self.same_song_except(wanted, &[]).await
    }

    /// The same song, leaving out every copy in `refused`: ones that were
    /// found and then would not play. Three looks run at once, and only what
    /// can play is weighed:
    ///
    /// - the album on YouTube Music, whose official uploads carry each song's
    ///   exact length and the artist's own channel, so it finds a song that is
    ///   too new or too plain-named for a search to put first;
    /// - a YouTube search for "artist - title";
    /// - a SoundCloud search, without previews and without copies SoundCloud
    ///   only serves encrypted, which never play here.
    ///
    /// Every candidate is judged by the name its source gives it, never the
    /// tidied name the app shows, so "(Instrumental)" and the real uploader
    /// still count. The query carries the primary artist only: a feature
    /// joined into it pulls that artist's catalogue in and buries the upload.
    pub async fn same_song_except(&self, wanted: &Track, refused: &[String]) -> Option<Match> {
        let primary = wanted
            .artists
            .first()
            .map_or_else(String::new, |artist| artist.name.clone());
        let query = format!("{primary} - {}", wanted.name);
        let album = wanted
            .album
            .as_ref()
            .map(|album| album.name.clone())
            .filter(|name| !name.trim().is_empty());
        let resolver = &self.catalogue.resolver;
        let video = crate::youtube::browse::videos_listed(resolver, &query);
        let albums = async {
            match &album {
                Some(album) => crate::youtube::browse::album_songs(resolver, &primary, album).await,
                None => Vec::new(),
            }
        };
        let sound = async {
            match &self.soundcloud {
                Some(client) => client
                    .search_tracks(&query, 30, 0)
                    .await
                    .map(|page| {
                        page.collection
                            .iter()
                            .filter(|track| track.matchable())
                            .map(|track| track.listing_track(None))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        };
        let (videos, albums, sounds) = tokio::join!(video, albums, sound);
        let open = |tracks: Vec<Track>| -> Vec<Track> {
            tracks
                .into_iter()
                .filter(|track| !refused.contains(&track.uri))
                .collect()
        };
        let mut found = open(albums);
        let videos = open(videos.unwrap_or_default());
        found.extend(videos.iter().cloned());
        if let Some(best) = pick(wanted, &found, &open(sounds)) {
            return Some(best);
        }
        self.named_by_release_topic(wanted, &videos).await
    }

    /// YouTube files some official uploads under an unnamed "Release - Topic"
    /// channel, which a search lists without the artist. Their own page names
    /// the artist ("tell you (L2) · jacari"), so the few whose length already
    /// fits are opened and judged by that, one request each.
    async fn named_by_release_topic(&self, wanted: &Track, videos: &[Track]) -> Option<Match> {
        let wanted_secs = f64::from(wanted.duration_ms) / 1000.0;
        let fits = videos.iter().filter(|video| {
            video
                .artists
                .first()
                .is_some_and(|channel| channel.name.ends_with(" - Topic"))
                && (f64::from(video.duration_ms) / 1000.0 - wanted_secs).abs() <= 2.0
        });
        for video in fits.take(TOPIC_PAGES_OPENED) {
            let Some(id) = crate::youtube::video_id(&video.uri) else {
                continue;
            };
            let Ok(page) = self.catalogue.resolver.metadata(id).await else {
                continue;
            };
            let Some((title, artists)) = page.description.as_deref().and_then(topic_credits) else {
                continue;
            };
            let named = Track {
                name: title,
                artists: artists
                    .into_iter()
                    .map(|name| ArtistRef {
                        name,
                        ..Default::default()
                    })
                    .collect(),
                ..video.clone()
            };
            if let Some(found) = pick(wanted, &[named], &[]) {
                return Some(found);
            }
        }
        None
    }

    #[cfg(test)]
    pub fn no_network() -> Self {
        Self {
            catalogue: crate::youtube::Catalogue {
                resolver: Default::default(),
                cache: std::path::PathBuf::new(),
                links: std::sync::Arc::new(crate::youtube::Links::default()),
            },
            soundcloud: None,
        }
    }
}

/// How many "Release - Topic" pages one search opens at most.
const TOPIC_PAGES_OPENED: usize = 2;

/// The title and artists an auto-generated upload's description names, on the
/// line after "Provided to YouTube by …": "tell you (L2) · jacari".
fn topic_credits(description: &str) -> Option<(String, Vec<String>)> {
    let mut lines = description
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    lines
        .next()?
        .starts_with("Provided to YouTube by")
        .then_some(())?;
    let mut parts = lines.next()?.split(" · ").map(str::trim);
    let title = parts.next().filter(|title| !title.is_empty())?.to_owned();
    let artists: Vec<String> = parts
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect();
    (!artists.is_empty()).then_some((title, artists))
}

/// The best of the listing entries both sources returned. SoundCloud entries
/// need their public permalink, which is what the download helper fetches.
pub(crate) fn pick(wanted: &Track, videos: &[Track], sounds: &[Track]) -> Option<Match> {
    let mut best: Option<(Verdict, Match)> = None;
    for track in videos.iter().chain(sounds) {
        let Some(candidate) = candidate(track) else {
            continue;
        };
        let Some(found) = evaluate(wanted, &candidate) else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|(current, _)| better(found, *current))
        {
            best = Some((
                found,
                Match {
                    uri: track.uri.clone(),
                    source: candidate.url,
                },
            ));
        }
    }
    best.map(|(_, found)| found)
}

/// A listing entry as the scorer sees it. Its `url` is the download source.
fn candidate(track: &Track) -> Option<Candidate> {
    let url = if crate::youtube::is_track_uri(&track.uri) {
        crate::youtube::permalink(&track.uri)?
    } else if crate::soundcloud::is_track_uri(&track.uri) {
        crate::soundcloud::permalink(track.external_urls.spotify.as_deref())?.to_owned()
    } else {
        return None;
    };
    let mut credits: Vec<String> = track
        .artists
        .iter()
        .map(|artist| artist.name.clone())
        .filter(|name| !name.trim().is_empty())
        .collect();
    if let Some(artist) = track
        .publisher_metadata
        .as_ref()
        .and_then(|metadata| metadata.artist.as_deref())
        .filter(|name| !name.trim().is_empty())
    {
        credits.push(artist.to_owned());
    }
    Some(Candidate {
        url,
        title: track.name.clone(),
        uploader: track
            .artists
            .first()
            .map(|artist| artist.name.clone())
            .unwrap_or_default(),
        credits,
        duration: (track.duration_ms > 0).then(|| f64::from(track.duration_ms) / 1000.0),
        live: false,
        verified: track.verified,
        isrc: track
            .external_ids
            .isrc
            .clone()
            .filter(|isrc| !isrc.trim().is_empty()),
    })
}

#[cfg(test)]
pub(super) fn youtube_url(text: &str) -> Option<String> {
    let url = reqwest::Url::parse(text).ok()?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let id = match url.host_str()? {
        "youtube.com" | "www.youtube.com" | "music.youtube.com" if url.path() == "/watch" => url
            .query_pairs()
            .find(|(key, _)| key == "v")?
            .1
            .into_owned(),
        "youtu.be" => url.path().strip_prefix('/')?.to_string(),
        _ => return None,
    };
    if id.len() != 11
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return None;
    }
    Some(format!("https://www.youtube.com/watch?v={id}"))
}

/// Accented letters grouped under the letter they are read as, so an upload
/// that drops the diacritics still says the same name. Covers the Latin
/// blocks; letters from other scripts are left alone.
const ACCENT_GROUPS: &[(&str, &str)] = &[
    ("a", "àáâãäåāăąǎǟǡǻȁȃȧḁạảấầẩẫậắằẳẵặɑ"),
    ("ae", "æ"),
    ("b", "ḃḅḇƀ"),
    ("c", "çćĉċčḉ"),
    ("d", "ďḋḍḏḑḓđð"),
    ("e", "èéêëēĕėęěȅȇȩḕḗḙḛḝẹẻẽếềểễệ"),
    ("f", "ḟ"),
    ("g", "ĝğġģǧǵḡ"),
    ("h", "ĥȟḣḥḧḩḫẖħ"),
    ("i", "ìíîïĩīĭįİǐȉȋḭḯỉịıɨ"),
    ("j", "ĵǰ"),
    ("k", "ķǩḱḳḵ"),
    ("l", "ĺļľḷḹḻḽł"),
    ("m", "ḿṁṃ"),
    ("n", "ñńņňǹṅṇṉṋŋ"),
    ("o", "òóôõöōŏőơǒǫǭȍȏȫȭȯȱṍṏṑṓọỏốồổỗộớờởỡợø"),
    ("oe", "œ"),
    ("p", "ṕṗ"),
    ("r", "ŕŗřȑȓṙṛṝṟ"),
    ("s", "śŝşšșṡṣṥṧṩʂ"),
    ("ss", "ß"),
    ("t", "ţťțṫṭṯṱẗŧ"),
    ("th", "þ"),
    ("u", "ùúûüũūŭůűųưǔǖǘǚǜȕȗṳṵṷṹṻụủứừửữựʉ"),
    ("v", "ṽṿ"),
    ("w", "ŵẁẃẅẇẉẘ"),
    ("x", "ẋẍ"),
    ("y", "ýÿŷȳẏẙỳỵỷỹ"),
    ("z", "źżžẑẓẕƶ"),
];

fn accent_base(c: char) -> Option<&'static str> {
    ACCENT_GROUPS
        .iter()
        .find(|(_, letters)| letters.contains(c))
        .map(|(base, _)| *base)
}

/// Fold a title or name to the form the gates compare: compatibility forms
/// unified, apostrophes removed, diacritics dropped, lowercase, everything
/// else kept so `$`, `&` and emoji still separate words.
fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\'' | '\u{2018}' | '\u{2019}' | '\u{02BC}' | '\u{00B4}' | '`' => continue,
            '\u{FF01}'..='\u{FF5E}' => {
                if let Some(ascii) = char::from_u32(c as u32 - 0xFEE0) {
                    out.extend(ascii.to_lowercase());
                }
                continue;
            }
            '\u{3000}' => {
                out.push(' ');
                continue;
            }
            _ => {}
        }
        for lower in c.to_lowercase() {
            if let Some(base) = accent_base(lower) {
                out.push_str(base);
            } else {
                out.push(lower);
            }
        }
    }
    out
}

fn tokens_of(text: &str) -> Vec<String> {
    fold(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

fn set(tokens: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    tokens.into_iter().collect()
}

/// The same words with `$` read as `s`, used only when the primary spelling
/// does not line up: `$uicideboy$` and `Suicideboys` are one name.
fn secondary_tokens(text: &str) -> Vec<String> {
    fold(text)
        .replace('$', "s")
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

fn collapsed(tokens: &[String]) -> String {
    tokens.concat()
}

fn coverage(wanted: &BTreeSet<String>, actual: &BTreeSet<String>) -> f64 {
    if wanted.is_empty() {
        return 0.0;
    }
    wanted.intersection(actual).count() as f64 / wanted.len() as f64
}

/// Strong version words can only describe another recording, so they count
/// anywhere in a title, brackets or not.
const STRONG_VERSION_WORDS: &[(&str, &str)] = &[
    ("8d", "8d"),
    ("acapella", "acapella"),
    ("bootleg", "bootleg"),
    ("chopped", "chopped"),
    ("flip", "flip"),
    ("freestyle", "freestyle"),
    ("karaoke", "karaoke"),
    ("mashup", "mashup"),
    ("nightcore", "nightcore"),
    ("refix", "refix"),
    ("recreate", "recreate"),
    ("remake", "remake"),
    ("reverb", "reverb"),
    ("rework", "rework"),
    ("screwed", "screwed"),
    ("slowed", "slowed"),
    ("sped", "speed"),
    ("stems", "stems"),
    ("vip", "vip"),
];

/// Version words that are also ordinary title words. They only count inside a
/// bracketed or dash-separated segment, so `Live Forever` stays the song and
/// `Live Forever (Live)` is the live recording.
const SEGMENT_VERSION_WORDS: &[(&str, &str)] = &[
    ("acoustic", "acoustic"),
    ("clean", "clean"),
    ("cover", "cover"),
    ("demo", "demo"),
    ("edit", "edit"),
    ("extended", "extended"),
    ("instrumental", "instrumental"),
    ("live", "live"),
    ("remaster", "remaster"),
    ("remastered", "remastered"),
    ("remix", "remix"),
    ("tribute", "tribute"),
];

/// Tags that mark an unreleased or unfinished recording. They reject unless
/// the Spotify track carries the same tag, or its ISRC names the recording.
const SOFT_TAGS: &[&str] = &[
    "leak",
    "leaked",
    "og",
    "reference",
    "snippet",
    "unmastered",
    "unreleased",
];

/// A segment carrying one of these is part of the song's identity, not a tag:
/// `Song (Pt. 2)` is not `Song`.
const IDENTITY_MARKERS: &[&str] = &[
    "chapter",
    "interlude",
    "intro",
    "outro",
    "part",
    "pt",
    "v1",
    "v2",
];

/// A credit starts here and runs to the end of its segment.
const CREDIT_TAIL: &[&str] = &[
    "dir",
    "feat",
    "featuring",
    "ft",
    "mastered",
    "mixed",
    "prod",
    "produced",
    "producer",
];

const BOILERPLATE: &[&str] = &[
    "audio",
    "hd",
    "hq",
    "4k",
    "lyric",
    "lyrics",
    "music",
    "official",
    "video",
    "visualizer",
];

/// A title's identity and how it is tagged. Bracketed groups and ` - `
/// separated parts are metadata unless they carry an identity marker.
struct Parsed {
    /// The title's own words in order, for the inflection rule.
    tokens: Vec<String>,
    words: BTreeSet<String>,
    collapsed: String,
    versions: BTreeSet<&'static str>,
    soft: BTreeSet<&'static str>,
}

fn tags_in(tokens: &[String], strong_only: bool) -> BTreeSet<&'static str> {
    let mut tags = BTreeSet::new();
    for pair in tokens.windows(2) {
        let tag = match (pair[0].as_str(), pair[1].as_str()) {
            ("type", "beat") => Some("type beat"),
            ("sped", "up") | ("speed", "up") => Some("speed"),
            ("bass", "boosted") => Some("bass"),
            ("open", "verse") => Some("open verse"),
            ("vocals", "only") => Some("vocals only"),
            ("radio", "edit") => Some("edit"),
            ("extended", "mix") => Some("extended"),
            ("a", "cappella") => Some("acapella"),
            _ => None,
        };
        if let Some(tag) = tag {
            tags.insert(tag);
        }
    }
    for token in tokens {
        if let Some((_, tag)) = STRONG_VERSION_WORDS
            .iter()
            .find(|(word, _)| *word == token.as_str())
        {
            tags.insert(tag);
        }
        if !strong_only
            && let Some((_, tag)) = SEGMENT_VERSION_WORDS
                .iter()
                .find(|(word, _)| *word == token.as_str())
        {
            tags.insert(tag);
        }
    }
    tags
}

fn soft_tags(tokens: &[String]) -> BTreeSet<&'static str> {
    tokens
        .iter()
        .filter_map(|token| {
            SOFT_TAGS
                .iter()
                .find(|tag| **tag == token.as_str())
                .copied()
        })
        .collect()
}

fn has_identity_marker(text: &str) -> bool {
    tokens_of(text)
        .iter()
        .any(|token| IDENTITY_MARKERS.contains(&token.as_str()))
}

/// Bracketed groups, and the text outside them split on ` - `.
fn split_parts(text: &str) -> (Vec<String>, Vec<String>) {
    let mut outside = String::with_capacity(text.len());
    let mut brackets = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' => {
                if depth == 0 {
                    start = index + c.len_utf8();
                }
                depth += 1;
            }
            ')' | ']' | '}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 && index > start {
                        brackets.push(text[start..index].to_owned());
                    }
                }
            }
            _ if depth == 0 => outside.push(c),
            _ => {}
        }
    }
    let chars: Vec<char> = outside.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    for (index, c) in chars.iter().enumerate() {
        let dash = matches!(c, '-' | '\u{2013}' | '\u{2014}');
        let spaced = index > 0
            && chars[index - 1].is_whitespace()
            && chars
                .get(index + 1)
                .is_some_and(|next| next.is_whitespace());
        if dash && spaced {
            if !current.trim().is_empty() {
                parts.push(current.trim().to_owned());
            }
            current.clear();
        } else {
            current.push(*c);
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_owned());
    }
    (parts, brackets)
}

/// Drop a producer or feature credit that runs on from the song name:
/// `Sky prod by Art Dealer` is `Sky`.
fn strip_credit_tail(part: &str) -> &str {
    let mut starts = Vec::new();
    let mut open = None;
    for (index, c) in part.char_indices() {
        if c.is_alphanumeric() {
            if open.is_none() {
                open = Some(index);
            }
        } else if let Some(start) = open.take() {
            starts.push((start, index));
        }
    }
    if let Some(start) = open {
        starts.push((start, part.len()));
    }
    for (start, end) in starts {
        let word = fold(&part[start..end]);
        if CREDIT_TAIL.contains(&word.as_str()) {
            return part[..start].trim_end();
        }
    }
    part
}

fn parse_wanted(name: &str) -> Parsed {
    let (parts, brackets) = split_parts(name);
    let segments: Vec<&str> = brackets
        .iter()
        .map(String::as_str)
        .chain(parts.iter().skip(1).map(String::as_str))
        .collect();
    let mut tokens = parts.first().map_or_else(Vec::new, |part| tokens_of(part));
    if tokens.is_empty() {
        tokens = tokens_of(name);
    }
    for segment in &segments {
        if has_identity_marker(segment) {
            tokens.extend(tokens_of(segment));
        }
    }
    let mut versions = BTreeSet::new();
    for segment in &segments {
        versions.extend(tags_in(&tokens_of(segment), false));
    }
    versions.extend(tags_in(&tokens_of(name), true));
    let all = tokens_of(name);
    Parsed {
        words: set(tokens.iter().cloned()),
        collapsed: collapsed(&tokens),
        tokens,
        versions,
        soft: soft_tags(&all),
    }
}

fn parse_candidate(title: &str, wanted: &BTreeSet<String>, wanted_collapsed: &str) -> Parsed {
    let (raw_parts, brackets) = split_parts(title);
    let parts: Vec<String> = raw_parts
        .iter()
        .map(|part| strip_credit_tail(part).trim().to_owned())
        .filter(|part| !part.is_empty())
        .collect();
    let main = parts
        .iter()
        .enumerate()
        .max_by_key(|(index, part)| {
            let tokens = tokens_of(part);
            let words = set(tokens.iter().cloned());
            let shared = words.intersection(wanted).count();
            let exact = usize::from(collapsed(&tokens) == wanted_collapsed);
            (shared, exact, *index)
        })
        .map(|(index, _)| index);
    let mut tokens = main.map_or_else(Vec::new, |index| tokens_of(&parts[index]));
    let segments: Vec<&str> = brackets
        .iter()
        .map(String::as_str)
        .chain(
            parts
                .iter()
                .enumerate()
                .filter(|(index, _)| Some(*index) != main)
                .map(|(_, part)| part.as_str()),
        )
        .collect();
    for segment in &segments {
        if has_identity_marker(segment) {
            tokens.extend(tokens_of(segment));
        }
    }
    let mut versions = BTreeSet::new();
    for segment in &segments {
        versions.extend(tags_in(&tokens_of(segment), false));
    }
    versions.extend(tags_in(&tokens_of(title), true));
    let all = tokens_of(title);
    Parsed {
        words: set(tokens.iter().cloned()),
        collapsed: collapsed(&tokens),
        tokens,
        versions,
        soft: soft_tags(&all),
    }
}

/// How a passing candidate ranks against another. Score first, then the
/// evidence that it is the released recording: ISRC, a verified account, a
/// publisher credit naming the artist.
#[derive(Clone, Copy)]
struct Verdict {
    score: f64,
    isrc: bool,
    verified: bool,
    credited: bool,
}

fn better(left: Verdict, right: Verdict) -> bool {
    if left.score != right.score {
        return left.score > right.score;
    }
    if left.isrc != right.isrc {
        return left.isrc;
    }
    if left.verified != right.verified {
        return left.verified;
    }
    left.credited && !right.credited
}

fn isrc_equal(track: &Track, candidate: &Candidate) -> bool {
    let wanted = track
        .external_ids
        .isrc
        .as_deref()
        .unwrap_or_default()
        .trim();
    let actual = candidate.isrc.as_deref().unwrap_or_default().trim();
    !wanted.is_empty() && wanted.eq_ignore_ascii_case(actual)
}

/// A credit other than the uploader that names the primary artist: a label or
/// collective account's publisher metadata.
fn publisher_credit(primary: &BTreeSet<String>, candidate: &Candidate) -> bool {
    if primary.is_empty() {
        return false;
    }
    let uploader = fold(&candidate.uploader).trim().to_owned();
    candidate.credits.iter().any(|credit| {
        let folded = fold(credit).trim().to_owned();
        !folded.is_empty()
            && folded != uploader
            && coverage(primary, &set(tokens_of(credit))) >= 1.0
    })
}

fn artist_evidence(candidate: &Candidate, title: &BTreeSet<String>) -> BTreeSet<String> {
    let mut evidence = title.clone();
    evidence.extend(tokens_of(&candidate.uploader));
    for credit in &candidate.credits {
        evidence.extend(tokens_of(credit));
    }
    evidence
}

fn secondary_evidence(candidate: &Candidate, title: &str) -> BTreeSet<String> {
    let mut evidence = set(secondary_tokens(title));
    evidence.extend(secondary_tokens(&candidate.uploader));
    for credit in &candidate.credits {
        evidence.extend(secondary_tokens(credit));
    }
    evidence
}

/// An inflected title scores this much below the same title spelled exactly.
const INFLECTION_PENALTY: f64 = 0.01;

/// Folded, with every space and punctuation mark taken out: "Playboi Carti"
/// is "playboicarti".
fn account_key(text: &str) -> String {
    fold(text).chars().filter(|c| c.is_alphanumeric()).collect()
}

fn same_account(left: &str, right: &str) -> bool {
    let key = account_key(left);
    !key.is_empty() && key == account_key(right)
}

/// An uploader or credit that is the artist's name written without spaces:
/// the account `playboicarti` is Playboi Carti. Equality only, so an account
/// that merely contains the name is not the artist.
fn artist_account(primary: &str, candidate: &Candidate) -> bool {
    same_account(primary, &candidate.uploader)
        || candidate
            .credits
            .iter()
            .any(|credit| same_account(primary, credit))
}

/// One word inflected from another: "go" and "going", "love" and "loved". The
/// shorter starts the longer, has two letters at least, and is at most three
/// letters shorter.
fn inflected_word(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let (shorter, longer) = if left.chars().count() <= right.chars().count() {
        (left, right)
    } else {
        (right, left)
    };
    let (short, long) = (shorter.chars().count(), longer.chars().count());
    short >= 2 && long - short <= 3 && longer.starts_with(shorter)
}

/// The same title with its words inflected. Another word count is another title.
fn inflected_title(wanted: &[String], actual: &[String]) -> bool {
    !wanted.is_empty()
        && wanted.len() == actual.len()
        && wanted
            .iter()
            .zip(actual)
            .all(|(left, right)| inflected_word(left, right))
}

fn symbols_only_artist(wanted: &str, candidate: &Candidate) -> bool {
    let raw = fold(wanted).trim().to_owned();
    if raw.is_empty() {
        return false;
    }
    let same = |text: &str| fold(text).trim() == raw;
    same(&candidate.uploader)
        || same(&candidate.title)
        || candidate.credits.iter().any(|credit| same(credit))
}

fn evaluate(track: &Track, candidate: &Candidate) -> Option<Verdict> {
    if track.duration_ms == 0 || candidate.live {
        return None;
    }
    let primary_name = &track.artists.first()?.name;
    let wanted = parse_wanted(&track.name);
    if wanted.words.is_empty() {
        return None;
    }
    let parsed = parse_candidate(&candidate.title, &wanted.words, &wanted.collapsed);
    if parsed.words.is_empty() {
        return None;
    }
    let duration = candidate.duration.filter(|d| d.is_finite() && *d > 0.0)?;
    let difference = (duration - f64::from(track.duration_ms) / 1000.0).abs();
    if difference > 5.0 {
        return None;
    }
    let isrc = isrc_equal(track, candidate);
    let identity_exact = parsed.collapsed == wanted.collapsed;
    let primary = set(tokens_of(primary_name));
    let credited = publisher_credit(&primary, candidate);
    if difference > 3.0 && !(isrc || (identity_exact && (candidate.verified || credited))) {
        return None;
    }
    // The artist's own account spelling a title word inflected is the same
    // song: "Never Going Down" is "Never Go Down". The uploader itself must be
    // the artist, the lengths must agree within a second and a half, and the
    // versions gate below still applies, so a remix, an instrumental or a fan
    // upload cannot ride on it. It scores just under the title spelled exactly.
    let inflected = same_account(primary_name, &candidate.uploader)
        && difference <= 1.5
        && inflected_title(&wanted.tokens, &parsed.tokens);
    let title_words: BTreeSet<String> = set(tokens_of(&candidate.title));
    let evidence = artist_evidence(candidate, &title_words);
    let all_artists = set(tokens_of(&track.artist_names()));
    let expected: BTreeSet<String> = wanted.words.union(&all_artists).cloned().collect();
    let mut meaningful = parsed.words.clone();
    for word in BOILERPLATE {
        if !expected.contains(*word) {
            meaningful.remove(*word);
        }
    }
    let precision = if identity_exact || inflected {
        1.0
    } else {
        coverage(&meaningful, &expected)
    };
    let artist = coverage(&all_artists, &evidence);
    let duration_points = if difference <= 3.0 {
        1.0 - difference / 3.0
    } else {
        0.0
    };
    let mut score = 0.5 * precision + 0.35 * artist + 0.15 * duration_points;
    if inflected && !identity_exact {
        score = (score - INFLECTION_PENALTY).max(0.0);
    }
    let verdict = Verdict {
        score,
        isrc,
        verified: candidate.verified,
        credited,
    };
    if isrc {
        return Some(verdict);
    }
    if primary.is_empty() {
        // An artist whose name has no letters (`¥$`) can only be identified by
        // its exact spelling in a credit, and only with an exact title.
        if !identity_exact || !symbols_only_artist(primary_name, candidate) {
            return None;
        }
    } else {
        let named = coverage(&primary, &evidence) >= 1.0
            || coverage(
                &set(secondary_tokens(primary_name)),
                &secondary_evidence(candidate, &candidate.title),
            ) >= 1.0
            || artist_account(primary_name, candidate);
        if !named {
            return None;
        }
    }
    if parsed.versions != wanted.versions || !parsed.soft.is_subset(&wanted.soft) {
        return None;
    }
    if !identity_exact && !inflected && coverage(&wanted.words, &parsed.words) < 0.9 {
        return None;
    }
    if precision < 0.75 {
        return None;
    }
    Some(verdict)
}

#[cfg(test)]
pub(crate) fn score(track: &Track, candidate: &Candidate) -> Option<f64> {
    evaluate(track, candidate).map(|verdict| verdict.score)
}

#[cfg(test)]
pub(crate) fn best_match_index(track: &Track, candidates: &[Candidate]) -> Option<usize> {
    let mut best: Option<(usize, Verdict)> = None;
    for (index, candidate) in candidates.iter().enumerate() {
        let Some(found) = evaluate(track, candidate) else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|(_, current)| better(found, *current))
        {
            best = Some((index, found));
        }
    }
    best.map(|(index, _)| index)
}

#[cfg(test)]
pub(crate) fn best_match<'a>(track: &Track, candidates: &'a [Candidate]) -> Option<&'a Candidate> {
    best_match_index(track, candidates).map(|index| &candidates[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Track {
        Track {
            uri: "spotify:track:abc".into(),
            name: "Northern Lights".into(),
            duration_ms: 200_000,
            artists: vec![ArtistRef {
                name: "Aurora".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }
    fn candidate(title: &str, uploader: &str, duration: Option<f64>) -> Candidate {
        Candidate {
            title: title.into(),
            uploader: uploader.into(),
            credits: vec![uploader.into()],
            duration,
            url: "https://www.youtube.com/watch?v=abcdefghijk".into(),
            ..Default::default()
        }
    }

    fn song(name: &str, artist: &str, duration_ms: u32) -> Track {
        Track {
            uri: "spotify:track:abc".into(),
            name: name.into(),
            duration_ms,
            artists: vec![ArtistRef {
                name: artist.into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn upload(title: &str, uploader: &str, credits: &[&str], duration: f64) -> Candidate {
        Candidate {
            title: title.into(),
            uploader: uploader.into(),
            credits: credits.iter().map(|credit| (*credit).to_owned()).collect(),
            duration: Some(duration),
            url: "https://soundcloud.com/someone/song".into(),
            ..Default::default()
        }
    }

    /// The owner's 2626 playlist, 2026-11-11: this instrumental was saved as
    /// slayr's song, and slayr's own upload was turned away.
    #[test]
    fn an_instrumental_is_never_the_song_and_the_artists_own_upload_is() {
        let wanted = song("Never Go Down", "slayr", 120_000);
        let instrumental = upload(
            "slayr - Never Go Down (Instrumental)",
            "slayr",
            &["NOVOX", "slayr"],
            120.328,
        );
        assert_eq!(score(&wanted, &instrumental), None);
        let own = upload("Never Going Down", "slayr", &["slayr"], 120.238);
        assert!(score(&wanted, &own).is_some());
        let remix = upload(
            "never go down remix [slayr] prod. wa",
            "scxba",
            &["scxba"],
            181.3,
        );
        assert_eq!(score(&wanted, &remix), None);
    }

    #[test]
    fn an_inflected_title_needs_the_artists_own_account_and_the_same_length() {
        let wanted = song("Never Go Down", "slayr", 120_000);
        let exact = upload("Never Go Down", "slayr", &["slayr"], 120.2);
        let inflected = upload("Never Going Down", "slayr", &["slayr"], 120.2);
        assert!(score(&wanted, &exact).unwrap() > score(&wanted, &inflected).unwrap());
        for (title, uploader, credits, length) in [
            ("Never Going Down", "fan", &["fan", "slayr"][..], 120.2),
            ("Never Going Down", "slayr", &["slayr"][..], 122.0),
            ("Never Going Down Now", "slayr", &["slayr"][..], 120.2),
            ("Never Go", "slayr", &["slayr"][..], 120.2),
        ] {
            assert_eq!(
                score(&wanted, &upload(title, uploader, credits, length)),
                None,
                "{title} by {uploader}, {length} s"
            );
        }
    }

    #[test]
    fn an_account_spelling_the_artist_without_spaces_is_the_artist() {
        let wanted = song("DIFFERENT DAY", "Playboi Carti", 166_700);
        let own = upload("DIFFERENT DAY", "playboicarti", &["playboicarti"], 166.726);
        assert!(score(&wanted, &own).is_some());
        for (title, uploader, length) in [
            (
                "PLAYBOI CARTI - UR THE MOON // DIFFERENT DAY",
                "carti meatrider",
                226.7,
            ),
            ("DIFFERENT DAY", "playboicartifan", 166.7),
            (
                "Playboi Carti Different day (Remix) ID",
                "Brady (US)",
                149.2,
            ),
            (
                "Playboi Carti - Different Day [PITCH,SPED UP]",
                "sahara2mil",
                154.8,
            ),
            ("different day (remix) - playboi carti", "yevixx", 96.6),
        ] {
            assert_eq!(
                score(&wanted, &upload(title, uploader, &[uploader], length)),
                None,
                "{title} by {uploader}"
            );
        }
    }

    #[test]
    fn the_album_route_takes_the_official_upload_by_its_length() {
        let wanted = song("flash", "2hollis", 165_000);
        assert_eq!(
            score(&wanted, &upload("beginning", "2hollis", &["2hollis"], 89.0)),
            None
        );
        assert!(score(&wanted, &upload("flash", "2hollis", &["2hollis"], 165.0)).is_some());
    }

    #[test]
    fn an_auto_generated_upload_names_its_title_and_artists() {
        let description = "Provided to YouTube by Create Music Group\n\ntell you (L2) · jacari\n\ntell you (L2)\n\n℗ jacari/watercolor\n\nAuto-generated by YouTube.";
        assert_eq!(
            topic_credits(description),
            Some(("tell you (L2)".to_owned(), vec!["jacari".to_owned()]))
        );
        assert_eq!(
            topic_credits("Provided to YouTube by X\n\nSong · A · B\n"),
            Some(("Song".to_owned(), vec!["A".to_owned(), "B".to_owned()]))
        );
        assert_eq!(topic_credits("A video about tell you · jacari"), None);
        assert_eq!(
            topic_credits("Provided to YouTube by X\n\nJust a title"),
            None
        );
        assert_eq!(topic_credits(""), None);
    }

    #[test]
    fn prefers_metadata_and_duration_and_accepts_topic_uploads() {
        let candidates = [
            candidate(
                "Aurora - Northern Lights (Official Audio)",
                "Aurora",
                Some(202.9),
            ),
            candidate("Northern Lights", "Aurora - Topic", Some(200.0)),
            candidate("Northern Lights", "Someone else", Some(200.0)),
        ];
        assert_eq!(
            best_match(&track(), &candidates).unwrap().uploader,
            "Aurora - Topic"
        );
    }

    #[test]
    fn refuses_missing_duration_wrong_artist_titles_and_alternate_recordings() {
        for candidate in [
            candidate("Northern Lights", "Aurora", None),
            candidate("Northern Lights", "Aurora", Some(203.1)),
            candidate("Northern Lights", "Unknown", Some(200.0)),
            candidate("Southern Lights", "Aurora", Some(200.0)),
            candidate("Aurora - Northern Lights (Live)", "Aurora", Some(200.0)),
            candidate("Aurora - Northern Lights (Remix)", "Aurora", Some(200.0)),
            candidate("Northern Lights (Cover)", "Aurora", Some(200.0)),
            candidate("Northern Lights (Sped Up)", "Aurora", Some(200.0)),
            candidate(
                "Northern Lights Full Album Compilation",
                "Aurora",
                Some(200.0),
            ),
        ] {
            assert!(score(&track(), &candidate).is_none(), "{candidate:?}");
        }
        assert!(best_match(&track(), &[]).is_none());
    }

    #[test]
    fn preserves_explicit_versions_and_unicode_names() {
        let mut track = track();
        track.name = "Northern Lights (Acoustic)".into();
        assert!(score(&track, &candidate("Northern Lights", "Aurora", Some(200.0))).is_none());
        assert!(score(&track, &candidate(&track.name, "Aurora", Some(200.0))).is_some());
        track.name = "Jóga".into();
        track.artists[0].name = "Björk".into();
        assert!(score(&track, &candidate("BJÖRK - JÓGA", "Bjork", Some(197.0))).is_some());
    }

    #[test]
    fn a_credit_or_an_accent_never_changes_which_song_it_is() {
        let mut track = track();
        track.name = "Sky".into();
        track.artists[0].name = "Playboi Carti".into();
        track.duration_ms = 195_900;
        for title in [
            "Sky (Prod. By Art Dealer)",
            "Sky (prod. by @artdealer)",
            "Playboi Carti - Sky prod by Art Dealer",
            "Sky (Explicit) [Official Audio]",
            "Sky [320 kbps]",
        ] {
            assert!(
                score(&track, &candidate(title, "Playboi Carti", Some(195.9))).is_some(),
                "{title}"
            );
        }
        for title in ["Sky (Live)", "Sky (Remix)", "Sky (Acapella)", "Sky *OG*"] {
            assert!(
                score(&track, &candidate(title, "Playboi Carti", Some(195.9))).is_none(),
                "{title}"
            );
        }
    }

    #[test]
    fn download_urls_are_canonical_youtube_only() {
        assert_eq!(
            youtube_url("https://youtu.be/abcdefghijk?t=4").unwrap(),
            "https://www.youtube.com/watch?v=abcdefghijk"
        );
        for url in [
            "http://youtube.com/watch?v=abcdefghijk",
            "https://youtube.com.evil/watch?v=abcdefghijk",
            "https://youtube.com/results?q=x",
            "https://youtube.com/watch?v=bad",
            "ytsearch5:test",
        ] {
            assert!(youtube_url(url).is_none());
        }
    }

    #[test]
    fn one_search_picks_from_youtube_and_soundcloud_listings() {
        use crate::api::models::ExternalUrls;
        let listing = |uri: &str, name: &str, channel: &str, duration_ms: u32| Track {
            uri: uri.into(),
            name: name.into(),
            artists: vec![ArtistRef {
                name: channel.into(),
                ..Default::default()
            }],
            duration_ms,
            ..Default::default()
        };
        let mut soundcloud = listing("soundcloud:track:12", "Northern Lights", "Aurora", 200_000);
        soundcloud.external_urls = ExternalUrls {
            spotify: Some("https://soundcloud.com/aurora/northern-lights".into()),
        };
        // A cover on YouTube loses to the real recording on SoundCloud, and
        // the pick carries the permalink the download helper needs.
        let found = pick(
            &track(),
            &[listing(
                "youtube:video:abcdefghijk",
                "Northern Lights (Cover)",
                "Aurora",
                200_000,
            )],
            &[soundcloud.clone()],
        )
        .unwrap();
        assert_eq!(found.uri, "soundcloud:track:12");
        assert_eq!(
            found.source,
            "https://soundcloud.com/aurora/northern-lights"
        );
        // A private or unresolved SoundCloud entry has no download source,
        // so the canonical YouTube listing wins instead.
        soundcloud.external_urls.spotify = None;
        let found = pick(
            &track(),
            &[listing(
                "youtube:video:abcdefghijk",
                "Aurora - Northern Lights",
                "Aurora",
                200_000,
            )],
            &[soundcloud],
        )
        .unwrap();
        assert_eq!(found.uri, "youtube:video:abcdefghijk");
        assert_eq!(found.source, "https://www.youtube.com/watch?v=abcdefghijk");
        // No qualifying listing means no match at all.
        assert!(pick(&track(), &[], &[]).is_none());
    }

    #[derive(Deserialize)]
    struct Fixtures {
        cases: Vec<FixtureCase>,
    }

    #[derive(Deserialize)]
    struct FixtureCase {
        id: String,
        track: Track,
        candidates: Vec<Candidate>,
        expect: Expectation,
    }

    #[derive(Deserialize)]
    struct Expectation {
        result: String,
        #[serde(default)]
        index: Option<usize>,
    }

    /// The hand-written pairs both implementations are measured against. A
    /// wrong song must never be accepted; the table also pins every credit,
    /// spelling and version case that used to be a false negative.
    #[test]
    fn published_matching_cases_hold() {
        let fixtures: Fixtures =
            serde_json::from_str(include_str!("testdata/matching-cases.json")).unwrap();
        assert!(fixtures.cases.len() >= 90);
        for case in fixtures.cases {
            let found = best_match_index(&case.track, &case.candidates);
            match case.expect.result.as_str() {
                "match" => {
                    let index = found.unwrap_or_else(|| panic!("{}: expected a match", case.id));
                    assert!(
                        score(&case.track, &case.candidates[index]).is_some(),
                        "{}: chosen candidate must be the scored one",
                        case.id
                    );
                    if let Some(expected) = case.expect.index {
                        assert_eq!(index, expected, "{}: chosen candidate", case.id);
                    }
                }
                "none" => assert!(
                    found.is_none(),
                    "{}: expected no match, got candidate {}",
                    case.id,
                    found.unwrap_or_default()
                ),
                other => panic!("{}: unknown expectation {other}", case.id),
            }
        }
    }

    /// The phone's Spotify songs are found this way on real YouTube:
    /// The whole search against the real services, for a list of Spotify
    /// songs saved as JSON (`[{uri, name, duration_ms, artists: [{name}],
    /// album: {name}}]`). Prints what each would play, or that none was found.
    ///
    /// `SIPHON_MATCH_SONGS=songs.json cargo test --lib every_song_finds_a_copy_for_real -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the helper, the network and a song list"]
    async fn every_song_finds_a_copy_for_real() {
        let Some(path) = std::env::var_os("SIPHON_MATCH_SONGS") else {
            return;
        };
        let songs: Vec<Track> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let searcher = Searcher::new(
            crate::youtube::Catalogue {
                resolver: crate::youtube::Resolver::default(),
                cache: std::env::temp_dir(),
                links: std::sync::Arc::new(crate::youtube::Links::default()),
            },
            Some(crate::soundcloud::api::Client::new(
                reqwest::Client::new(),
                None,
                None,
            )),
        );
        let mut missing = 0;
        for wanted in &songs {
            let found = searcher.same_song(wanted).await;
            if found.is_none() {
                missing += 1;
            }
            println!(
                "{} | {} | {} s => {}",
                wanted.name,
                wanted.artist_names(),
                wanted.duration_ms / 1000,
                found.map_or_else(|| "NO MATCH".to_owned(), |found| found.source)
            );
        }
        println!("{} of {} found", songs.len() - missing, songs.len());
    }

    /// `cargo test --lib same_song_is_found_on_youtube_for_real -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the helper and the network"]
    async fn same_song_is_found_on_youtube_for_real() {
        let resolver = crate::youtube::Resolver::default();
        for (artist, name, duration_ms) in [
            ("The Weeknd", "Blinding Lights", 200_040),
            ("Queen", "Bohemian Rhapsody", 354_320),
        ] {
            let wanted = Track {
                uri: "spotify:track:x".into(),
                name: name.into(),
                artists: vec![ArtistRef {
                    name: artist.into(),
                    ..Default::default()
                }],
                duration_ms,
                ..Default::default()
            };
            let query = format!("{artist} - {name}");
            let videos = crate::youtube::browse::videos_for(&resolver, &query)
                .await
                .unwrap();
            let best = videos
                .iter()
                .filter_map(|video| {
                    let found = Candidate {
                        url: video.uri.clone(),
                        title: video.name.clone(),
                        uploader: video
                            .artists
                            .first()
                            .map(|artist| artist.name.clone())
                            .unwrap_or_default(),
                        credits: video
                            .artists
                            .iter()
                            .map(|artist| artist.name.clone())
                            .collect(),
                        duration: Some(f64::from(video.duration_ms) / 1000.0),
                        ..Default::default()
                    };
                    score(&wanted, &found).map(|score| (video, score))
                })
                .max_by(|left, right| left.1.total_cmp(&right.1));
            println!(
                "{query}: {:?}",
                best.map(|(video, score)| (&video.name, &video.uri, score))
            );
            assert!(best.is_some(), "{query} is on YouTube");
        }
    }
}
