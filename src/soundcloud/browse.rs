//! Runtime-only catalogue reads. Search cursors are opaque and source-local.
use super::{api::Client, models};
use crate::model::{
    SoundCloudCollection, SoundCloudPlaylistSummary, SoundCloudSearchKind, SoundCloudSearchResults,
    SoundCloudUserSummary, SourceCard, SourceProfile, SourceShelf,
};

#[derive(Clone, Debug)]
pub struct SearchRequest {
    pub query: String,
    pub serial: u64,
    pub request: u64,
    pub more: Option<(SoundCloudSearchKind, String)>,
}

pub fn is_url(query: &str) -> bool {
    reqwest::Url::parse(query.trim()).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && matches!(
                url.host_str(),
                Some(
                    "soundcloud.com"
                        | "www.soundcloud.com"
                        | "m.soundcloud.com"
                        | "on.soundcloud.com"
                )
            )
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
    })
}

fn playlist(value: models::Playlist) -> SoundCloudPlaylistSummary {
    let converted = value.to_playlist();
    SoundCloudPlaylistSummary {
        id: value.id,
        image: crate::api::models::pick_image(&converted.images, 300).map(str::to_owned),
        name: converted.name,
        creator: converted.owner.display_name.unwrap_or_default(),
    }
}

fn user(value: models::User) -> SoundCloudUserSummary {
    let converted = value.to_artist();
    SoundCloudUserSummary {
        id: value.id,
        image: crate::api::models::pick_image(&converted.images, 300).map(str::to_owned),
        name: converted.name,
    }
}

fn tracks(
    result: super::api::Result<models::Page<models::Track>>,
    out: &mut SoundCloudSearchResults,
) {
    match result {
        Ok(page) => {
            out.tracks = page
                .collection
                .iter()
                .map(|track| track.to_track(None))
                .collect();
            out.next[0] = page.next_href;
        }
        Err(error) => out.errors.push(error.to_string()),
    }
}
fn playlists(
    result: super::api::Result<models::Page<models::Playlist>>,
    out: &mut SoundCloudSearchResults,
) {
    match result {
        Ok(page) => {
            out.playlists = page.collection.into_iter().map(playlist).collect();
            out.next[1] = page.next_href;
        }
        Err(error) => out.errors.push(error.to_string()),
    }
}
fn users(
    result: super::api::Result<models::Page<models::User>>,
    out: &mut SoundCloudSearchResults,
) {
    match result {
        Ok(page) => {
            out.users = page.collection.into_iter().map(user).collect();
            out.next[2] = page.next_href;
        }
        Err(error) => out.errors.push(error.to_string()),
    }
}

pub async fn search(client: &Client, request: &SearchRequest) -> SoundCloudSearchResults {
    let mut out = SoundCloudSearchResults::default();
    if let Some((kind, cursor)) = &request.more {
        match kind {
            SoundCloudSearchKind::Tracks => tracks(client.next_page(cursor).await, &mut out),
            SoundCloudSearchKind::Playlists => playlists(client.next_page(cursor).await, &mut out),
            SoundCloudSearchKind::Users => users(client.next_page(cursor).await, &mut out),
        }
        return out;
    }
    if is_url(&request.query) {
        let mut resolved = SoundCloudSearchResults::default();
        match client.resolve(&request.query).await {
            Ok(models::Resolved::Track(track)) => resolved.tracks.push(track.to_track(None)),
            Ok(models::Resolved::Playlist(value)) => resolved.playlists.push(playlist(value)),
            Ok(models::Resolved::User(value)) => resolved.users.push(user(value)),
            Err(error) => out.errors.push(error.to_string()),
        }
        out.resolved = Some(Box::new(resolved));
        return out;
    }
    let (t, p, u) = tokio::join!(
        client.search_tracks(&request.query, 20, 0),
        client.search_playlists(&request.query, 20, 0),
        client.search_users(&request.query, 20, 0)
    );
    tracks(t, &mut out);
    playlists(p, &mut out);
    users(u, &mut out);
    out
}

/// Preserve every occurrence while its metadata batch is in flight.
pub(crate) fn playlist_add_rows(playlist: &models::Playlist) -> Vec<crate::api::models::Track> {
    playlist
        .tracks
        .iter()
        .map(|track| {
            let mut row = track.to_track(Some(playlist));
            if track.is_stub() {
                row.name = "Loading SoundCloud track…".into();
            }
            row
        })
        .collect()
}

fn user_profile(user: &models::User) -> SourceProfile {
    let artist = user.to_artist();
    let mut details = Vec::new();
    if let Some(count) = user.followers_count {
        details.push(format!("{} followers", crate::util::format_compact(count)));
    }
    if let Some(count) = user.track_count {
        details.push(format!("{} tracks", crate::util::format_count(count)));
    }
    if let Some(city) = user.city.as_ref().filter(|city| !city.trim().is_empty()) {
        details.push(city.clone());
    }
    SourceProfile {
        id: user.id.to_string(),
        name: user.username.clone(),
        image: crate::api::models::pick_image(&artist.images, 500).map(str::to_owned),
        banner: user
            .visuals
            .as_ref()
            .and_then(|visuals| visuals.visuals.first())
            .and_then(|visual| visual.visual_url.clone()),
        details: details.join(" • "),
        about: user.description.clone().unwrap_or_default(),
        link: super::permalink(user.permalink_url.as_deref()).map(str::to_owned),
    }
}

fn person_card(user: &models::User) -> SourceCard {
    let summary = self::user(user.clone());
    SourceCard {
        uri: super::user_uri(user.id),
        title: summary.name,
        subtitle: user
            .followers_count
            .map(|count| format!("{} followers", crate::util::format_compact(count)))
            .unwrap_or_default(),
        image: summary.image,
        round: true,
    }
}

fn set_card(value: &models::Playlist) -> SourceCard {
    let album = value.is_album.unwrap_or(false);
    let summary = self::playlist(value.clone());
    SourceCard {
        uri: super::playlist_uri(summary.id),
        title: summary.name,
        subtitle: if album && summary.creator.is_empty() {
            "Album".into()
        } else {
            summary.creator
        },
        image: summary.image,
        round: false,
    }
}

/// SoundCloud's own rows: its own sets, and playlists it generates.
fn selection_shelves(selections: Vec<models::Selection>) -> Vec<SourceShelf> {
    selections
        .into_iter()
        .filter_map(|selection| {
            let cards: Vec<SourceCard> = selection
                .items
                .collection
                .into_iter()
                .filter_map(|item| {
                    let uri = match item.kind.as_str() {
                        "playlist" => super::playlist_uri(item.id.as_u64()?),
                        "system-playlist" => {
                            let uri = super::system_uri(item.urn.as_deref()?);
                            super::system_urn(&uri)?;
                            uri
                        }
                        _ => return None,
                    };
                    Some(SourceCard {
                        uri,
                        title: item.title,
                        subtitle: item
                            .short_description
                            .or_else(|| item.user.map(|user| user.username))
                            .unwrap_or_default(),
                        image: models::large_artwork(
                            item.artwork_url
                                .as_deref()
                                .or(item.calculated_artwork_url.as_deref()),
                        ),
                        round: false,
                    })
                })
                .collect();
            (!cards.is_empty()).then_some(SourceShelf {
                title: selection.title,
                cards,
            })
        })
        .collect()
}

fn converted(tracks: &[models::Track]) -> Vec<crate::api::models::Track> {
    tracks.iter().map(|track| track.to_track(None)).collect()
}

/// A profile's page: its details on the first page of any tab, and that tab.
async fn user_page(
    client: &Client,
    id: u64,
    tab: &str,
    cursor: Option<&str>,
) -> super::api::Result<SoundCloudCollection> {
    const PAGE: u32 = 50;
    const CARDS: u32 = 30;
    let mut out = SoundCloudCollection::default();
    if cursor.is_none() {
        let user = client.user(id).await?;
        out.title = user.username.clone();
        out.profile = Some(user_profile(&user));
    }
    match tab {
        "albums" | "playlists" => {
            let kind = if tab == "albums" {
                "albums"
            } else {
                "playlists_without_albums"
            };
            let page: models::Page<models::Playlist> = match cursor {
                Some(cursor) => client.next_page(cursor).await?,
                None => client.user_tab(id, kind, CARDS).await?,
            };
            out.cards = page.collection.iter().map(set_card).collect();
            out.next = page.next_href;
        }
        "following" => {
            let page: models::Page<models::User> = match cursor {
                Some(cursor) => client.next_page(cursor).await?,
                None => client.user_tab(id, "followings", CARDS).await?,
            };
            out.cards = page.collection.iter().map(person_card).collect();
            out.next = page.next_href;
        }
        "reposts" => {
            let page: models::Page<models::RepostItem> = match cursor {
                Some(cursor) => client.next_page(cursor).await?,
                None => client.user_reposts(id, PAGE).await?,
            };
            out.tracks = page
                .collection
                .iter()
                .filter_map(|item| item.track.as_ref())
                .map(|track| track.to_track(None))
                .collect();
            out.next = page.next_href;
        }
        "likes" => {
            let page: models::Page<models::LikeItem> = match cursor {
                Some(cursor) => client.next_page(cursor).await?,
                None => client.user_tab(id, "likes", PAGE).await?,
            };
            out.tracks = page
                .collection
                .iter()
                .filter_map(|item| item.track.as_ref())
                .map(|track| track.to_track(None))
                .collect();
            out.next = page.next_href;
        }
        "popular" => {
            let page: models::Page<models::Track> = match cursor {
                Some(cursor) => client.next_page(cursor).await?,
                None => client.user_tab(id, "toptracks", PAGE).await?,
            };
            out.tracks = converted(&page.collection);
            out.next = page.next_href;
        }
        _ => {
            let page: models::Page<models::Track> = match cursor {
                Some(cursor) => client.next_page(cursor).await?,
                None => client.user_tracks(id, PAGE, 0).await?,
            };
            out.tracks = converted(&page.collection);
            out.next = page.next_href;
        }
    }
    Ok(out)
}

/// Discover: what's trending, under SoundCloud's own rows. Either may fail alone.
async fn home(client: &Client) -> super::api::Result<SoundCloudCollection> {
    let (selections, trending) = tokio::join!(client.selections(), client.trending(20));
    if let (Err(error), Err(_)) = (&selections, &trending) {
        return Err(match error {
            super::api::Error::InvalidUrl => super::api::Error::InvalidUrl,
            _ => selections.err().expect("checked above"),
        });
    }
    let mut out = SoundCloudCollection {
        title: "SoundCloud".into(),
        ..Default::default()
    };
    if let Ok(page) = selections {
        out.shelves = selection_shelves(page.collection);
    }
    if let Ok(page) = trending {
        out.tracks = page
            .collection
            .iter()
            .map(|item| item.track.to_track(None))
            .collect();
    }
    Ok(out)
}

async fn system_playlist(client: &Client, urn: &str) -> super::api::Result<SoundCloudCollection> {
    let value = client.system_playlist(urn).await?;
    let stubs: Vec<u64> = value
        .tracks
        .iter()
        .filter(|track| track.is_stub())
        .map(|track| track.id)
        .collect();
    let filled = if stubs.is_empty() {
        Vec::new()
    } else {
        // A batch at a time, in the playlist's own order below.
        let mut filled = Vec::new();
        for batch in stubs.chunks(50) {
            filled.extend(client.tracks(batch).await?);
        }
        filled
    };
    let tracks = value
        .tracks
        .iter()
        .filter_map(|track| {
            if track.is_stub() {
                filled.iter().find(|full| full.id == track.id)
            } else {
                Some(track)
            }
        })
        .map(|track| track.to_track(None))
        .collect();
    Ok(SoundCloudCollection {
        title: value.title,
        image: models::large_artwork(
            value
                .artwork_url
                .as_deref()
                .or(value.calculated_artwork_url.as_deref()),
        ),
        tracks,
        ..Default::default()
    })
}

pub async fn collection(
    client: &Client,
    uri: &str,
    cursor: Option<&str>,
) -> Result<SoundCloudCollection, String> {
    let read = async {
        if uri == super::HOME {
            home(client).await
        } else if uri == super::FEED {
            Ok(SoundCloudCollection::default())
        } else if let Some(genre) = super::charts_genre(uri) {
            system_playlist(
                client,
                &format!("soundcloud:system-playlists:trending-by-genre:{genre}"),
            )
            .await
        } else if let Some(urn) = super::system_urn(uri) {
            system_playlist(client, urn).await
        } else if let Some(id) = super::related_id(uri) {
            let page: models::Page<models::Track> = match cursor {
                Some(cursor) => client.next_page(cursor).await?,
                None => client.related_tracks(id, 50).await?,
            };
            let title = if cursor.is_none() {
                let seed = client.track(id).await?.to_track(None);
                format!("Related to {}", seed.name)
            } else {
                String::new()
            };
            Ok(SoundCloudCollection {
                title,
                tracks: converted(&page.collection),
                next: page.next_href,
                ..Default::default()
            })
        } else if let Some(id) = super::playlist_id(uri) {
            let value = client.playlist(id).await?;
            let converted = value.to_playlist();
            Ok(SoundCloudCollection {
                title: value.title.clone(),
                image: crate::api::models::pick_image(&converted.images, 500).map(str::to_owned),
                tracks: value
                    .tracks
                    .iter()
                    .map(|track| track.to_track(Some(&value)))
                    .collect(),
                ..Default::default()
            })
        } else if let Some((id, tab)) = super::user_tab(uri) {
            user_page(client, id, tab, cursor).await
        } else {
            Err(super::api::Error::InvalidUrl)
        }
    };
    read.await.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads real SoundCloud with no account:
    /// `cargo test --lib soundcloud_browsing_works_for_real -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the network"]
    async fn soundcloud_browsing_works_for_real() {
        let client = Client::new(reqwest::Client::new(), None, None);
        let home = collection(&client, super::super::HOME, None).await.unwrap();
        assert!(!home.tracks.is_empty() && !home.shelves.is_empty());
        for (genre, name) in super::super::GENRES {
            let shown = collection(&client, &super::super::charts_uri(genre), None)
                .await
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(!shown.tracks.is_empty(), "{name}");
        }

        let found = client.search_users("flume", 1, 0).await.unwrap();
        let id = found.collection[0].id;
        let base = super::super::user_uri(id);
        let uploads = collection(&client, &base, None).await.unwrap();
        assert!(uploads.profile.is_some() && !uploads.tracks.is_empty());
        let next = uploads.next.as_deref().expect("uploads have more");
        assert!(
            !collection(&client, &base, Some(next))
                .await
                .unwrap()
                .tracks
                .is_empty()
        );
        for tab in super::super::USER_TABS {
            let shown = collection(&client, &format!("{base}:{tab}"), None)
                .await
                .unwrap();
            assert!(shown.profile.is_some(), "{tab}");
            println!(
                "{tab}: {} tracks, {} cards",
                shown.tracks.len(),
                shown.cards.len()
            );
        }
        let trending = client.trending(10).await.unwrap();
        let mut played = false;
        for item in &trending.collection {
            if client.resolve_stream(&item.track).await.is_ok() {
                played = true;
                break;
            }
        }
        assert!(played, "something trending has a stream");
        let track = crate::soundcloud::track_id(&uploads.tracks[0].uri).unwrap();
        let related = super::super::related_uri(track);
        let shown = collection(&client, &related, None).await.unwrap();
        assert!(!shown.tracks.is_empty() && shown.title.starts_with("Related to"));
        let system = home
            .shelves
            .iter()
            .flat_map(|shelf| &shelf.cards)
            .find(|card| super::super::system_urn(&card.uri).is_some())
            .expect("a generated playlist on Discover");
        let shown = collection(&client, &system.uri, None).await.unwrap();
        assert!(!shown.tracks.is_empty());
    }

    #[test]
    fn browsing_pages_are_recognised_and_nothing_else() {
        for page in [
            super::super::HOME.to_owned(),
            super::super::FEED.to_owned(),
            super::super::user_uri(42),
            format!("{}:popular", super::super::user_uri(42)),
            format!("{}:following", super::super::user_uri(42)),
            super::super::playlist_uri(7),
            super::super::charts_uri("trap"),
            super::super::related_uri(9),
            super::super::system_uri("soundcloud:system-playlists:trending-by-genre:trap"),
        ] {
            assert!(super::super::is_page(&page), "{page}");
        }
        for other in [
            "soundcloud:track:1",
            "soundcloud:user:42:secret",
            "soundcloud:user:nope",
            "soundcloud:charts:not-a-genre",
            "soundcloud:system:../../me",
            "soundcloud:system:soundcloud:system-playlists:a/b",
            "soundcloud:related:x",
        ] {
            assert!(!super::super::is_page(other), "{other}");
        }
    }

    #[test]
    fn a_selection_becomes_a_shelf_of_what_can_be_opened() {
        let selections: Vec<models::Selection> = serde_json::from_str(
            r#"[{"title":"Trending by genre","items":{"collection":[
                {"kind":"system-playlist","urn":"soundcloud:system-playlists:trending-by-genre:trap",
                 "id":"soundcloud:system-playlists:trending-by-genre:trap","title":"Trap",
                 "short_description":"Trending",
                 "calculated_artwork_url":"https://i1.sndcdn.com/a-t500x500.jpg"},
                {"kind":"playlist","id":734469195,"title":"Emerging Indie",
                 "user":{"id":1,"username":"Scenes"},
                 "artwork_url":"https://i1.sndcdn.com/b-large.jpg"},
                {"kind":"something-new","id":3,"title":"Unknown"}
              ]}},{"title":"Empty","items":{"collection":[]}}]"#,
        )
        .unwrap();
        let shelves = selection_shelves(selections);
        assert_eq!(shelves.len(), 1, "an empty row is left out");
        let cards = &shelves[0].cards;
        assert_eq!(cards.len(), 2, "a kind unknown to the app is left out");
        assert_eq!(
            cards[0].uri,
            "soundcloud:system:soundcloud:system-playlists:trending-by-genre:trap"
        );
        assert_eq!(cards[0].subtitle, "Trending");
        assert_eq!(cards[1].uri, "soundcloud:playlist:734469195");
        assert_eq!(cards[1].subtitle, "Scenes");
    }

    #[test]
    fn a_profile_says_who_they_are_in_one_line() {
        let user: models::User = serde_json::from_str(
            r#"{"id":2976616,"username":"Flume","followers_count":2228750,"track_count":412,
                "city":"Sydney","description":"Bio",
                "permalink_url":"https://soundcloud.com/flume",
                "avatar_url":"https://i1.sndcdn.com/avatars-large.jpg",
                "visuals":{"visuals":[{"visual_url":"https://i1.sndcdn.com/visuals.jpg"}]}}"#,
        )
        .unwrap();
        let profile = user_profile(&user);
        assert_eq!(profile.details, "2.2M followers • 412 tracks • Sydney");
        assert_eq!(
            profile.banner.as_deref(),
            Some("https://i1.sndcdn.com/visuals.jpg")
        );
        assert_eq!(
            profile.link.as_deref(),
            Some("https://soundcloud.com/flume")
        );
        assert_eq!(profile.id, "2976616");
    }
    #[test]
    fn detects_only_soundcloud_web_links() {
        for url in [
            "https://soundcloud.com/artist/song",
            "https://on.soundcloud.com/abc",
            "http://m.soundcloud.com/person",
        ] {
            assert!(is_url(url));
        }
        for query in [
            "SoundCloud ambient",
            "https://soundcloud.com.evil.test/song",
            "https://evil.test/soundcloud.com",
            "https://secret@soundcloud.com/a",
            "soundcloud:track:1",
        ] {
            assert!(!is_url(query));
        }
    }
}
