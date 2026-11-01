//! What this listener has heard, kept as plays on this computer, and the
//! statistics made from them.
//!
//! Each play is one line in a file per month under `listening/`, only ever
//! appended to, so a cut-off write costs at most its own line. Reports read
//! every month off the UI thread. A play made here knows exactly how long it
//! was heard; one from another device, which Spotify lists without that, counts
//! as the song's length and is marked `estimated`. Spotify's own Extended
//! Streaming History can be imported and replaces those estimates.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// Two records of a song this close together are the same play.
const SAME_PLAY_SECONDS: i64 = 120;
/// Spotify counts a stream from 30 seconds; an imported play shorter than that
/// was a skip.
const IMPORT_FROM_MS: u64 = 30_000;
/// How many rows each list of a report holds.
const TOP: usize = 50;
/// How many rows of each list the group sees.
const SHARED_TOP: usize = 10;
/// The most a single file inside an import may unpack to.
const LARGEST_IMPORT_FILE: u64 = 512 * 1024 * 1024;
/// Genres asked of Spotify per report, one request each.
const LOOKUPS: usize = 40;

/// The files of this log are written by whoever holds this, so that an import
/// and a play ending at the same moment do not interleave.
static WRITING: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayArtist {
    #[serde(rename = "i", default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "n")]
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Play {
    /// When it ended, in seconds since the epoch.
    #[serde(rename = "t")]
    pub at: i64,
    #[serde(rename = "m")]
    pub ms: u32,
    #[serde(rename = "u", default, skip_serializing_if = "String::is_empty")]
    pub uri: String,
    #[serde(rename = "n")]
    pub title: String,
    #[serde(rename = "a", default, skip_serializing_if = "Vec::is_empty")]
    pub artists: Vec<PlayArtist>,
    #[serde(rename = "l", default, skip_serializing_if = "String::is_empty")]
    pub album: String,
    #[serde(rename = "c", default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    /// The upload's own genre, for songs whose artists have none to look up.
    #[serde(rename = "g", default, skip_serializing_if = "Option::is_none")]
    pub genre: Option<String>,
    #[serde(rename = "e", default, skip_serializing_if = "std::ops::Not::not")]
    pub estimated: bool,
}

impl Play {
    /// Which song this is, across records with and without a URI.
    fn song_key(&self) -> String {
        if self.uri.is_empty() {
            let artist = self.artists.first().map_or("", |a| a.name.as_str());
            format!(
                "{}\u{1f}{}",
                self.title.to_lowercase(),
                artist.to_lowercase()
            )
        } else {
            self.uri.clone()
        }
    }
}

/// Which part of the Group page is on show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Feed,
    Stats,
}

/// What the statistics page asks of the app.
#[derive(Clone, Copy, Debug)]
pub enum Action {
    Tab(Tab),
    Period(Period),
    Import,
}

/// What the backend answers. An empty import error means none was chosen.
#[derive(Debug)]
pub enum Update {
    Report(Period, Box<Report>),
    Imported(Result<u64, String>),
}

/// What a report covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Period {
    #[default]
    ThisMonth,
    FourWeeks,
    SixMonths,
    ThisYear,
    AllTime,
}

impl Period {
    pub const ALL: [Self; 5] = [
        Self::ThisMonth,
        Self::FourWeeks,
        Self::SixMonths,
        Self::ThisYear,
        Self::AllTime,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ThisMonth => "This month",
            Self::FourWeeks => "4 weeks",
            Self::SixMonths => "6 months",
            Self::ThisYear => "This year",
            Self::AllTime => "All time",
        }
    }

    /// What the group's service calls it, for the two periods it holds.
    pub fn shared_key(self) -> Option<&'static str> {
        match self {
            Self::ThisMonth => Some("month"),
            Self::AllTime => Some("all"),
            _ => None,
        }
    }

    fn since(self, now: &jiff::Zoned) -> Option<i64> {
        use jiff::ToSpan;
        let date = now.date();
        let start = match self {
            Self::ThisMonth => date.first_of_month(),
            Self::FourWeeks => date.checked_sub(28.days()).ok()?,
            Self::SixMonths => date.checked_sub(6.months()).ok()?,
            Self::ThisYear => date.first_of_year(),
            Self::AllTime => return None,
        };
        start
            .to_zoned(now.time_zone().clone())
            .ok()
            .map(|at| at.timestamp().as_second())
    }
}

/// One row of a top list.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Top {
    pub name: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub art: Option<String>,
    /// What plays it, for a song.
    #[serde(default)]
    pub uri: String,
    pub plays: u64,
    pub minutes: u64,
}

/// What the group sees of a member, for one period.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Summary {
    /// The local month this counts, for this month's summary.
    #[serde(default)]
    pub month: String,
    pub minutes: u64,
    pub plays: u64,
    #[serde(default)]
    pub songs: Vec<Top>,
    #[serde(default)]
    pub artists: Vec<Top>,
    #[serde(default)]
    pub genres: Vec<Top>,
    #[serde(default)]
    pub hours: Vec<u64>,
}

/// Something to ask Spotify so that an artist's genres can be counted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    Artist(String),
    /// An artist known only by name, as imported plays are: their song says who
    /// they are.
    Song(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub month: String,
    pub minutes: u64,
    pub plays: u64,
    pub distinct_songs: u64,
    pub distinct_artists: u64,
    /// Artists first heard in this period.
    pub new_artists: u64,
    /// Days in a row up to today or yesterday.
    pub streak_days: u64,
    pub longest_streak_days: u64,
    pub songs: Vec<Top>,
    pub artists: Vec<Top>,
    pub albums: Vec<Top>,
    pub genres: Vec<Top>,
    /// Of the minutes heard, how many have known genres, in percent.
    pub genres_known_percent: u8,
    /// Minutes by local hour of the day.
    pub hours: Vec<u64>,
    /// Minutes by day of the week, Monday first.
    pub weekdays: Vec<u64>,
    /// Minutes by local month, oldest first, as (`YYYY-MM`, minutes).
    pub months: Vec<(String, u64)>,
    /// Of the minutes heard, how many Spotify only reported, so are estimated.
    pub estimated_percent: u8,
    pub first_play: Option<i64>,
    pub lookups: Vec<Lookup>,
}

impl Report {
    pub fn summary(&self) -> Summary {
        let shared = |list: &[Top]| list.iter().take(SHARED_TOP).cloned().collect();
        Summary {
            month: self.month.clone(),
            minutes: self.minutes,
            plays: self.plays,
            songs: shared(&self.songs),
            artists: shared(&self.artists),
            genres: self.genres.iter().take(5).cloned().collect(),
            hours: self.hours.clone(),
        }
    }
}

/// How much two summaries share, 0 to 100: of the minutes each spent on their
/// top artists, how much went to artists they have in common.
pub fn taste_match(a: &Summary, b: &Summary) -> u8 {
    let weights = |summary: &Summary| -> HashMap<String, f64> {
        let total: u64 = summary.artists.iter().map(|t| t.minutes).sum();
        summary
            .artists
            .iter()
            .map(|t| {
                (
                    t.name.to_lowercase(),
                    t.minutes as f64 / total.max(1) as f64,
                )
            })
            .collect()
    };
    let (a, b) = (weights(a), weights(b));
    let shared: f64 = a
        .iter()
        .filter_map(|(name, weight)| b.get(name).map(|other| weight.min(*other)))
        .sum();
    (shared * 100.0).round().clamp(0.0, 100.0) as u8
}

/// Artists two summaries have in common, biggest first by the first's minutes.
pub fn shared_artists(a: &Summary, b: &Summary) -> Vec<String> {
    let theirs: HashSet<String> = b.artists.iter().map(|t| t.name.to_lowercase()).collect();
    a.artists
        .iter()
        .filter(|t| theirs.contains(&t.name.to_lowercase()))
        .map(|t| t.name.clone())
        .collect()
}

/// Which genres artists have, as Spotify lists them, kept between runs.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Genres {
    #[serde(default)]
    pub artists: HashMap<String, Vec<String>>,
    /// Lowercase names to ids, for plays that only name their artists.
    #[serde(default)]
    pub names: HashMap<String, String>,
}

impl Genres {
    fn of_artist(&self, artist: &PlayArtist) -> Option<&Vec<String>> {
        let id = artist
            .id
            .as_ref()
            .or_else(|| self.names.get(&artist.name.to_lowercase()))?;
        self.artists.get(id)
    }
}

fn month_of(at: i64) -> String {
    let zoned = jiff::Timestamp::from_second(at)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(jiff::tz::TimeZone::UTC);
    format!("{:04}-{:02}", zoned.year(), zoned.month())
}

fn month_file(dir: &Path, month: &str) -> PathBuf {
    dir.join(format!("{month}.jsonl"))
}

/// Reads one month, skipping any line a cut-off write left unreadable.
fn read_month(path: &Path) -> Vec<Play> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    std::io::BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect()
}

/// Every play kept, in no particular order.
pub fn read_all(dir: &Path) -> Vec<Play> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .flat_map(|path| read_month(&path))
        .collect()
}

fn open_for_append(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    // What someone listened to is theirs: readable by its owner only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Adds plays to the log. Those it may already hold, from Spotify's list or an
/// earlier import, are checked against it first. Answers how many were new.
pub fn record(dir: &Path, plays: Vec<Play>, check: bool) -> std::io::Result<u64> {
    let _writing = WRITING.lock().unwrap_or_else(|poison| poison.into_inner());
    std::fs::create_dir_all(dir)?;
    let mut months: BTreeMap<String, Vec<Play>> = BTreeMap::new();
    for play in plays {
        months.entry(month_of(play.at)).or_default().push(play);
    }
    let mut added = 0;
    for (month, plays) in months {
        let path = month_file(dir, &month);
        let mut held: HashMap<String, Vec<i64>> = HashMap::new();
        if check {
            // A play near a month's edge may sit in the one beside it, but then
            // it is only counted twice until a report sees both and merges them.
            for play in read_month(&path) {
                held.entry(play.song_key()).or_default().push(play.at);
            }
        }
        let mut text = String::new();
        for play in plays {
            let times = held.entry(play.song_key()).or_default();
            if check
                && times
                    .iter()
                    .any(|at| at.abs_diff(play.at) <= SAME_PLAY_SECONDS as u64)
            {
                continue;
            }
            times.push(play.at);
            if let Ok(line) = serde_json::to_string(&play) {
                text.push_str(&line);
                text.push('\n');
                added += 1;
            }
        }
        if !text.is_empty() {
            let mut file = open_for_append(&path)?;
            file.write_all(text.as_bytes())?;
            file.flush()?;
        }
    }
    Ok(added)
}

/// Forgets everything kept, when the listener clears their history.
pub fn clear(dir: &Path) -> std::io::Result<()> {
    let _writing = WRITING.lock().unwrap_or_else(|poison| poison.into_inner());
    match std::fs::remove_dir_all(dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

pub fn read_genres(dir: &Path) -> Genres {
    std::fs::read_to_string(dir.join("genres.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Merges what Spotify said, replacing the file whole.
pub fn add_genres(
    dir: &Path,
    artists: Vec<(String, Vec<String>)>,
    names: Vec<(String, String)>,
) -> std::io::Result<()> {
    let _writing = WRITING.lock().unwrap_or_else(|poison| poison.into_inner());
    std::fs::create_dir_all(dir)?;
    let mut genres = read_genres(dir);
    genres.artists.extend(artists);
    genres.names.extend(
        names
            .into_iter()
            .map(|(name, id)| (name.to_lowercase(), id)),
    );
    let text = serde_json::to_vec(&genres).map_err(std::io::Error::other)?;
    fastframe_file::write(&dir.join("genres.json"), text).map_err(std::io::Error::other)
}

/// One after another, a play and its near-copies become the best one: a real
/// measurement over an estimate, then whichever knows more about the song.
fn merged(mut plays: Vec<Play>) -> Vec<Play> {
    plays.sort_by_key(|play| play.at);
    let mut last_of_song: HashMap<String, usize> = HashMap::new();
    let mut kept: Vec<Play> = Vec::with_capacity(plays.len());
    for play in plays {
        let key = play.song_key();
        if let Some(&index) = last_of_song.get(&key) {
            let earlier = &mut kept[index];
            if play.at - earlier.at <= SAME_PLAY_SECONDS {
                if earlier.estimated && !play.estimated {
                    earlier.ms = play.ms;
                    earlier.estimated = false;
                }
                if earlier.art.is_none() {
                    earlier.art = play.art;
                }
                if earlier.uri.is_empty() {
                    earlier.uri = play.uri;
                }
                if earlier.artists.iter().all(|a| a.id.is_none()) && !play.artists.is_empty() {
                    earlier.artists = play.artists;
                }
                continue;
            }
        }
        last_of_song.insert(key, kept.len());
        kept.push(play);
    }
    kept
}

#[derive(Default)]
struct Tally {
    name: String,
    detail: String,
    art: Option<String>,
    uri: String,
    plays: u64,
    ms: u64,
}

impl Tally {
    fn add(&mut self, play: &Play) {
        self.plays += 1;
        self.ms += u64::from(play.ms);
        if self.art.is_none() {
            self.art.clone_from(&play.art);
        }
    }
}

fn ranked(tallies: HashMap<String, Tally>) -> Vec<Top> {
    let mut rows: Vec<Tally> = tallies.into_values().collect();
    rows.sort_by(|a, b| {
        (b.plays, b.ms)
            .cmp(&(a.plays, a.ms))
            .then_with(|| a.name.cmp(&b.name))
    });
    rows.truncate(TOP);
    rows.into_iter()
        .map(|row| Top {
            name: row.name,
            detail: row.detail,
            art: row.art,
            uri: row.uri,
            plays: row.plays,
            minutes: row.ms / 60_000,
        })
        .collect()
}

fn local(at: i64, zone: &jiff::tz::TimeZone) -> jiff::Zoned {
    jiff::Timestamp::from_second(at)
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .to_zoned(zone.clone())
}

/// Longest run of days and the run reaching today or yesterday.
fn streaks(days: &HashSet<jiff::civil::Date>, today: jiff::civil::Date) -> (u64, u64) {
    let mut sorted: Vec<_> = days.iter().copied().collect();
    sorted.sort();
    let (mut longest, mut run) = (0u64, 0u64);
    let mut previous: Option<jiff::civil::Date> = None;
    for day in &sorted {
        run = match previous.and_then(|p| p.tomorrow().ok()) {
            Some(next) if next == *day => run + 1,
            _ => 1,
        };
        longest = longest.max(run);
        previous = Some(*day);
    }
    let mut current = 0;
    let mut day = if days.contains(&today) {
        Some(today)
    } else {
        today.yesterday().ok().filter(|y| days.contains(y))
    };
    while let Some(on) = day {
        if !days.contains(&on) {
            break;
        }
        current += 1;
        day = on.yesterday().ok();
    }
    (current, longest)
}

/// Works a report out from every play kept.
pub fn report(plays: Vec<Play>, period: Period, genres: &Genres, now: &jiff::Zoned) -> Report {
    let plays = merged(plays);
    let zone = now.time_zone().clone();
    let since = period.since(now);
    let mut report = Report {
        month: format!("{:04}-{:02}", now.year(), now.month()),
        hours: vec![0; 24],
        weekdays: vec![0; 7],
        first_play: plays.first().map(|play| play.at),
        ..Report::default()
    };
    let mut songs: HashMap<String, Tally> = HashMap::new();
    let mut artists: HashMap<String, Tally> = HashMap::new();
    let mut albums: HashMap<String, Tally> = HashMap::new();
    let mut genre_ms: HashMap<String, Tally> = HashMap::new();
    let mut months: BTreeMap<String, u64> = BTreeMap::new();
    let mut days = HashSet::new();
    let mut heard_before: HashSet<String> = HashSet::new();
    let mut new_artists: HashSet<String> = HashSet::new();
    let (mut total_ms, mut estimated_ms, mut genre_known_ms) = (0u64, 0u64, 0u64);
    let mut missing: HashMap<String, (u64, Lookup)> = HashMap::new();
    let mut hours_ms = [0u64; 24];
    let mut weekdays_ms = [0u64; 7];

    for play in &plays {
        let at = local(play.at, &zone);
        days.insert(at.date());
        let inside = since.is_none_or(|since| play.at >= since);
        if !inside {
            for artist in &play.artists {
                heard_before.insert(artist.name.to_lowercase());
            }
            continue;
        }
        total_ms += u64::from(play.ms);
        if play.estimated {
            estimated_ms += u64::from(play.ms);
        }
        report.plays += 1;
        hours_ms[at.hour() as usize] += u64::from(play.ms);
        weekdays_ms[at.weekday().to_monday_zero_offset() as usize] += u64::from(play.ms);
        *months
            .entry(format!("{:04}-{:02}", at.year(), at.month()))
            .or_default() += u64::from(play.ms);

        let names = play
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let song = songs.entry(play.song_key()).or_insert_with(|| Tally {
            name: play.title.clone(),
            detail: names.clone(),
            uri: play.uri.clone(),
            ..Tally::default()
        });
        if song.uri.is_empty() {
            song.uri.clone_from(&play.uri);
        }
        song.add(play);
        if !play.album.is_empty() {
            let first = play.artists.first().map_or("", |a| a.name.as_str());
            let key = format!(
                "{}\u{1f}{}",
                play.album.to_lowercase(),
                first.to_lowercase()
            );
            albums
                .entry(key)
                .or_insert_with(|| Tally {
                    name: play.album.clone(),
                    detail: first.to_owned(),
                    ..Tally::default()
                })
                .add(play);
        }

        let mut play_genres: Vec<String> = Vec::new();
        for artist in &play.artists {
            let key = artist.name.to_lowercase();
            if !heard_before.contains(&key) {
                new_artists.insert(key.clone());
            }
            artists
                .entry(key.clone())
                .or_insert_with(|| Tally {
                    name: artist.name.clone(),
                    ..Tally::default()
                })
                .add(play);
            match genres.of_artist(artist) {
                Some(list) => play_genres.extend(list.iter().cloned()),
                None => {
                    let lookup = match (&artist.id, play.uri.strip_prefix("spotify:track:")) {
                        (Some(id), _) => Some((id.clone(), Lookup::Artist(id.clone()))),
                        (None, Some(song)) => Some((key, Lookup::Song(song.to_owned()))),
                        (None, None) => None,
                    };
                    if let Some((key, lookup)) = lookup {
                        missing.entry(key).or_insert((0, lookup)).0 += u64::from(play.ms);
                    }
                }
            }
        }
        if let Some(genre) = &play.genre {
            play_genres.push(genre.clone());
        }
        play_genres.sort();
        play_genres.dedup();
        if !play_genres.is_empty() {
            genre_known_ms += u64::from(play.ms);
            let share = u64::from(play.ms) / play_genres.len() as u64;
            for genre in play_genres {
                let row = genre_ms
                    .entry(genre.to_lowercase())
                    .or_insert_with(|| Tally {
                        name: genre.clone(),
                        ..Tally::default()
                    });
                row.plays += 1;
                row.ms += share;
            }
        }
    }

    report.minutes = total_ms / 60_000;
    report.distinct_songs = songs.len() as u64;
    report.distinct_artists = artists.len() as u64;
    report.new_artists = if since.is_some() {
        new_artists.len() as u64
    } else {
        0
    };
    let (current, longest) = streaks(&days, now.date());
    report.streak_days = current;
    report.longest_streak_days = longest;
    report.songs = ranked(songs);
    report.artists = ranked(artists);
    report.albums = ranked(albums);
    let mut genre_rows: Vec<Tally> = genre_ms.into_values().collect();
    genre_rows.sort_by(|a, b| b.ms.cmp(&a.ms).then_with(|| a.name.cmp(&b.name)));
    report.genres = genre_rows
        .into_iter()
        .take(TOP)
        .map(|row| Top {
            name: row.name,
            plays: row.plays,
            minutes: row.ms / 60_000,
            ..Top::default()
        })
        .collect();
    let percent = |part: u64| ((part * 100) / total_ms.max(1)).min(100) as u8;
    report.genres_known_percent = percent(genre_known_ms);
    report.estimated_percent = percent(estimated_ms);
    report.hours = hours_ms.iter().map(|ms| ms / 60_000).collect();
    report.weekdays = weekdays_ms.iter().map(|ms| ms / 60_000).collect();
    report.months = months
        .into_iter()
        .map(|(month, ms)| (month, ms / 60_000))
        .collect();
    let mut wanted: Vec<(u64, Lookup)> = missing.into_values().collect();
    wanted.sort_by_key(|row| std::cmp::Reverse(row.0));
    report.lookups = wanted
        .into_iter()
        .take(LOOKUPS)
        .map(|(_, lookup)| lookup)
        .collect();
    report
}

#[derive(Deserialize)]
struct ExtendedRow {
    ts: Option<String>,
    ms_played: Option<u64>,
    master_metadata_track_name: Option<String>,
    master_metadata_album_artist_name: Option<String>,
    master_metadata_album_album_name: Option<String>,
    spotify_track_uri: Option<String>,
    #[serde(default)]
    incognito_mode: Option<bool>,
    // The shorter "Account data" download.
    #[serde(rename = "endTime")]
    end_time: Option<String>,
    #[serde(rename = "msPlayed")]
    short_ms: Option<u64>,
    #[serde(rename = "trackName")]
    track_name: Option<String>,
    #[serde(rename = "artistName")]
    artist_name: Option<String>,
}

fn parse_time(row: &ExtendedRow) -> Option<i64> {
    if let Some(ts) = &row.ts {
        return ts
            .parse::<jiff::Timestamp>()
            .ok()
            .map(jiff::Timestamp::as_second);
    }
    // "2024-03-01 18:04", in UTC.
    let end = row.end_time.as_ref()?;
    format!("{}:00Z", end.replace(' ', "T"))
        .parse::<jiff::Timestamp>()
        .ok()
        .map(jiff::Timestamp::as_second)
}

/// The songs in one of Spotify's history files. Episodes, skips and what was
/// played in a private session stay out.
pub fn parse_history(bytes: &[u8]) -> Result<Vec<Play>, serde_json::Error> {
    let rows: Vec<ExtendedRow> = serde_json::from_slice(bytes)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            if row.incognito_mode == Some(true) {
                return None;
            }
            let ms = row.ms_played.or(row.short_ms)?;
            if ms < IMPORT_FROM_MS {
                return None;
            }
            let at = parse_time(&row)?;
            let title = row
                .master_metadata_track_name
                .as_ref()
                .or(row.track_name.as_ref())?
                .trim()
                .to_owned();
            if title.is_empty() {
                return None;
            }
            let artist = row
                .master_metadata_album_artist_name
                .as_ref()
                .or(row.artist_name.as_ref())
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty());
            Some(Play {
                at,
                ms: u32::try_from(ms).unwrap_or(u32::MAX),
                uri: row.spotify_track_uri.unwrap_or_default(),
                title,
                artists: artist
                    .map(|name| PlayArtist { id: None, name })
                    .into_iter()
                    .collect(),
                album: row.master_metadata_album_album_name.unwrap_or_default(),
                art: None,
                genre: None,
                estimated: false,
            })
        })
        .collect())
}

fn is_history_file(name: &str) -> bool {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.ends_with(".json")
        && (base.starts_with("Streaming_History_Audio")
            || base.starts_with("StreamingHistory_music"))
}

/// The plays in a file Spotify sent: its ZIP, or one history file out of it.
pub fn read_import(path: &Path) -> Result<Vec<Play>, String> {
    const UNREADABLE: &str = "That file could not be read.";
    let is_zip = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
    if !is_zip {
        let bytes = std::fs::read(path).map_err(|_| UNREADABLE.to_owned())?;
        return parse_history(&bytes).map_err(|_| "That isn't a Spotify history file.".to_owned());
    }
    let file = std::fs::File::open(path).map_err(|_| UNREADABLE.to_owned())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|_| UNREADABLE.to_owned())?;
    let mut plays = Vec::new();
    let mut found = false;
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        if !entry.name().is_ok_and(|name| is_history_file(&name)) {
            continue;
        }
        let mut bytes = Vec::new();
        if entry
            .take(LARGEST_IMPORT_FILE)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return Err(UNREADABLE.to_owned());
        }
        if let Ok(rows) = parse_history(&bytes) {
            found = true;
            plays.extend(rows);
        }
    }
    if found {
        Ok(plays)
    } else {
        Err("There is no streaming history in that file.".to_owned())
    }
}

/// A SoundCloud upload's own genre, kept as tracks pass by, since the shared
/// track type has nowhere to carry it.
static SOUNDCLOUD_GENRES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub fn note_soundcloud_genre(uri: &str, genre: &str) {
    let genre = genre.trim();
    if genre.is_empty() {
        return;
    }
    let mut genres = SOUNDCLOUD_GENRES
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    genres
        .get_or_insert_with(HashMap::new)
        .insert(uri.to_owned(), genre.to_owned());
}

pub fn soundcloud_genre(uri: &str) -> Option<String> {
    SOUNDCLOUD_GENRES
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .as_ref()?
        .get(uri)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> i64 {
        text.parse::<jiff::Timestamp>().unwrap().as_second()
    }

    fn now() -> jiff::Zoned {
        at_zoned("2026-11-15T12:00:00Z")
    }

    fn at_zoned(text: &str) -> jiff::Zoned {
        text.parse::<jiff::Timestamp>()
            .unwrap()
            .to_zoned(jiff::tz::TimeZone::UTC)
    }

    fn play(time: &str, title: &str, artist: &str, minutes: u32) -> Play {
        Play {
            at: at(time),
            ms: minutes * 60_000,
            uri: format!("spotify:track:{title}"),
            title: title.into(),
            artists: vec![PlayArtist {
                id: Some(format!("id-{artist}")),
                name: artist.into(),
            }],
            album: format!("{artist} album"),
            art: None,
            genre: None,
            estimated: false,
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("siphon-stats-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn minutes_songs_and_artists_are_counted_for_the_period() {
        let plays = vec![
            play("2026-11-10T10:00:00Z", "one", "Ana", 3),
            play("2026-11-10T11:00:00Z", "one", "Ana", 3),
            play("2026-11-11T11:00:00Z", "two", "Bo", 4),
            // Before this month: in all time only.
            play("2026-10-11T11:00:00Z", "old", "Cleo", 10),
        ];
        let month = report(plays.clone(), Period::ThisMonth, &Genres::default(), &now());
        assert_eq!(month.minutes, 10);
        assert_eq!(month.plays, 3);
        assert_eq!(month.distinct_songs, 2);
        assert_eq!(month.distinct_artists, 2);
        assert_eq!(month.songs[0].name, "one");
        assert_eq!((month.songs[0].plays, month.songs[0].minutes), (2, 6));
        assert_eq!(month.artists[0].name, "Ana");
        assert_eq!(month.albums[0].name, "Ana album");
        assert_eq!(month.months, vec![("2026-11".to_owned(), 10)]);

        let all = report(plays, Period::AllTime, &Genres::default(), &now());
        assert_eq!(all.minutes, 20);
        assert_eq!(
            all.months,
            vec![("2026-10".to_owned(), 10), ("2026-11".to_owned(), 10)]
        );
    }

    #[test]
    fn artists_first_heard_in_the_period_are_new() {
        let plays = vec![
            play("2026-10-11T11:00:00Z", "old", "Ana", 3),
            play("2026-11-11T11:00:00Z", "one", "Ana", 3),
            play("2026-11-11T12:00:00Z", "two", "Bo", 3),
        ];
        let month = report(plays, Period::ThisMonth, &Genres::default(), &now());
        assert_eq!(month.new_artists, 1);
    }

    #[test]
    fn genres_share_a_plays_time_and_say_how_much_is_known() {
        let mut genres = Genres::default();
        genres
            .artists
            .insert("id-Ana".into(), vec!["house".into(), "techno".into()]);
        let plays = vec![
            play("2026-11-10T10:00:00Z", "one", "Ana", 4),
            play("2026-11-10T11:00:00Z", "two", "Bo", 4),
        ];
        let month = report(plays, Period::ThisMonth, &genres, &now());
        assert_eq!(month.genres.len(), 2);
        assert_eq!(month.genres[0].minutes, 2);
        assert_eq!(month.genres_known_percent, 50);
        assert_eq!(month.lookups, vec![Lookup::Artist("id-Bo".into())]);
    }

    #[test]
    fn an_artist_known_by_name_is_looked_up_through_their_song_then_counted() {
        let mut named = play("2026-11-10T10:00:00Z", "one", "Ana", 4);
        named.artists[0].id = None;
        let month = report(
            vec![named.clone()],
            Period::ThisMonth,
            &Genres::default(),
            &now(),
        );
        assert_eq!(month.lookups, vec![Lookup::Song("one".into())]);

        let mut genres = Genres::default();
        genres.names.insert("ana".into(), "id-Ana".into());
        genres.artists.insert("id-Ana".into(), vec!["house".into()]);
        let month = report(vec![named], Period::ThisMonth, &genres, &now());
        assert_eq!(month.genres[0].name, "house");
        assert!(month.lookups.is_empty());
    }

    #[test]
    fn an_uploads_own_genre_counts() {
        let mut upload = play("2026-11-10T10:00:00Z", "one", "Ana", 4);
        upload.uri = "soundcloud:track:1".into();
        upload.artists[0].id = None;
        upload.genre = Some("Drum & Bass".into());
        let month = report(vec![upload], Period::ThisMonth, &Genres::default(), &now());
        assert_eq!(month.genres[0].name, "Drum & Bass");
        assert_eq!(month.genres_known_percent, 100);
    }

    #[test]
    fn a_measured_play_replaces_spotifys_estimate_of_it() {
        let mut estimate = play("2026-11-10T10:00:30Z", "one", "Ana", 5);
        estimate.estimated = true;
        let measured = play("2026-11-10T10:00:00Z", "one", "Ana", 2);
        let month = report(
            vec![estimate.clone(), measured],
            Period::ThisMonth,
            &Genres::default(),
            &now(),
        );
        assert_eq!(month.plays, 1);
        assert_eq!(month.minutes, 2);
        assert_eq!(month.estimated_percent, 0);

        let alone = report(
            vec![estimate],
            Period::ThisMonth,
            &Genres::default(),
            &now(),
        );
        assert_eq!(alone.estimated_percent, 100);
    }

    #[test]
    fn hearing_a_song_twice_in_a_row_is_two_plays() {
        let plays = vec![
            play("2026-11-10T10:00:00Z", "one", "Ana", 3),
            play("2026-11-10T10:03:00Z", "one", "Ana", 3),
        ];
        assert_eq!(
            report(plays, Period::ThisMonth, &Genres::default(), &now()).plays,
            2
        );
    }

    #[test]
    fn streaks_count_days_in_a_row() {
        let plays = vec![
            play("2026-11-01T10:00:00Z", "a", "Ana", 1),
            play("2026-11-02T10:00:00Z", "a", "Ana", 1),
            play("2026-11-03T10:00:00Z", "a", "Ana", 1),
            play("2026-11-13T10:00:00Z", "a", "Ana", 1),
            play("2026-11-14T10:00:00Z", "a", "Ana", 1),
        ];
        let month = report(plays, Period::ThisMonth, &Genres::default(), &now());
        // Nothing yet today: the run up to yesterday still stands.
        assert_eq!(month.streak_days, 2);
        assert_eq!(month.longest_streak_days, 3);
    }

    #[test]
    fn a_broken_streak_is_zero() {
        let plays = vec![play("2026-11-01T10:00:00Z", "a", "Ana", 1)];
        let month = report(plays, Period::ThisMonth, &Genres::default(), &now());
        assert_eq!(month.streak_days, 0);
        assert_eq!(month.longest_streak_days, 1);
    }

    #[test]
    fn hours_and_weekdays_use_local_time() {
        let zone = jiff::tz::TimeZone::fixed(jiff::tz::offset(2));
        let now = at("2026-11-15T12:00:00Z");
        let now = jiff::Timestamp::from_second(now).unwrap().to_zoned(zone);
        // 23:30 UTC on Sunday is 01:30 on Monday here.
        let plays = vec![play("2026-11-08T23:30:00Z", "a", "Ana", 60)];
        let month = report(plays, Period::ThisMonth, &Genres::default(), &now);
        assert_eq!(month.hours[1], 60);
        assert_eq!(month.weekdays[0], 60);
    }

    #[test]
    fn the_log_survives_a_cut_off_line_and_keeps_months_apart() {
        let dir = temp("log");
        record(
            &dir,
            vec![
                play("2026-11-10T10:00:00Z", "one", "Ana", 3),
                play("2026-10-10T10:00:00Z", "two", "Ana", 3),
            ],
            false,
        )
        .unwrap();
        assert!(dir.join("2026-11.jsonl").exists());
        assert!(dir.join("2026-10.jsonl").exists());
        let mut file = open_for_append(&dir.join("2026-11.jsonl")).unwrap();
        file.write_all(b"{\"t\":17").unwrap();
        assert_eq!(read_all(&dir).len(), 2);
        clear(&dir).unwrap();
        assert!(read_all(&dir).is_empty());
        clear(&dir).unwrap();
    }

    #[test]
    fn plays_already_held_are_not_added_again() {
        let dir = temp("again");
        let plays = vec![play("2026-11-10T10:00:00Z", "one", "Ana", 3)];
        assert_eq!(record(&dir, plays.clone(), true).unwrap(), 1);
        assert_eq!(record(&dir, plays, true).unwrap(), 0);
        assert_eq!(read_all(&dir).len(), 1);
        clear(&dir).unwrap();
    }

    #[test]
    fn genres_are_kept_between_runs() {
        let dir = temp("genres");
        add_genres(
            &dir,
            vec![("id".into(), vec!["house".into()])],
            vec![("ANA".into(), "id".into())],
        )
        .unwrap();
        let genres = read_genres(&dir);
        assert_eq!(genres.artists["id"], vec!["house".to_owned()]);
        assert_eq!(genres.names["ana"], "id");
        clear(&dir).unwrap();
    }

    #[test]
    fn extended_history_keeps_songs_only() {
        let text = serde_json::json!([
            {"ts": "2024-03-01T18:04:00Z", "ms_played": 200000, "master_metadata_track_name": "Song", "master_metadata_album_artist_name": "Artist", "master_metadata_album_album_name": "Album", "spotify_track_uri": "spotify:track:abc", "incognito_mode": false},
            {"ts": "2024-03-01T18:05:00Z", "ms_played": 2000, "master_metadata_track_name": "Skipped", "master_metadata_album_artist_name": "Artist", "spotify_track_uri": "spotify:track:d"},
            {"ts": "2024-03-01T18:06:00Z", "ms_played": 200000, "master_metadata_track_name": "Secret", "master_metadata_album_artist_name": "Artist", "spotify_track_uri": "spotify:track:e", "incognito_mode": true},
            {"ts": "2024-03-01T18:07:00Z", "ms_played": 200000, "master_metadata_track_name": null, "episode_name": "A podcast"},
        ]);
        let plays = parse_history(text.to_string().as_bytes()).unwrap();
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].title, "Song");
        assert_eq!(plays[0].uri, "spotify:track:abc");
        assert_eq!(plays[0].ms, 200_000);
        assert_eq!(plays[0].album, "Album");
        assert_eq!(plays[0].artists[0].name, "Artist");
        assert_eq!(plays[0].at, at("2024-03-01T18:04:00Z"));
    }

    #[test]
    fn the_shorter_account_download_is_understood_too() {
        let text = serde_json::json!([
            {"endTime": "2024-03-01 18:04", "artistName": "Artist", "trackName": "Song", "msPlayed": 200000},
        ]);
        let plays = parse_history(text.to_string().as_bytes()).unwrap();
        assert_eq!(plays[0].at, at("2024-03-01T18:04:00Z"));
        assert_eq!(plays[0].uri, "");
    }

    #[test]
    fn a_zip_from_spotify_is_read() {
        let dir = temp("zip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("my_spotify_data.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file(
                "Spotify Extended Streaming History/Streaming_History_Audio_2024_1.json",
                options,
            )
            .unwrap();
        writer
            .write_all(serde_json::json!([{"ts": "2024-03-01T18:04:00Z", "ms_played": 200000, "master_metadata_track_name": "Song", "master_metadata_album_artist_name": "Artist"}]).to_string().as_bytes())
            .unwrap();
        writer
            .start_file(
                "Spotify Extended Streaming History/Streaming_History_Video_2024.json",
                options,
            )
            .unwrap();
        writer.write_all(b"[]").unwrap();
        writer.start_file("ReadMeFirst.pdf", options).unwrap();
        writer.write_all(b"pdf").unwrap();
        writer.finish().unwrap();
        assert_eq!(read_import(&path).unwrap().len(), 1);
        clear(&dir).unwrap();
    }

    #[test]
    fn a_zip_without_history_says_so() {
        let dir = temp("zip-none");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("other.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        writer
            .start_file("photo.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"[]").unwrap();
        writer.finish().unwrap();
        assert!(read_import(&path).is_err());
        clear(&dir).unwrap();
    }

    #[test]
    fn taste_match_weighs_what_two_people_share() {
        let top = |name: &str, minutes| Top {
            name: name.into(),
            minutes,
            ..Top::default()
        };
        let a = Summary {
            artists: vec![top("Ana", 75), top("Bo", 25)],
            ..Summary::default()
        };
        let b = Summary {
            artists: vec![top("ana", 50), top("Cleo", 50)],
            ..Summary::default()
        };
        assert_eq!(taste_match(&a, &b), 50);
        assert_eq!(taste_match(&a, &a), 100);
        assert_eq!(taste_match(&a, &Summary::default()), 0);
        assert_eq!(shared_artists(&a, &b), vec!["Ana".to_owned()]);
    }

    #[test]
    fn the_group_sees_only_the_top_of_each_list() {
        let top = |i: usize| Top {
            name: format!("{i}"),
            ..Top::default()
        };
        let full = Report {
            songs: (0..50).map(top).collect(),
            artists: (0..50).map(top).collect(),
            genres: (0..50).map(top).collect(),
            ..Report::default()
        };
        let shared = full.summary();
        assert_eq!(shared.songs.len(), 10);
        assert_eq!(shared.artists.len(), 10);
        assert_eq!(shared.genres.len(), 5);
    }
}
