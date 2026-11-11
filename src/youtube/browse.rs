//! YouTube's catalogue without Google credentials: search, channels, playlists
//! and mixes through the helper's yt-dlp, and channels' newest uploads through
//! YouTube's public feeds. Runs on the backend runtime, never the UI thread.

use serde::Deserialize;

use super::{Error, Resolver, Thumbnail, Video, valid_id};
use crate::api::models::Track;
use crate::model::{SoundCloudCollection, SourceCard, SourceProfile, YouTubeSearchResults};

/// How many videos one page of a channel, playlist or search holds.
const PAGE: u32 = 30;
/// A channel's newest uploads kept for the feed, of all channels together.
const FEED_LIMIT: usize = 60;
const FEED_BYTES: usize = 2 * 1024 * 1024;

pub const HOME: &str = "youtube:home";
const CHANNEL_PREFIX: &str = "youtube:channel:";
const PLAYLIST_PREFIX: &str = "youtube:playlist:";
const MIX_PREFIX: &str = "youtube:mix:";
const VIDEO_CHANNEL_PREFIX: &str = "youtube:video-channel:";
pub const PLAYLISTS_TAB: &str = ":playlists";

/// A channel's id (`UC` and 22 characters) or its `@handle`. Nothing else ever
/// reaches a subprocess or a URL.
pub fn valid_channel(id: &str) -> bool {
    let safe = |bytes: &[u8]| {
        bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    };
    match id.strip_prefix('@') {
        Some(handle) => (1..=64).contains(&handle.len()) && safe(handle.as_bytes()),
        None => id.len() == 24 && id.starts_with("UC") && safe(id.as_bytes()),
    }
}

fn valid_playlist(id: &str) -> bool {
    (10..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

pub fn channel_uri(id: &str) -> String {
    format!("{CHANNEL_PREFIX}{id}")
}

pub fn playlist_uri(id: &str) -> String {
    format!("{PLAYLIST_PREFIX}{id}")
}

pub fn mix_uri(video: &str) -> String {
    format!("{MIX_PREFIX}{video}")
}

/// A page standing for the channel of a video whose channel is not known yet: a
/// song saved before channels were kept with it.
pub fn video_channel_uri(video: &str) -> String {
    format!("{VIDEO_CHANNEL_PREFIX}{video}")
}

/// What a browsing page's URI asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target<'a> {
    Home,
    Channel { id: &'a str, playlists: bool },
    Playlist(&'a str),
    Mix(&'a str),
    VideoChannel(&'a str),
}

pub fn target(uri: &str) -> Option<Target<'_>> {
    if uri == HOME {
        return Some(Target::Home);
    }
    if let Some(rest) = uri.strip_prefix(CHANNEL_PREFIX) {
        let (id, playlists) = match rest.strip_suffix(PLAYLISTS_TAB) {
            Some(id) => (id, true),
            None => (rest, false),
        };
        return valid_channel(id).then_some(Target::Channel { id, playlists });
    }
    if let Some(id) = uri.strip_prefix(PLAYLIST_PREFIX) {
        return valid_playlist(id).then_some(Target::Playlist(id));
    }
    if let Some(id) = uri.strip_prefix(MIX_PREFIX) {
        return valid_id(id).then_some(Target::Mix(id));
    }
    if let Some(id) = uri.strip_prefix(VIDEO_CHANNEL_PREFIX) {
        return valid_id(id).then_some(Target::VideoChannel(id));
    }
    None
}

/// Whether `uri` is a page of YouTube browsing.
pub fn is_page(uri: &str) -> bool {
    target(uri).is_some()
}

/// The channel `uri` belongs to, tab or not, for what a channel's tabs share.
pub fn channel_of(uri: &str) -> Option<&str> {
    match target(uri)? {
        Target::Channel { id, .. } => Some(id),
        _ => None,
    }
}

/// The page a pasted channel or playlist link opens. Videos are left to
/// [`super::parse`], which plays them.
pub fn page_for_link(text: &str) -> Option<String> {
    let url = reqwest::Url::parse(text.trim()).ok()?;
    if !matches!(url.scheme(), "https" | "http")
        || !matches!(
            url.host_str()?,
            "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com"
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    if path == "/playlist" {
        let id = url.query_pairs().find(|(key, _)| key == "list")?.1;
        return valid_playlist(&id).then(|| playlist_uri(&id));
    }
    let mut segments = path.trim_start_matches('/').split('/');
    let first = segments.next()?;
    let id = if first == "channel" {
        segments.next()?
    } else if first.starts_with('@') {
        first
    } else {
        return None;
    };
    valid_channel(id).then(|| channel_uri(id))
}

#[derive(Debug, Default, Deserialize)]
struct Listing {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    channel_id: Option<String>,
    #[serde(default)]
    uploader_id: Option<String>,
    #[serde(default)]
    channel_follower_count: Option<u64>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    thumbnails: Vec<ListingImage>,
    #[serde(default)]
    entries: Vec<Option<Entry>>,
}

#[derive(Debug, Default, Deserialize)]
struct ListingImage {
    #[serde(default)]
    id: Option<String>,
    url: String,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct Entry {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    ie_key: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    channel_id: Option<String>,
    #[serde(default)]
    uploader: Option<String>,
    #[serde(default)]
    uploader_id: Option<String>,
    #[serde(default)]
    channel_follower_count: Option<u64>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    view_count: Option<u64>,
    #[serde(default)]
    live_status: Option<String>,
    #[serde(default)]
    thumbnails: Vec<Thumbnail>,
}

/// A thumbnail or avatar on YouTube's own image hosts, over HTTPS. Avatars come
/// without a scheme.
pub(super) fn image_url(url: &str) -> Option<String> {
    let url = if url.starts_with("//") {
        format!("https:{url}")
    } else {
        url.to_owned()
    };
    let parsed = reqwest::Url::parse(&url).ok()?;
    let host = parsed.host_str()?;
    (parsed.scheme() == "https"
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && (host.ends_with(".ytimg.com")
            || host.ends_with(".ggpht.com")
            || host.ends_with(".googleusercontent.com")))
    .then_some(url)
}

fn entry_image(entry: &Entry) -> Option<String> {
    entry
        .thumbnails
        .iter()
        .max_by_key(|image| image.width.unwrap_or(0))
        .and_then(|image| image_url(&image.url))
}

/// The channel's round picture and its wide banner, by shape.
fn channel_images(images: &[ListingImage]) -> (Option<String>, Option<String>) {
    let square = |image: &&ListingImage| {
        image.id.as_deref() == Some("avatar_uncropped")
            || matches!((image.width, image.height), (Some(w), Some(h)) if w == h)
    };
    let wide = |image: &&ListingImage| matches!((image.width, image.height), (Some(w), Some(h)) if w > h * 2);
    let hosted = |image: &&ListingImage| image_url(&image.url).is_some();
    let avatar = images
        .iter()
        .filter(hosted)
        .filter(square)
        .max_by_key(|image| image.width.unwrap_or(0))
        .and_then(|image| image_url(&image.url));
    let banner = images
        .iter()
        .filter(hosted)
        .filter(wide)
        .max_by_key(|image| image.width.unwrap_or(0))
        .and_then(|image| image_url(&image.url));
    (avatar, banner)
}

fn followers(count: u64) -> String {
    let unit = if count == 1 {
        "subscriber"
    } else {
        "subscribers"
    };
    format!("{} {unit}", crate::util::format_compact(count))
}

/// The videos of a listing, without the live streams and upcoming premieres it
/// mixes in, which have no length and cannot be played as audio.
fn videos(listing: &Listing) -> Vec<Track> {
    listed(listing)
        .filter_map(|video| video.to_track().ok())
        .collect()
}

/// The videos of a listing as YouTube names them, for the matcher, which must
/// see "(Instrumental)" and the real channel rather than the tidied names.
fn raw_videos(listing: &Listing) -> Vec<Track> {
    listed(listing)
        .filter_map(|video| video.listing_track().ok())
        .collect()
}

fn listed(listing: &Listing) -> impl Iterator<Item = Video> + '_ {
    listing
        .entries
        .iter()
        .flatten()
        .filter(|entry| {
            valid_id(&entry.id) && entry.duration.is_some() && entry.live_status.is_none()
        })
        .map(|entry| Video {
            id: entry.id.clone(),
            title: entry.title.clone().unwrap_or_default(),
            channel: entry.channel.clone().or_else(|| listing.channel.clone()),
            channel_id: entry
                .channel_id
                .clone()
                .or_else(|| listing.channel_id.clone()),
            uploader: entry.uploader.clone(),
            description: None,
            duration: entry.duration,
            upload_date: None,
            view_count: entry.view_count,
            thumbnails: entry.thumbnails.clone(),
            is_live: false,
            live_status: None,
            availability: None,
        })
}

fn channel_cards(listing: &Listing) -> Vec<SourceCard> {
    listing
        .entries
        .iter()
        .flatten()
        .filter(|entry| valid_channel(&entry.id))
        .map(|entry| SourceCard {
            uri: channel_uri(&entry.id),
            title: entry
                .channel
                .clone()
                .or_else(|| entry.title.clone())
                .unwrap_or_default()
                .trim()
                .to_owned(),
            subtitle: entry
                .channel_follower_count
                .map(followers)
                .or_else(|| entry.uploader_id.clone())
                .unwrap_or_default(),
            image: entry_image(entry),
            round: true,
        })
        .collect()
}

fn playlist_cards(listing: &Listing) -> Vec<SourceCard> {
    listing
        .entries
        .iter()
        .flatten()
        .filter(|entry| {
            entry.ie_key.as_deref() != Some("Youtube")
                && !valid_channel(&entry.id)
                && valid_playlist(&entry.id)
        })
        .map(|entry| SourceCard {
            uri: playlist_uri(&entry.id),
            title: entry.title.clone().unwrap_or_default(),
            subtitle: entry
                .channel
                .clone()
                .or_else(|| listing.channel.clone())
                .unwrap_or_else(|| "Playlist".into()),
            image: entry_image(entry),
            round: false,
        })
        .collect()
}

fn profile(listing: &Listing, id: &str) -> SourceProfile {
    let (image, banner) = channel_images(&listing.thumbnails);
    let name = listing
        .channel
        .clone()
        .or_else(|| {
            listing
                .title
                .as_ref()
                .map(|title| title.trim_end_matches(" - Videos").to_owned())
        })
        .unwrap_or_default();
    let mut details = Vec::new();
    if let Some(handle) = &listing.uploader_id {
        details.push(handle.clone());
    }
    if let Some(count) = listing.channel_follower_count {
        details.push(followers(count));
    }
    SourceProfile {
        name,
        image,
        banner,
        details: details.join(" • "),
        about: listing.description.clone().unwrap_or_default(),
        link: Some(format!(
            "https://www.youtube.com/{}",
            if id.starts_with('@') {
                id.to_owned()
            } else {
                format!("channel/{id}")
            }
        )),
        id: listing
            .channel_id
            .clone()
            .filter(|id| valid_channel(id))
            .unwrap_or_else(|| id.to_owned()),
    }
}

fn parse_listing(bytes: &[u8]) -> Result<Listing, Error> {
    serde_json::from_slice(bytes).map_err(|_| Error::Metadata)
}

/// The page after `start`, while the last one came back full.
fn next_cursor(start: u32, entries: usize) -> Option<String> {
    (entries as u32 >= PAGE).then(|| (start + PAGE).to_string())
}

fn start_of(cursor: Option<&str>) -> u32 {
    cursor
        .and_then(|cursor| cursor.parse().ok())
        .filter(|start| *start >= 1)
        .unwrap_or(1)
}

async fn channel(
    resolver: &Resolver,
    id: &str,
    tab: &str,
    cursor: Option<&str>,
) -> Result<SoundCloudCollection, Error> {
    let start = start_of(cursor);
    let path = if id.starts_with('@') {
        format!("{id}/{tab}")
    } else {
        format!("channel/{id}/{tab}")
    };
    let bytes = resolver
        .listing(&format!("https://www.youtube.com/{path}"), start, PAGE)
        .await?;
    let listing = parse_listing(&bytes)?;
    let profile = profile(&listing, id);
    Ok(SoundCloudCollection {
        title: profile.name.clone(),
        tracks: videos(&listing),
        cards: playlist_cards(&listing),
        next: next_cursor(start, listing.entries.len()),
        profile: Some(profile),
        ..Default::default()
    })
}

async fn playlist(
    resolver: &Resolver,
    url: &str,
    fallback: &str,
    cursor: Option<&str>,
) -> Result<SoundCloudCollection, Error> {
    let start = start_of(cursor);
    let bytes = resolver.listing(url, start, PAGE).await?;
    let listing = parse_listing(&bytes)?;
    let tracks = videos(&listing);
    Ok(SoundCloudCollection {
        title: listing.title.clone().unwrap_or_else(|| fallback.to_owned()),
        image: tracks.first().and_then(|track| {
            track
                .album
                .as_ref()?
                .images
                .first()
                .map(|image| image.url.clone())
        }),
        tracks,
        next: next_cursor(start, listing.entries.len()),
        ..Default::default()
    })
}

/// Reads one browsing page. A channel with nothing on a tab has an empty tab,
/// not an error. The feed is [`feed`], which needs the followed channels.
pub async fn page(
    resolver: &Resolver,
    uri: &str,
    cursor: Option<&str>,
) -> Result<SoundCloudCollection, String> {
    let read = async {
        match target(uri).ok_or(Error::Metadata)? {
            Target::Home => Ok(SoundCloudCollection::default()),
            Target::Channel { id, playlists } => {
                if playlists {
                    match channel(resolver, id, "playlists", cursor).await {
                        Err(Error::Unavailable) => Ok(SoundCloudCollection::default()),
                        result => result,
                    }
                } else {
                    channel(resolver, id, "videos", cursor).await
                }
            }
            Target::Playlist(id) => {
                let url = format!("https://www.youtube.com/playlist?list={id}");
                playlist(resolver, &url, "YouTube playlist", cursor).await
            }
            Target::Mix(id) => {
                let url = format!("https://www.youtube.com/watch?v={id}&list=RD{id}");
                playlist(resolver, &url, "YouTube mix", cursor).await
            }
            Target::VideoChannel(id) => {
                let video = resolver.metadata(id).await?;
                let channel_id = video
                    .channel_id
                    .filter(|channel| valid_channel(channel))
                    .ok_or(Error::Metadata)?;
                channel(resolver, &channel_id, "videos", cursor).await
            }
        }
    };
    read.await.map_err(|error: Error| error.to_string())
}

/// The search results address, with `kind` being YouTube's own result filter.
fn results_url(query: &str, kind: &str) -> Option<String> {
    reqwest::Url::parse_with_params(
        "https://www.youtube.com/results",
        &[("search_query", query), ("sp", kind)],
    )
    .ok()
    .map(Into::into)
}

const VIDEOS: &str = "EgIQAQ==";
const CHANNELS: &str = "EgIQAg==";
const PLAYLISTS: &str = "EgIQAw==";

/// Channels found by name, each with its follower count.
pub async fn channels(
    resolver: &Resolver,
    query: &str,
) -> Result<Vec<(SourceCard, Option<u64>)>, String> {
    let read = async {
        let url = results_url(query, CHANNELS).ok_or(Error::Metadata)?;
        parse_listing(&resolver.listing(&url, 1, 8).await?)
    };
    let listing = read.await.map_err(|error: Error| error.to_string())?;
    let followers: std::collections::HashMap<String, Option<u64>> = listing
        .entries
        .iter()
        .flatten()
        .map(|entry| (channel_uri(&entry.id), entry.channel_follower_count))
        .collect();
    Ok(channel_cards(&listing)
        .into_iter()
        .map(|card| {
            let count = followers.get(&card.uri).copied().flatten();
            (card, count)
        })
        .collect())
}

/// The videos a search finds, and nothing else.
pub async fn videos_for(resolver: &Resolver, query: &str) -> Result<Vec<Track>, String> {
    let read = async {
        let url = results_url(query, VIDEOS).ok_or(Error::Metadata)?;
        parse_listing(&resolver.listing(&url, 1, PAGE).await?)
    };
    read.await
        .map(|listing: Listing| videos(&listing))
        .map_err(|error: Error| error.to_string())
}

/// The videos a search finds, as YouTube names them, for the matcher.
pub async fn videos_listed(resolver: &Resolver, query: &str) -> Result<Vec<Track>, String> {
    let read = async {
        let url = results_url(query, VIDEOS).ok_or(Error::Metadata)?;
        parse_listing(&resolver.listing(&url, 1, PAGE).await?)
    };
    read.await
        .map(|listing: Listing| raw_videos(&listing))
        .map_err(|error: Error| error.to_string())
}

/// How many of the albums YouTube Music finds are opened to look for the one
/// asked for.
const ALBUMS_OPENED: usize = 3;

/// The songs of `album` by `artist` on YouTube Music: the official upload of
/// every song, each with its exact length, from the artist's own channel. An
/// album is only taken when its name is the one asked for; otherwise nothing.
pub async fn album_songs(resolver: &Resolver, artist: &str, album: &str) -> Vec<Track> {
    let wanted = album_key(album);
    if wanted.is_empty() || artist.trim().is_empty() {
        return Vec::new();
    }
    let Some(url) = music_search_url(&format!("{artist} {album}"), "albums") else {
        return Vec::new();
    };
    let Ok(found) = async { parse_listing(&resolver.listing(&url, 1, 6).await?) }.await else {
        return Vec::new();
    };
    let browse: Vec<String> = found
        .entries
        .iter()
        .flatten()
        .map(|entry| entry.id.clone())
        .filter(|id| valid_album_browse(id))
        .take(ALBUMS_OPENED)
        .collect();
    for id in browse {
        let url = format!("https://music.youtube.com/browse/{id}");
        let Ok(listing) = async { parse_listing(&resolver.listing(&url, 1, 200).await?) }.await
        else {
            continue;
        };
        if listing
            .title
            .as_deref()
            .is_some_and(|title| album_key(album_title(title)) == wanted)
        {
            return raw_videos(&listing);
        }
    }
    Vec::new()
}

/// YouTube Music names an album's page "Album - Name", "Single - Name" or
/// "EP - Name".
fn album_title(title: &str) -> &str {
    ["Album - ", "Single - ", "EP - "]
        .iter()
        .find_map(|prefix| title.strip_prefix(prefix))
        .unwrap_or(title)
}

/// An album name to compare by: letters and digits only, lowercase.
fn album_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn valid_album_browse(id: &str) -> bool {
    id.strip_prefix("MPREb_").is_some_and(|rest| {
        !rest.is_empty()
            && rest.len() <= 64
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    })
}

fn music_search_url(query: &str, kind: &str) -> Option<String> {
    let mut url =
        reqwest::Url::parse_with_params("https://music.youtube.com/search", &[("q", query)])
            .ok()?;
    url.set_fragment(Some(kind));
    Some(url.into())
}

/// A search. A further page of videos leaves channels and playlists out.
pub async fn search(
    resolver: &Resolver,
    query: &str,
    cursor: Option<&str>,
) -> YouTubeSearchResults {
    let mut out = YouTubeSearchResults::default();
    let start = start_of(cursor);
    let list = |kind: &'static str, start: u32, count: u32| async move {
        let url = results_url(query, kind).ok_or(Error::Metadata)?;
        parse_listing(&resolver.listing(&url, start, count).await?)
    };
    let videos_read = list(VIDEOS, start, PAGE);
    let (videos_list, channels, playlists) = if cursor.is_some() {
        (
            videos_read.await,
            Ok(Listing::default()),
            Ok(Listing::default()),
        )
    } else {
        tokio::join!(videos_read, list(CHANNELS, 1, 8), list(PLAYLISTS, 1, 8))
    };
    match videos_list {
        Ok(listing) => {
            out.videos = videos(&listing);
            out.next = next_cursor(start, listing.entries.len());
        }
        Err(error) => out.error = Some(error.to_string()),
    }
    if let Ok(listing) = channels {
        out.channels = channel_cards(&listing);
    }
    if let Ok(listing) = playlists {
        out.playlists = playlist_cards(&listing);
    }
    out
}

fn tag<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

fn attribute<'a>(text: &'a str, element: &str, name: &str) -> Option<&'a str> {
    let at = text.find(element)?;
    let rest = &text[at..];
    let end = rest.find('>')?;
    tag(&rest[..end], &format!("{name}=\""), "\"")
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The newest uploads in one channel's public feed, each with when it went up.
/// Shorts cannot be told apart here but by their title.
fn parse_feed(xml: &str, channel_id: &str) -> Vec<(String, Track)> {
    let name = xml
        .split("<entry>")
        .next()
        .and_then(|head| tag(head, "<title>", "</title>"))
        .map(unescape)
        .unwrap_or_default();
    xml.split("<entry>")
        .skip(1)
        .filter_map(|entry| {
            let id = tag(entry, "<yt:videoId>", "</yt:videoId>")?;
            let title = unescape(tag(entry, "<title>", "</title>")?);
            if title.to_lowercase().contains("#shorts") {
                return None;
            }
            let published = tag(entry, "<published>", "</published>")?.to_owned();
            let thumbnail = attribute(entry, "<media:thumbnail", "url")?;
            // The description is where a producer writes the beat's tempo
            // and key; the entry's own words say nothing more here.
            let description = tag(entry, "<media:description>", "</media:description>")
                .map(unescape)
                .filter(|text| !text.trim().is_empty());
            let track = Video {
                id: id.to_owned(),
                title,
                channel: Some(name.clone()),
                channel_id: Some(channel_id.to_owned()),
                uploader: None,
                description,
                duration: None,
                upload_date: published.get(..10).map(|date| date.replace('-', "")),
                view_count: attribute(entry, "<media:statistics", "views")
                    .and_then(|views| views.parse().ok()),
                thumbnails: vec![Thumbnail {
                    url: thumbnail.to_owned(),
                    width: attribute(entry, "<media:thumbnail", "width")
                        .and_then(|width| width.parse().ok()),
                    height: attribute(entry, "<media:thumbnail", "height")
                        .and_then(|height| height.parse().ok()),
                }],
                is_live: false,
                live_status: None,
                availability: None,
            }
            .to_track()
            .ok()?;
            Some((published, track))
        })
        .collect()
}

/// Newest first, across the channels followed. A channel whose feed fails leaves
/// the others standing, unless all of them failed.
fn merge_feeds(mut uploads: Vec<(String, Track)>) -> Vec<Track> {
    uploads.sort_by(|a, b| b.0.cmp(&a.0));
    uploads.truncate(FEED_LIMIT);
    uploads.into_iter().map(|(_, track)| track).collect()
}

/// The newest uploads of the channels followed, from YouTube's public feeds.
pub async fn feed(
    http: reqwest::Client,
    channels: Vec<String>,
) -> Result<SoundCloudCollection, String> {
    let mut reads = tokio::task::JoinSet::new();
    let count = channels.len();
    for channel in channels
        .into_iter()
        .filter(|id| id.starts_with("UC") && valid_channel(id))
    {
        let http = http.clone();
        reads.spawn(async move {
            let url = format!("https://www.youtube.com/feeds/videos.xml?channel_id={channel}");
            let response = http.get(url).send().await.ok()?.error_for_status().ok()?;
            let bytes = response.bytes().await.ok()?;
            (bytes.len() <= FEED_BYTES)
                .then(|| parse_feed(&String::from_utf8_lossy(&bytes), &channel))
        });
    }
    let mut uploads = Vec::new();
    let mut read = 0;
    while let Some(result) = reads.join_next().await {
        if let Ok(Some(mut channel)) = result {
            read += 1;
            uploads.append(&mut channel);
        }
    }
    if read == 0 && count > 0 {
        return Err("YouTube could not be reached. Check your connection and try again.".into());
    }
    Ok(SoundCloudCollection {
        tracks: merge_feeds(uploads),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads real YouTube through the helper's yt-dlp and its feeds:
    /// `cargo test --lib youtube_browsing_works_for_real -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the helper and the network"]
    async fn youtube_browsing_works_for_real() {
        let resolver = Resolver::default();
        let found = search(&resolver, "dark type beat", None).await;
        assert_eq!(found.error, None);
        assert!(!found.videos.is_empty() && !found.channels.is_empty());
        assert!(!found.playlists.is_empty());
        let more = search(&resolver, "dark type beat", found.next.as_deref()).await;
        assert!(!more.videos.is_empty(), "the next page has videos");
        assert_ne!(more.videos[0].uri, found.videos[0].uri);

        let uri = found.videos[0].artists[0].uri.clone().unwrap();
        let Some(Target::Channel { id, .. }) = target(&uri) else {
            panic!("{uri}");
        };
        let shown = page(&resolver, &uri, None).await.unwrap();
        let profile = shown.profile.expect("a channel's page says who it is");
        assert!(!profile.name.is_empty() && profile.image.is_some());
        assert!(!shown.tracks.is_empty());
        let tab = format!("{uri}{PLAYLISTS_TAB}");
        page(&resolver, &tab, None).await.unwrap();
        let via_video = video_channel_uri(found.videos[0].id.as_deref().unwrap());
        assert!(
            page(&resolver, &via_video, None)
                .await
                .unwrap()
                .profile
                .is_some()
        );

        let list = found.playlists[0].uri.clone();
        assert!(
            !page(&resolver, &list, None)
                .await
                .unwrap()
                .tracks
                .is_empty()
        );
        let mix = mix_uri(found.videos[0].id.as_deref().unwrap());
        assert!(!page(&resolver, &mix, None).await.unwrap().tracks.is_empty());

        let video = found.videos[0].id.as_deref().unwrap();
        let audio = resolver.audio_url(video).await.unwrap();
        assert!(audio.contains("googlevideo.com"), "a video gives its audio");

        let uploads = feed(reqwest::Client::new(), vec![id.to_owned()])
            .await
            .unwrap();
        assert!(!uploads.tracks.is_empty());
    }

    const CHANNEL: &str = "UCliaQW3E6I9cBJkgm2fdOnw";

    #[test]
    fn only_well_formed_targets_become_pages() {
        assert_eq!(target(HOME), Some(Target::Home));
        assert_eq!(
            target(&channel_uri(CHANNEL)),
            Some(Target::Channel {
                id: CHANNEL,
                playlists: false
            })
        );
        assert_eq!(
            target(&format!("{}{PLAYLISTS_TAB}", channel_uri("@sanchebeats"))),
            Some(Target::Channel {
                id: "@sanchebeats",
                playlists: true
            })
        );
        for bad in [
            "youtube:channel:UCshort",
            "youtube:channel:@bad handle",
            "youtube:channel:--exec=calc",
            "youtube:playlist:--exec",
            "youtube:mix:tooshort",
            "youtube:likes",
            "youtube:video:eHz0FSfSLP8",
        ] {
            assert_eq!(target(bad), None, "{bad}");
        }
    }

    #[test]
    fn channel_and_playlist_links_open_their_pages() {
        for (link, page) in [
            (
                "https://www.youtube.com/channel/UCliaQW3E6I9cBJkgm2fdOnw/videos",
                channel_uri(CHANNEL),
            ),
            (
                "https://youtube.com/@sanchebeats",
                channel_uri("@sanchebeats"),
            ),
            (
                "https://www.youtube.com/playlist?list=PLVWUBCvQ0fKp8muwgS27rfCyA-CshIiFQ",
                playlist_uri("PLVWUBCvQ0fKp8muwgS27rfCyA-CshIiFQ"),
            ),
        ] {
            assert_eq!(page_for_link(link), Some(page), "{link}");
        }
        for other in [
            "https://www.youtube.com/watch?v=eHz0FSfSLP8",
            "https://evil.test/@sanchebeats",
            "https://www.youtube.com.evil.test/@sanchebeats",
            "dark type beat",
        ] {
            assert_eq!(page_for_link(other), None, "{other}");
        }
    }

    #[test]
    fn a_channel_listing_gives_its_profile_and_playable_videos() {
        let listing: Listing = serde_json::from_str(
            r#"{"title":"Sanche Beats - Videos","channel":"Sanche Beats",
            "channel_id":"UCliaQW3E6I9cBJkgm2fdOnw","uploader_id":"@sanchebeats",
            "channel_follower_count":66400,"description":"Dark beats",
            "thumbnails":[
              {"id":"0","url":"https://yt3.googleusercontent.com/b=w1060","width":1060,"height":175},
              {"id":"5","url":"https://yt3.googleusercontent.com/b=w2560","width":2560,"height":424},
              {"id":"banner_uncropped","url":"https://yt3.googleusercontent.com/b=s0"},
              {"id":"7","url":"https://yt3.googleusercontent.com/a=s900","width":900,"height":900},
              {"id":"avatar_uncropped","url":"https://yt3.googleusercontent.com/a=s0"},
              {"id":"evil","url":"https://evil.test/a.png","width":4000,"height":4000}
            ],
            "entries":[
              {"id":"sAXfRSvRVyQ","title":"VICES","duration":164,"view_count":748,
               "thumbnails":[{"url":"https://i.ytimg.com/vi/sAXfRSvRVyQ/hq720.jpg","width":720,"height":404}]},
              {"id":"7XtFkYxMsRw","title":"Live now","live_status":"is_live"},
              {"id":"8cCiNHMx41w","title":"Upcoming"},
              null
            ]}"#,
        )
        .unwrap();
        let profile = profile(&listing, CHANNEL);
        assert_eq!(profile.name, "Sanche Beats");
        assert_eq!(profile.details, "@sanchebeats • 66.4K subscribers");
        assert_eq!(
            profile.banner.as_deref(),
            Some("https://yt3.googleusercontent.com/b=w2560")
        );
        assert_eq!(
            profile.image.as_deref(),
            Some("https://yt3.googleusercontent.com/a=s900"),
            "an image off YouTube's hosts is never used"
        );
        let tracks = videos(&listing);
        assert_eq!(tracks.len(), 1, "live and upcoming videos are left out");
        assert_eq!(tracks[0].uri, "youtube:video:sAXfRSvRVyQ");
        assert_eq!(tracks[0].duration_ms, 164_000);
        assert_eq!(tracks[0].play_count, Some(748));
        assert_eq!(
            tracks[0].artists[0].uri.as_deref(),
            Some("youtube:channel:UCliaQW3E6I9cBJkgm2fdOnw"),
            "a video's channel is a link, taken from the listing"
        );
    }

    #[test]
    fn search_cards_are_channels_and_playlists_with_their_pictures() {
        let listing: Listing = serde_json::from_str(
            r#"{"entries":[
              {"id":"UCe2PodV-P_3hhS5t-zjrbYQ","title":"Dark","ie_key":"YoutubeTab",
               "channel":" Dark ","channel_follower_count":11600,
               "thumbnails":[{"url":"//yt3.ggpht.com/a=s176","width":176,"height":176}]},
              {"id":"PL_TkK1tLo3uLsSxEX2t6GM1umKvUx8OOJ","title":"Melodic","ie_key":"YoutubeTab",
               "channel":"XarBeats",
               "thumbnails":[{"url":"https://i.ytimg.com/vi/zyZi2kZZc9s/hq720.jpg","width":720,"height":404}]}
            ]}"#,
        )
        .unwrap();
        let channels = channel_cards(&listing);
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].title, "Dark");
        assert_eq!(channels[0].subtitle, "11.6K subscribers");
        assert_eq!(
            channels[0].image.as_deref(),
            Some("https://yt3.ggpht.com/a=s176")
        );
        assert!(channels[0].round);
        let playlists = playlist_cards(&listing);
        assert_eq!(playlists.len(), 1);
        assert_eq!(
            playlists[0].uri,
            "youtube:playlist:PL_TkK1tLo3uLsSxEX2t6GM1umKvUx8OOJ"
        );
        assert_eq!(playlists[0].subtitle, "XarBeats");
    }

    #[test]
    fn pages_continue_only_while_they_come_back_full() {
        assert_eq!(next_cursor(1, PAGE as usize).as_deref(), Some("31"));
        assert_eq!(next_cursor(31, 12), None);
        assert_eq!(start_of(Some("31")), 31);
        assert_eq!(start_of(Some("--flag")), 1);
        assert_eq!(start_of(None), 1);
    }

    #[test]
    fn a_feed_gives_uploads_newest_first_without_shorts() {
        let xml = r#"<feed><title>Sanche &amp; Co</title>
          <entry><yt:videoId>sAXfRSvRVyQ</yt:videoId><title>Old &quot;one&quot; | free trap type beat</title>
            <published>2026-01-01T10:00:00+00:00</published>
            <media:thumbnail url="https://i1.ytimg.com/vi/sAXfRSvRVyQ/hqdefault.jpg" width="480" height="360"/>
            <media:description>Old one up now. 140 BPM &amp; Key: C# minor.</media:description>
            <media:statistics views="1200"/></entry>
          <entry><yt:videoId>7XtFkYxMsRw</yt:videoId><title>A short #Shorts</title>
            <published>2026-03-01T10:00:00+00:00</published>
            <media:thumbnail url="https://i1.ytimg.com/vi/7XtFkYxMsRw/hqdefault.jpg" width="480" height="360"/></entry>
          <entry><yt:videoId>8cCiNHMx41w</yt:videoId><title>New</title>
            <published>2026-02-01T10:00:00+00:00</published>
            <media:thumbnail url="https://i1.ytimg.com/vi/8cCiNHMx41w/hqdefault.jpg" width="480" height="360"/></entry>
        </feed>"#;
        let tracks = merge_feeds(parse_feed(xml, CHANNEL));
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].uri, "youtube:video:8cCiNHMx41w");
        // A beat out of a channel's feed carries its own name, and the tempo
        // and key its description states, without another request made.
        assert_eq!(tracks[1].name, "one");
        assert_eq!(tracks[1].artists[0].name, "Sanche & Co");
        let beat = tracks[1].beat.as_ref().unwrap();
        assert_eq!(beat.for_artists, Vec::<String>::new());
        assert_eq!(beat.style.as_deref(), Some("Trap"));
        assert_eq!(beat.bpm, Some(140));
        assert_eq!(beat.key.as_deref(), Some("C\u{266F}m"));
        assert_eq!(beat.secondary().as_deref(), Some("Trap"));
        assert_eq!(
            crate::beats::tempo_key(&tracks[1]).as_deref(),
            Some("140 BPM \u{b7} C\u{266F}m")
        );
        assert_eq!(tracks[1].play_count, Some(1200));
        assert_eq!(tracks[1].release_date.as_deref(), Some("2026-01-01"));
    }
}

#[cfg(test)]
mod album_tests {
    use super::*;

    #[test]
    fn an_album_page_is_named_without_its_kind() {
        assert_eq!(album_title("Album - star"), "star");
        assert_eq!(album_title("Single - Raiider"), "Raiider");
        assert_eq!(album_title("EP - ep 1"), "ep 1");
        assert_eq!(album_title("Album"), "Album");
        assert_eq!(
            album_key("Hurry Up, We're Dreaming."),
            "hurryupweredreaming"
        );
        assert_eq!(album_key("★"), "");
    }

    #[test]
    fn only_a_youtube_music_album_is_opened() {
        assert!(valid_album_browse("MPREb_vCogBYqpxeT"));
        assert!(!valid_album_browse("MPREb_"));
        assert!(!valid_album_browse("UCabc"));
        assert!(!valid_album_browse("MPREb_../x"));
    }

    #[test]
    fn an_albums_songs_keep_their_own_names_and_channel() {
        let listing = parse_listing(
            br#"{"title":"Album - star","entries":[
                {"id":"Cx0dj7B61_o","title":"beginning (Instrumental)","duration":89,"channel":"2hollis"},
                {"id":"zCEb83y2r7A","title":"flash","duration":165,"channel":"2hollis"}
            ]}"#,
        )
        .unwrap();
        let songs = raw_videos(&listing);
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[0].name, "beginning (Instrumental)");
        assert_eq!(songs[0].artists[0].name, "2hollis");
        assert_eq!(songs[1].duration_ms, 165_000);
    }

    #[test]
    fn a_music_search_names_its_shelf() {
        assert_eq!(
            music_search_url("2hollis star", "albums").as_deref(),
            Some("https://music.youtube.com/search?q=2hollis+star#albums")
        );
    }
}
