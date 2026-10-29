//! Runtime-only catalogue reads. Search cursors are opaque and source-local.
use super::{api::Client, models};
use crate::model::{
    SoundCloudCollection, SoundCloudPlaylistSummary, SoundCloudSearchKind, SoundCloudSearchResults,
    SoundCloudUserSummary,
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

pub async fn collection(
    client: &Client,
    uri: &str,
    cursor: Option<&str>,
) -> Result<SoundCloudCollection, String> {
    let read = async {
        if let Some(id) = super::playlist_id(uri) {
            let value = client.playlist(id).await?;
            Ok(SoundCloudCollection {
                title: value.title.clone(),
                tracks: value
                    .tracks
                    .iter()
                    .map(|track| track.to_track(Some(&value)))
                    .collect(),
                next: None,
            })
        } else if let Some(id) = super::user_id(uri) {
            let title = if cursor.is_none() {
                client.user(id).await?.username
            } else {
                String::new()
            };
            let page: models::Page<models::Track> = match cursor {
                Some(cursor) => client.next_page(cursor).await?,
                None => client.user_tracks(id, 50, 0).await?,
            };
            Ok(SoundCloudCollection {
                title,
                tracks: page
                    .collection
                    .iter()
                    .map(|track| track.to_track(None))
                    .collect(),
                next: page.next_href,
            })
        } else {
            Err(super::api::Error::InvalidUrl)
        }
    };
    read.await.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
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
