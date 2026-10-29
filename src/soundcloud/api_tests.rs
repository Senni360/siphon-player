use super::*;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const OLD_ID: &str = "00000000000000000000000000000000";
const NEW_ID: &str = "11111111111111111111111111111111";

fn fixture_track() -> Track {
    serde_json::from_str(include_str!("../testdata/soundcloud/track.json")).unwrap()
}

#[tokio::test]
async fn extension_bridge_resolves_tracks_and_complete_sets_but_rejects_profiles() {
    use crate::extension_bridge::{ResolvedItem, resolve_soundcloud};
    let (client, task) = server(vec![
        ok(include_str!("../testdata/soundcloud/track.json")),
        ok(
            json!({"kind":"playlist","id":7,"title":"Set","tracks":[{"id":1},{"id":2},{"id":1}]})
                .to_string(),
        ),
        ok(json!([{"id":1,"title":"One"},{"id":2,"title":"Two"}]).to_string()),
        ok(include_str!("../testdata/soundcloud/user.json")),
    ])
    .await;
    assert!(matches!(
        resolve_soundcloud(&client, "https://soundcloud.com/artist/song")
            .await
            .unwrap(),
        ResolvedItem::Track(_)
    ));
    let ResolvedItem::Set { uri, tracks, .. } =
        resolve_soundcloud(&client, "https://soundcloud.com/artist/sets/set")
            .await
            .unwrap()
    else {
        panic!("set resolved");
    };
    assert_eq!(uri, "soundcloud:playlist:7");
    assert_eq!(
        tracks
            .iter()
            .map(|track| track.uri.as_str())
            .collect::<Vec<_>>(),
        [
            "soundcloud:track:1",
            "soundcloud:track:2",
            "soundcloud:track:1"
        ]
    );
    assert!(
        resolve_soundcloud(&client, "https://soundcloud.com/artist/song-shaped-profile")
            .await
            .is_err()
    );
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("GET /resolve?url="));
    assert!(requests[2].starts_with("GET /tracks?ids="));
}

fn fixture_formats() -> Track {
    serde_json::from_str(include_str!("../testdata/soundcloud/transcodings.json")).unwrap()
}

#[test]
fn extracts_only_a_valid_client_id_from_an_asset() {
    let fixture: Value =
        serde_json::from_str(include_str!("../testdata/soundcloud/client_asset.json")).unwrap();
    assert_eq!(
        extract_client_id(fixture["script"].as_str().unwrap()).as_deref(),
        fixture["expected_client_id"].as_str()
    );
    for script in [
        "client_id: null",
        "client_id:\"short\"",
        "client_id:42",
        "client_id:\"",
        "other_client_id:\"ffffffffffffffffffffffffffffffff\"",
    ] {
        assert!(extract_client_id(script).is_none());
    }
}

#[test]
fn finds_script_assets_with_or_without_crossorigin() {
    let html = r#"<script src="/without-crossorigin.js"></script>
        <script crossorigin src="https://a-v2.sndcdn.com/assets/one.js"></script>
        <script src='//a-v2.sndcdn.com/assets/two.js?a=1&amp;b=2' crossorigin="anonymous"></script>
        <script crossorigin src="https://attacker.invalid/external.js"></script>"#;
    let assets = script_assets(html, &Url::parse(HOME).unwrap());
    assert_eq!(
        assets.iter().map(Url::as_str).collect::<Vec<_>>(),
        [
            "https://soundcloud.com/without-crossorigin.js",
            "https://a-v2.sndcdn.com/assets/one.js",
            "https://a-v2.sndcdn.com/assets/two.js?a=1&b=2"
        ]
    );
}

#[test]
fn ranks_formats_and_never_selects_drm_or_adaptive_manifests() {
    let track = fixture_formats();
    let mut choices = track.media.unwrap().transcodings;
    choices.retain(|item| item.preference().is_some());
    choices.sort_by_key(Transcoding::preference);
    assert_eq!(
        choices
            .iter()
            .map(|item| item.preset.as_str())
            .collect::<Vec<_>>(),
        ["mp3_0_0", "aac_160k", "aac_96k", "mp3_0_0"]
    );
    assert_eq!(choices[0].protocol(), Some(Protocol::Progressive));
    assert_eq!(choices[3].protocol(), Some(Protocol::Hls));
    let mut mislabeled = choices[1].clone();
    mislabeled.url = "https://api-v2.soundcloud.com/media/stream/ctr-encrypted-hls".into();
    assert!(mislabeled.preference().is_none());
    mislabeled.url = "https://api-v2.soundcloud.com/media/stream/hls".into();
    mislabeled.preset.clear();
    mislabeled.format.mime_type = "audio/mpegurl".into();
    assert!(mislabeled.preference().is_none());
    let live = fixture_track();
    assert!(
        live.media
            .unwrap()
            .transcodings
            .iter()
            .any(|item| item.codec() == Some("aac") && item.preference().is_some())
    );
}

#[test]
fn progressive_aac_remains_playable_when_it_is_the_only_encoding() {
    for mime in ["audio/aac", "audio/mp4; codecs=\"mp4a.40.2\""] {
        let mut track = fixture_formats();
        let mut transcoding = track.media.as_ref().unwrap().transcodings[0].clone();
        transcoding.url = "https://api-v2.soundcloud.com/media/progressive".into();
        transcoding.preset = "aac_160k".into();
        transcoding.format.protocol = "progressive".into();
        transcoding.format.mime_type = mime.into();
        assert!(transcoding.preference().is_some());
        track.media.as_mut().unwrap().transcodings = vec![transcoding];
        assert!(track.is_playable());
    }
}

#[test]
fn preview_detection_includes_policy_flags_and_shorter_duration() {
    let tracks: Vec<Track> =
        serde_json::from_str(include_str!("../testdata/soundcloud/previews.json")).unwrap();
    assert_eq!(
        tracks.iter().map(Track::is_preview).collect::<Vec<_>>(),
        [true, true, true, false, false]
    );
    assert_eq!(
        tracks
            .iter()
            .map(|track| track.to_track(None).preview_only)
            .collect::<Vec<_>>(),
        [true, true, true, false, false]
    );
    let track = fixture_formats();
    let mut item = track.media.as_ref().unwrap().transcodings[4].clone();
    item.duration = 30_000;
    let info = stream_info(&track, &item, "https://example.invalid/audio".into());
    assert!(info.is_preview);
    assert_eq!(info.duration_ms, 30_000);
    item.duration = track.duration;
    item.snipped = true;
    assert!(stream_info(&track, &item, String::new()).is_preview);
    item.snipped = false;
    item.url = "https://api-v2.soundcloud.com/media/preview/track".into();
    assert!(stream_info(&track, &item, String::new()).is_preview);
    item.url = "https://api-v2.soundcloud.com/media/track".into();
    for url in [
        "https://cf-preview-media.sndcdn.com/preview/0/30/audio.mp3",
        "https://cf-hls-media.sndcdn.com/playlist/0/30/audio.m3u8",
    ] {
        let info = stream_info(&track, &item, url.into());
        assert!(info.is_preview);
        assert_eq!(info.duration_ms, 30_000);
    }
}

#[test]
fn converts_tracks_with_full_duration_uploader_identity_and_artwork() {
    let mut raw = fixture_track();
    raw.policy = Some("SNIP".into());
    raw.duration = 30_000;
    let converted = raw.to_track(None);
    assert_eq!(converted.id, Some(raw.id.to_string()));
    assert_eq!(converted.uri, super::super::track_uri(raw.id));
    assert_eq!(converted.duration_ms, raw.full_duration.unwrap());
    assert!(!converted.explicit);
    assert_eq!(converted.is_playable, Some(true));
    assert!(converted.preview_only);
    let restored: crate::api::models::Track =
        serde_json::from_value(serde_json::to_value(&converted).unwrap()).unwrap();
    assert!(
        restored.preview_only,
        "local playlist snapshots keep preview availability"
    );
    let old: crate::api::models::Track = serde_json::from_str("{}").unwrap();
    assert!(!old.preview_only, "old snapshots remain readable");
    assert_eq!(
        converted.artists[0].name,
        raw.publisher_metadata
            .as_ref()
            .unwrap()
            .artist
            .as_ref()
            .unwrap()
            .as_str()
    );
    let user = raw.user.as_ref().unwrap();
    assert_eq!(
        converted.artists[0].uri,
        Some(super::super::user_uri(user.id))
    );
    assert_eq!(converted.artists[0].id, Some(user.id.to_string()));
    assert_eq!(converted.external_urls.spotify, raw.permalink_url);
    assert!(
        converted.album.unwrap().images[0]
            .url
            .ends_with("-t500x500.jpg")
    );
    raw.publisher_metadata = None;
    raw.artwork_url = None;
    let playlist: Playlist =
        serde_json::from_str(include_str!("../testdata/soundcloud/playlist.json")).unwrap();
    let converted = raw.to_track(Some(&playlist));
    assert_eq!(converted.artists[0].name, user.username);
    let album = converted.album.unwrap();
    assert_eq!(album.name, playlist.title);
    assert_eq!(album.uri, super::super::playlist_uri(playlist.id));
    // The default avatar is a fixed resource, without a resizable suffix.
    assert_eq!(
        Some(album.images[0].url.as_str()),
        user.avatar_url.as_deref()
    );
    assert_eq!(album.images[0].width, None);
}

#[test]
fn blocked_unstreamable_and_encrypted_only_tracks_are_unplayable() {
    let mut track = fixture_formats();
    track.policy = Some("BLOCK".into());
    assert_eq!(track.to_track(None).is_playable, Some(false));
    track.policy = Some("ALLOW".into());
    track.streamable = Some(false);
    assert!(!track.is_playable());
    track.streamable = Some(true);
    track.media.as_mut().unwrap().transcodings.truncate(2);
    assert!(!track.is_playable());
}

#[tokio::test]
async fn opus_only_tracks_are_rejected_before_resolving_an_undecodable_stream() {
    let client = Client::new(Http::default(), None, None);
    let mut track = fixture_formats();
    track
        .media
        .as_mut()
        .unwrap()
        .transcodings
        .retain(|item| item.codec() == Some("opus"));
    assert!(!track.is_playable());
    assert!(matches!(
        client.resolve_stream(&track).await,
        Err(Error::NoStream)
    ));
    assert!(
        client.client_id().is_none(),
        "unsupported audio must not trigger network discovery"
    );
}

#[test]
fn converts_playlist_and_user_summaries() {
    let playlist: Playlist =
        serde_json::from_str(include_str!("../testdata/soundcloud/playlist.json")).unwrap();
    let summary = playlist.to_playlist();
    assert_eq!(summary.uri, super::super::playlist_uri(playlist.id));
    assert_eq!(summary.track_total(), playlist.track_count.unwrap());
    assert_eq!(
        summary.owner.uri,
        playlist.user.map(|user| super::super::user_uri(user.id))
    );
    let mut user: User =
        serde_json::from_str(include_str!("../testdata/soundcloud/user.json")).unwrap();
    user.avatar_url = Some("https://i1.sndcdn.com/avatars-fixture-large.jpg".into());
    let artist = user.to_artist();
    assert_eq!(artist.uri, super::super::user_uri(user.id));
    assert_eq!(
        artist.followers.unwrap().total,
        user.followers_count.unwrap()
    );
    assert_eq!(artist.external_urls.spotify, user.permalink_url);
    assert!(artist.images[0].url.contains("t500x500"));
    assert_eq!(artist.images[0].width, Some(500));
}

#[test]
fn fills_live_playlist_stubs_in_order_and_preserves_complete_entries() {
    let mut playlist: Playlist =
        serde_json::from_str(include_str!("../testdata/soundcloud/playlist.json")).unwrap();
    assert!(playlist.tracks.iter().any(Track::is_stub));
    playlist.tracks.push(
        playlist
            .tracks
            .iter()
            .find(|track| track.is_stub())
            .unwrap()
            .clone(),
    );
    let ids = playlist
        .tracks
        .iter()
        .map(|track| track.id)
        .collect::<Vec<_>>();
    let original_title = playlist.tracks[0].title.clone();
    let mut filled = playlist
        .tracks
        .iter()
        .filter(|track| track.is_stub())
        .map(|stub| Track {
            id: stub.id,
            title: Some(format!("Filled {}", stub.id)),
            ..fixture_track()
        })
        .collect::<Vec<_>>();
    filled.reverse();
    fill_stubs(&mut playlist, filled).unwrap();
    assert!(!playlist.tracks.iter().any(Track::is_stub));
    assert_eq!(
        playlist
            .tracks
            .iter()
            .map(|track| track.id)
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(playlist.tracks[0].title, original_title);
    assert!(
        playlist
            .to_tracks()
            .iter()
            .all(|track| track.album.as_ref().unwrap().name == playlist.title)
    );
}

#[test]
fn missing_batch_results_do_not_partially_mutate_a_playlist() {
    let mut playlist = Playlist {
        tracks: vec![
            Track {
                id: 1,
                ..Default::default()
            },
            Track {
                id: 2,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let error = fill_stubs(
        &mut playlist,
        vec![Track {
            id: 1,
            title: Some("One".into()),
            ..Default::default()
        }],
    )
    .unwrap_err();
    assert!(matches!(error, Error::MissingTrack(2)));
    assert!(playlist.tracks.iter().all(Track::is_stub));
}

#[test]
fn retry_after_supports_seconds_dates_and_exponential_fallback() {
    let mut headers = header::HeaderMap::new();
    assert_eq!(retry_after(&headers, 2), Duration::from_secs(4));
    headers.insert(
        header::RETRY_AFTER,
        header::HeaderValue::from_static("86400"),
    );
    assert_eq!(retry_after(&headers, 0), Duration::from_secs(86400));
    headers.insert(
        header::RETRY_AFTER,
        header::HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"),
    );
    assert_eq!(retry_after(&headers, 2), Duration::ZERO);
    let future = jiff::Timestamp::now()
        .checked_add(Duration::from_secs(600))
        .unwrap();
    let future = future
        .to_zoned(jiff::tz::TimeZone::UTC)
        .strftime("%a, %d %b %Y %H:%M:%S GMT")
        .to_string();
    headers.insert(
        header::RETRY_AFTER,
        header::HeaderValue::from_str(&future).unwrap(),
    );
    assert!((599..=600).contains(&retry_after(&headers, 0).as_secs()));
}

/// A bounded HTTP fixture, using the production transport without contacting
/// SoundCloud or touching account credentials.
async fn server(
    replies: Vec<(u16, String, &'static str)>,
) -> (Client, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut client = Client::new(
        reqwest::Client::builder().no_proxy().build().unwrap(),
        Some(OLD_ID.into()),
        Some("fixture-oauth-token".into()),
    );
    let shared = Arc::get_mut(&mut client.shared).unwrap();
    shared.api = Url::parse(&format!("http://{address}/")).unwrap();
    shared.home = shared.api.join("home").unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body, headers) in replies {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut buffer = [0; 2048];
                let count = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    break;
                }
            }
            requests.push(String::from_utf8(request).unwrap());
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        }
        requests
    });
    (client, task)
}

fn ok(body: impl Into<String>) -> (u16, String, &'static str) {
    (200, body.into(), "")
}

#[tokio::test]
async fn disconnect_revokes_old_client_clones_and_their_private_access_tokens() {
    let client = Client::new(
        Http::default(),
        Some(OLD_ID.into()),
        Some("fixture-token".into()),
    );
    client
        .access
        .secrets
        .write()
        .unwrap()
        .insert("soundcloud:track:7".into(), "s-private".into());
    let old = client.clone();
    client.revoke();
    assert!(old.access.secrets.read().unwrap().is_empty());
    assert!(matches!(old.track(7).await, Err(Error::NotSignedIn)));
    let http = crate::http::build_collection_client(&crate::settings::ProxyConfig::Off).unwrap();
    assert!(matches!(
        old.set_liked(&http, 2, 7, true).await,
        Err(Error::SessionExpired)
    ));
}

#[tokio::test]
async fn disconnect_cancels_reads_and_like_writes_waiting_for_a_request_slot() {
    use std::future::Future;
    let client = Client::new(
        Http::default(),
        Some(OLD_ID.into()),
        Some("fixture-token".into()),
    );
    let http = crate::http::build_collection_client(&crate::settings::ProxyConfig::Off).unwrap();
    let permits = client.shared.limiter.acquire_many(4).await.unwrap();
    let mut read = std::pin::pin!(client.track(7));
    let mut write = std::pin::pin!(client.set_liked(&http, 2, 7, true));
    std::future::poll_fn(|cx| {
        assert!(read.as_mut().poll(cx).is_pending());
        assert!(write.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    client.revoke();
    drop(permits);
    assert!(matches!(read.await, Err(Error::NotSignedIn)));
    assert!(matches!(write.await, Err(Error::SessionExpired)));
}

#[tokio::test]
async fn rejected_like_releases_its_request_slot_before_verifying_the_session() {
    let (client, task) = server(vec![
        (401, String::new(), ""),
        (401, String::new(), ""),
        ok("<html>Discovery unavailable</html>"),
    ])
    .await;
    let reserved = client.shared.limiter.acquire_many(3).await.unwrap();
    let http = crate::http::build_collection_client(&crate::settings::ProxyConfig::Off).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), client.set_liked(&http, 2, 7, true))
        .await
        .unwrap();
    assert!(matches!(result, Err(Error::SessionExpired)));
    drop(reserved);
    let requests = task.await.unwrap();
    assert!(requests[0].starts_with("PUT /users/2/track_likes/7?"));
    assert!(requests[1].starts_with("GET /me?"));
    assert!(requests[2].starts_with("GET /home "));
}

#[tokio::test]
async fn session_library_includes_private_owned_collected_and_liked_playlists() {
    let (client, task) = server(vec![
        ok(r#"{"collection":[{"id":7,"title":"Private owned set","public":false,"secret_token":"s-private-set"}],"next_href":"users/2/playlists?cursor=owned"}"#),
        ok(r#"{"collection":[{"id":8,"title":"Another private set","public":false}],"next_href":null}"#),
        ok(r#"{"collection":[{"id":7,"title":"Private owned set"},{"id":9,"title":"Collected set"}],"next_href":"users/2/playlists/liked_and_owned?cursor=collected"}"#),
        ok(r#"{"collection":[{"id":10,"title":"More collected"}],"next_href":null}"#),
        ok(r#"{"collection":[{"playlist":{"id":9,"title":"Collected set"}}],"next_href":"users/2/playlist_likes?cursor=liked"}"#),
        ok(r#"{"collection":[{"playlist":{"id":11,"title":"Liked set"}}],"next_href":null}"#),
    ]).await;
    let lists = crate::backend::soundcloud_library::playlists(&client, 2, true)
        .await
        .unwrap();
    assert_eq!(
        lists
            .iter()
            .map(|list| list.uri.as_str())
            .collect::<Vec<_>>(),
        [
            "soundcloud:playlist:7",
            "soundcloud:playlist:8",
            "soundcloud:playlist:9",
            "soundcloud:playlist:10",
            "soundcloud:playlist:11"
        ]
    );
    assert_eq!(lists[0].public, Some(false));
    assert!(
        !serde_json::to_string(&lists)
            .unwrap()
            .contains("s-private-set")
    );
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 6);
    for request in requests {
        assert!(request.contains("authorization: OAuth fixture-oauth-token"));
        assert!(
            !request
                .lines()
                .next()
                .unwrap()
                .contains("fixture-oauth-token")
        );
    }
}

#[tokio::test]
async fn private_track_secrets_survive_playlist_hydration_and_fresh_stream_resolution() {
    let mut raw = fixture_formats();
    raw.id = 7;
    raw.track_authorization = Some("private-track-authorization".into());
    let mut encoding = raw
        .media
        .as_ref()
        .unwrap()
        .transcodings
        .iter()
        .find(|item| item.preference().is_some())
        .unwrap()
        .clone();
    encoding.url = "media/private-track".into();
    raw.media = Some(super::super::models::Media {
        transcodings: vec![encoding],
    });
    let mut value = serde_json::to_value(&raw).unwrap();
    value["secret_token"] = json!("s-private-track");
    let (client, task) = server(vec![
        ok(r#"{"collection":[{"id":7,"title":"Private set","secret_token":"s-private-set","tracks":[{"id":7,"secret_token":"s-private-track"}]}],"next_href":null}"#),
        ok(r#"{"id":7,"title":"Private set","tracks":[{"id":7},{"id":8}]}"#),
        ok(value.to_string()),
        ok(r#"[{"id":8,"title":"Public song"}]"#),
        ok(value.to_string()),
        ok(r#"{"url":"https://cdn.soundcloud.invalid/private.mp3"}"#),
    ]).await;
    client.user_playlists(2, 100, 0).await.unwrap();
    let list = client.playlist(7).await.unwrap();
    assert_eq!(
        list.tracks.iter().map(|track| track.id).collect::<Vec<_>>(),
        [7, 8]
    );
    let track = client.track(7).await.unwrap();
    client.resolve_stream(&track).await.unwrap();
    assert!(!format!("{track:?}").contains("s-private-track"));
    assert!(
        !serde_json::to_string(&track.to_track(None))
            .unwrap()
            .contains("s-private-track")
    );
    let requests = task.await.unwrap();
    assert!(requests[1].contains("secret_token=s-private-set"));
    for index in [2, 4, 5] {
        assert!(requests[index].contains("secret_token=s-private-track"));
        assert!(!requests[index].contains("s-private-set"));
    }
    assert!(requests[5].contains("track_authorization=private-track-authorization"));
    assert!(
        requests
            .iter()
            .all(|request| request.contains("authorization: OAuth fixture-oauth-token"))
    );
}

#[tokio::test]
async fn expired_session_prompts_once_and_public_reads_continue_anonymously() {
    for status in [401, 403] {
        let (client, task) = server(vec![
            (status, String::new(), ""),
            ok("<html>Discovery unavailable</html>"),
            (status, String::new(), ""),
            ok(r#"{"id":7,"title":"Public song"}"#),
            ok(r#"{"id":8,"title":"Another public song"}"#),
        ])
        .await;
        let notifications = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&notifications);
        let client = client.on_session_expired(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let clone = client.clone();
        client.track(7).await.unwrap();
        clone.track(8).await.unwrap();
        assert!(matches!(clone.me().await, Err(Error::SessionExpired)));
        let http =
            crate::http::build_collection_client(&crate::settings::ProxyConfig::Off).unwrap();
        assert!(matches!(
            clone.set_liked(&http, 2, 7, true).await,
            Err(Error::SessionExpired)
        ));
        assert_eq!(notifications.load(Ordering::SeqCst), 1);
        let requests = task.await.unwrap();
        assert!(requests[1].starts_with("GET /home "));
        assert!(requests[2].starts_with("GET /me?"));
        for index in [0, 2] {
            assert!(requests[index].contains("authorization: OAuth fixture-oauth-token"));
        }
        for index in [1, 3, 4] {
            assert!(!requests[index].contains("authorization:"));
        }
    }
}

#[tokio::test]
async fn forbidden_private_track_does_not_expire_a_valid_session() {
    let (client, task) = server(vec![
        (403, String::new(), ""),
        ok("<html>Discovery unavailable</html>"),
        ok(r#"{"id":2,"username":"Listener"}"#),
        ok(r#"{"id":7,"title":"Accessible song"}"#),
    ])
    .await;
    assert!(matches!(client.track(9).await, Err(Error::Status(403))));
    assert!(!client.access.expired.load(Ordering::SeqCst));
    client.track(7).await.unwrap();
    let requests = task.await.unwrap();
    for index in [0, 2, 3] {
        assert!(requests[index].contains("authorization: OAuth fixture-oauth-token"));
    }
    assert!(!requests[1].contains("authorization:"));
}

#[tokio::test]
async fn private_uploads_and_likes_are_requested_with_session_authorization() {
    let (client, task) = server(vec![
        ok(r#"{"collection":[{"id":7,"title":"Private upload","secret_token":"s-upload"}],"next_href":"users/2/tracks?cursor=private"}"#),
        ok(r#"{"collection":[{"id":8,"title":"Another private upload"}],"next_href":null}"#),
        ok(r#"{"collection":[{"track":{"id":9,"title":"Private like","secret_token":"s-like"}}],"next_href":null}"#),
    ]).await;
    let uploads = client.user_tracks(2, 50, 0).await.unwrap();
    assert_eq!(uploads.collection[0].id, 7);
    let next: Page<Track> = client
        .next_page(uploads.next_href.as_deref().unwrap())
        .await
        .unwrap();
    assert_eq!(next.collection[0].id, 8);
    assert_eq!(
        client.user_likes(2, 100, 0).await.unwrap().collection[0].id,
        9
    );
    {
        let secrets = client.access.secrets.read().unwrap();
        assert_eq!(secrets["soundcloud:track:7"], "s-upload");
        assert_eq!(secrets["soundcloud:track:9"], "s-like");
    }
    let requests = task.await.unwrap();
    assert!(
        requests
            .iter()
            .all(|request| request.contains("authorization: OAuth fixture-oauth-token"))
    );
}

#[tokio::test]
async fn soundcloud_profile_verification_is_anonymous_and_session_identity_uses_the_pasted_token() {
    let user = r#"{"kind":"user","id":2,"username":"Listener","permalink_url":"https://soundcloud.com/listener"}"#;
    let (client, task) = server(vec![ok(user), ok(user)]).await;
    let profile = crate::soundcloud::account::Resolver::resolve_profile(
        &client,
        &crate::soundcloud::account::ProfileInput::Permalink("listener".into()),
    )
    .await
    .unwrap();
    assert_eq!(profile.user_id, 2);
    let token =
        crate::soundcloud::account::SessionToken::parse("2-000000-pasted_fixture_session").unwrap();
    let session = crate::soundcloud::account::Resolver::me(&client, &token)
        .await
        .unwrap();
    assert_eq!(session, profile);
    let requests = task.await.unwrap();
    assert!(requests[0].starts_with("GET /resolve?url="));
    assert!(!requests[0].to_ascii_lowercase().contains("authorization:"));
    assert!(requests[1].starts_with("GET /me?"));
    assert!(
        requests[1]
            .to_ascii_lowercase()
            .contains("authorization: oauth 2-000000-pasted_fixture_session")
    );
    assert!(
        !requests[1]
            .lines()
            .next()
            .unwrap()
            .contains("pasted_fixture_session")
    );
}

#[tokio::test]
async fn soundcloud_like_writes_are_session_only_and_never_retry_or_follow_redirects() {
    let (mut client, task) = server(vec![
        (204, String::new(), ""),
        (204, String::new(), ""),
        (
            302,
            String::new(),
            "Location: https://attacker.invalid/\r\n",
        ),
        (429, String::new(), "Retry-After: 10\r\n"),
    ])
    .await;
    let http = crate::http::build_collection_client(&crate::settings::ProxyConfig::Off).unwrap();
    client.set_liked(&http, 2, 7, true).await.unwrap();
    client.set_liked(&http, 2, 7, false).await.unwrap();
    assert!(matches!(
        client.set_liked(&http, 2, 7, true).await,
        Err(Error::Status(302))
    ));
    assert!(matches!(
        client.set_liked(&http, 2, 7, true).await,
        Err(Error::RateLimited { .. })
    ));
    assert!(matches!(
        client.set_liked(&http, 2, 7, true).await,
        Err(Error::RateLimited { .. })
    ));
    client.oauth_token = None;
    assert!(matches!(
        client.set_liked(&http, 2, 7, true).await,
        Err(Error::NotSignedIn)
    ));
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("PUT /users/2/track_likes/7?client_id="));
    assert!(requests[1].starts_with("DELETE /users/2/track_likes/7?client_id="));
    for request in requests {
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: oauth fixture-oauth-token")
        );
        assert!(
            !request
                .lines()
                .next()
                .unwrap()
                .contains("fixture-oauth-token")
        );
    }
}

#[tokio::test]
async fn soundcloud_followings_allow_only_same_origin_feed_cursors() {
    let (client, task) = server(vec![
        ok(r#"{"collection":[{"id":9,"username":"An artist"}],"next_href":null}"#),
        ok(r#"{"collection":[],"next_href":null}"#),
    ])
    .await;
    let page = client.user_followings(2, 100, 0).await.unwrap();
    assert_eq!(page.collection[0].id, 9);
    let next = client
        .shared
        .api
        .join("users/2/followings?cursor=opaque")
        .unwrap();
    let page: Page<User> = client.next_page(next.as_str()).await.unwrap();
    assert!(page.collection.is_empty());
    assert!(matches!(
        client
            .next_page::<User>("https://attacker.invalid/users/2/followings")
            .await,
        Err(Error::InvalidUrl)
    ));
    let requests = task.await.unwrap();
    assert!(
        requests[0]
            .starts_with("GET /users/2/followings?limit=100&offset=0&linked_partitioning=1&")
    );
}

#[tokio::test]
async fn search_endpoints_encode_queries_and_return_typed_pages() {
    let (client, task) = server(vec![
        ok(include_str!("../testdata/soundcloud/search_tracks.json")),
        ok(json!({"collection":[{"id":7,"title":"A set"}],"next_href":null}).to_string()),
        ok(json!({"collection":[{"id":2,"username":"Uploader"}],"next_href":null}).to_string()),
    ])
    .await;
    let tracks = client.search_tracks("lofi & jazz", 500, 3).await.unwrap();
    assert_eq!(tracks.collection.len(), 2);
    assert!(tracks.next_href.is_some());
    assert_eq!(
        client
            .search_playlists("sets", 0, 0)
            .await
            .unwrap()
            .collection[0]
            .id,
        7
    );
    assert_eq!(
        client
            .search_users("users", 10, 0)
            .await
            .unwrap()
            .collection[0]
            .id,
        2
    );
    let requests = task.await.unwrap();
    assert!(requests[0].starts_with(
        "GET /search/tracks?q=lofi+%26+jazz&limit=200&offset=3&linked_partitioning=1&"
    ));
    assert!(requests[1].starts_with("GET /search/playlists?q=sets&limit=1&"));
    assert!(requests[2].starts_with("GET /search/users?q=users&limit=10&"));
}

#[tokio::test]
async fn user_endpoints_preserve_like_cursors_and_unwrap_playlists() {
    let playlists =
        json!({"collection":[{"id":7,"title":"A set"}],"next_href":"users/2/playlists?offset=1"})
            .to_string();
    let liked = json!({"collection":[{"playlist":{"id":8,"title":"Liked set"}}],"next_href":"users/2/playlist_likes?offset=next"}).to_string();
    let (client, task) = server(vec![
        ok(include_str!("../testdata/soundcloud/user.json")),
        ok(include_str!("../testdata/soundcloud/user.json")),
        ok(include_str!("../testdata/soundcloud/track_likes.json")),
        ok(playlists.clone()),
        ok(include_str!("../testdata/soundcloud/search_tracks.json")),
        ok(liked),
        ok(include_str!("../testdata/soundcloud/playlist_likes.json")),
        ok(playlists),
    ])
    .await;
    let user = client.user(2).await.unwrap();
    assert_eq!(client.me().await.unwrap().id, user.id);
    let likes = client.user_likes(2, 1, 0).await.unwrap();
    assert_eq!(likes.collection.len(), 1);
    assert!(likes.next_href.unwrap().contains("offset=2026-10-20"));
    assert_eq!(
        client.user_playlists(2, 1, 0).await.unwrap().collection[0].id,
        7
    );
    assert_eq!(
        client.user_tracks(2, 2, 0).await.unwrap().collection.len(),
        2
    );
    let liked = client.user_liked_playlists(2, 1, 0).await.unwrap();
    assert_eq!(liked.collection[0].id, 8);
    assert!(
        client
            .next_liked_playlists(liked.next_href.as_deref().unwrap())
            .await
            .unwrap()
            .collection
            .is_empty()
    );
    assert_eq!(
        client
            .user_collection_playlists(2, 1, 0)
            .await
            .unwrap()
            .collection[0]
            .id,
        7
    );
    let requests = task.await.unwrap();
    for (request, path) in requests.iter().zip([
        "/users/2?",
        "/me?",
        "/users/2/track_likes?",
        "/users/2/playlists?",
        "/users/2/tracks?",
        "/users/2/playlist_likes?",
        "/users/2/playlist_likes?offset=next&",
        "/users/2/playlists/liked_and_owned?",
    ]) {
        assert!(request.starts_with(&format!("GET {path}")));
        assert!(request.contains("authorization: OAuth fixture-oauth-token"));
    }
}

#[tokio::test]
async fn resolve_returns_each_kind_and_fills_playlist_stubs() {
    let (client, task) = server(vec![
        ok(include_str!("../testdata/soundcloud/track.json")),
        ok(json!({"kind":"playlist","id":7,"title":"Set","tracks":[{"id":1}]}).to_string()),
        ok(json!([{"id":1,"title":"One"}]).to_string()),
        ok(include_str!("../testdata/soundcloud/user.json")),
    ])
    .await;
    assert!(matches!(
        client
            .resolve("https://soundcloud.com/artist/track")
            .await
            .unwrap(),
        Resolved::Track(_)
    ));
    let Resolved::Playlist(playlist) = client
        .resolve("https://soundcloud.com/artist/sets/set")
        .await
        .unwrap()
    else {
        panic!("expected a playlist");
    };
    assert_eq!(playlist.tracks[0].title.as_deref(), Some("One"));
    assert!(matches!(
        client
            .resolve("https://soundcloud.com/artist")
            .await
            .unwrap(),
        Resolved::User(_)
    ));
    let requests = task.await.unwrap();
    assert!(
        requests[0].starts_with("GET /resolve?url=https%3A%2F%2Fsoundcloud.com%2Fartist%2Ftrack&")
    );
    assert!(requests[2].starts_with("GET /tracks?ids=1&"));
}

#[tokio::test]
async fn refreshes_once_on_auth_failure_scanning_assets_backwards() {
    for status in [401, 403] {
        let (client, task) = server(vec![
            (status, String::new(), ""),
            ok("<script crossorigin src='/old.js'></script><script src='/new.js' crossorigin></script>"),
            ok(format!("var config={{client_id:\"{NEW_ID}\"}};")),
            ok(include_str!("../testdata/soundcloud/track.json")),
        ]).await;
        let clone = client.clone();
        client.track(1).await.unwrap();
        assert_eq!(clone.client_id().as_deref(), Some(NEW_ID));
        let requests = task.await.unwrap();
        assert!(requests[0].contains(&format!("client_id={OLD_ID}")));
        assert!(requests[0].contains("authorization: OAuth fixture-oauth-token"));
        assert!(requests[1].starts_with("GET /home "));
        assert!(!requests[1].contains("authorization:"));
        assert!(requests[2].starts_with("GET /new.js "));
        assert!(!requests[2].contains("authorization:"));
        assert!(requests[3].contains(&format!("client_id={NEW_ID}")));
    }
}

#[tokio::test]
async fn anonymous_browsing_shares_playback_discovery_without_forwarding_its_token() {
    let (client, task) = server(vec![
        (403, String::new(), ""),
        ok("<script src='/app.js'></script>"),
        ok(format!("var config={{client_id:\"{NEW_ID}\"}};")),
        ok(include_str!("../testdata/soundcloud/track.json")),
        ok(include_str!("../testdata/soundcloud/track.json")),
    ])
    .await;
    let anonymous = client.anonymous();
    anonymous.track(1).await.unwrap();
    assert_eq!(client.client_id().as_deref(), Some(NEW_ID));
    client.track(1).await.unwrap();
    let requests = task.await.unwrap();
    assert!(
        requests[..4]
            .iter()
            .all(|request| !request.contains("authorization:"))
    );
    assert!(requests[4].contains("authorization: OAuth fixture-oauth-token"));
    assert!(
        requests[4].contains(NEW_ID),
        "playback reuses the ID discovered by public browsing"
    );
}

#[tokio::test]
async fn discovery_does_not_loop_after_a_second_auth_failure() {
    let (mut client, task) = server(vec![
        (401, String::new(), ""),
        ok("<script crossorigin src='/new.js'></script>"),
        ok(format!("client_id:\"{NEW_ID}\"")),
        (403, String::new(), ""),
    ])
    .await;
    client.oauth_token = None;
    assert!(matches!(client.track(1).await, Err(Error::Status(403))));
    assert_eq!(task.await.unwrap().len(), 4);
}

#[tokio::test]
async fn skips_missing_assets_and_reports_challenge_pages_without_a_client_id() {
    let (client, task) = server(vec![
        ok("<script crossorigin src='/first.js'></script><script crossorigin src='/missing.js'></script>"),
        (404, String::new(), ""),
        ok(format!("client_id:\"{NEW_ID}\"")),
    ]).await;
    *client.shared.client_id.write().unwrap() = None;
    assert_eq!(client.discover_client_id().await.unwrap(), NEW_ID);
    let requests = task.await.unwrap();
    assert!(requests[1].starts_with("GET /missing.js "));
    assert!(requests[2].starts_with("GET /first.js "));
    let (client, task) = server(vec![ok("<html>A web challenge</html>")]).await;
    *client.shared.client_id.write().unwrap() = None;
    assert!(matches!(
        client.discover_client_id().await,
        Err(Error::ClientIdUnavailable)
    ));
    assert!(client.client_id().is_none());
    task.await.unwrap();
}

#[tokio::test]
async fn batches_at_most_fifty_ids_and_restores_request_order() {
    let ids = (1..=53).chain([1]).collect::<Vec<_>>();
    let first = (1..=50)
        .rev()
        .map(|id| json!({"id":id,"title":format!("Track {id}")}))
        .collect::<Vec<_>>();
    let second = [1, 53, 52, 51].map(|id| json!({"id":id,"title":format!("Track {id}")}));
    let (client, task) = server(vec![
        ok(serde_json::to_string(&first).unwrap()),
        ok(serde_json::to_string(&second).unwrap()),
    ])
    .await;
    let tracks = client.tracks(&ids).await.unwrap();
    assert_eq!(tracks.iter().map(|track| track.id).collect::<Vec<_>>(), ids);
    let requests = task.await.unwrap();
    for (request, count) in requests.iter().zip([50, 4]) {
        let target = request
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        let url = client.shared.api.join(target).unwrap();
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key == "ids")
                .unwrap()
                .1
                .split(',')
                .count(),
            count
        );
    }
}

#[tokio::test]
async fn playlist_local_copy_exposes_placeholders_then_fills_every_batch_and_duplicate() {
    let ids: Vec<u64> = (1..=121).chain([1]).collect();
    let mut rows: Vec<_> = ids.iter().map(|id| json!({"id":id})).collect();
    rows.insert(7, json!({"id":1000,"title":"Already complete"}));
    let mut replies = vec![ok(
        json!({"id":7,"title":"A large set","tracks":rows}).to_string()
    )];
    for chunk in ids.chunks(50) {
        replies.push(ok(serde_json::to_string(
            &chunk
                .iter()
                .rev()
                .map(|id| json!({"id":id,"title":format!("Track {id}")}))
                .collect::<Vec<_>>(),
        )
        .unwrap()));
    }
    let (client, task) = server(replies).await;
    let mut playlist = client.playlist_snapshot(7).await.unwrap();
    let provisional = crate::soundcloud::browse::playlist_add_rows(&playlist);
    assert_eq!(provisional.len(), 123);
    assert_eq!(provisional[7].name, "Already complete");
    assert_eq!(provisional[0].name, "Loading SoundCloud track…");
    assert_eq!(provisional[0].is_playable, Some(false));
    client.fill_playlist(&mut playlist).await.unwrap();
    let filled = playlist.to_tracks();
    assert_eq!(filled.len(), provisional.len());
    assert_eq!(
        filled.iter().map(|row| &row.uri).collect::<Vec<_>>(),
        provisional.iter().map(|row| &row.uri).collect::<Vec<_>>()
    );
    assert_eq!(filled[7].name, "Already complete");
    assert_eq!(filled[0].name, "Track 1");
    assert_eq!(filled[122].name, "Track 1");
    assert!(
        filled
            .iter()
            .all(|track| track.album.as_ref().unwrap().name == "A large set")
    );
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("GET /playlists/7?"));
    for (request, chunk) in requests[1..].iter().zip(ids.chunks(50)) {
        let query = Url::parse(&format!(
            "http://test{}",
            request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
        ))
        .unwrap();
        assert_eq!(
            query.query_pairs().find(|(key, _)| key == "ids").unwrap().1,
            chunk
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
    }
}

#[tokio::test]
async fn playlist_endpoint_hydrates_stubs_before_returning() {
    let (client, task) = server(vec![
        ok(json!({"id":7,"title":"A set","tracks":[{"id":3},{"id":1,"title":"Already complete"},{"id":2},{"id":3}]}).to_string()),
        ok(json!([{"id":2,"title":"Two"},{"id":3,"title":"Three"}]).to_string()),
    ]).await;
    let playlist = client.playlist(7).await.unwrap();
    assert_eq!(
        playlist
            .tracks
            .iter()
            .map(|track| track.id)
            .collect::<Vec<_>>(),
        [3, 1, 2, 3]
    );
    assert!(!playlist.tracks.iter().any(Track::is_stub));
    assert_eq!(
        playlist.tracks[1].title.as_deref(),
        Some("Already complete")
    );
    let requests = task.await.unwrap();
    assert!(requests[0].starts_with("GET /playlists/7?"));
    assert!(requests[1].starts_with("GET /tracks?ids=3%2C2%2C3&"));
}

#[tokio::test]
async fn anonymous_client_omits_oauth_and_reuses_cached_discovery() {
    let (mut client, task) = server(vec![
        ok("<script crossorigin src='/asset.js'></script>"),
        ok(format!("client_id:\"{NEW_ID}\"")),
        ok(include_str!("../testdata/soundcloud/track.json")),
        ok(include_str!("../testdata/soundcloud/track.json")),
    ])
    .await;
    client.oauth_token = None;
    *client.shared.client_id.write().unwrap() = None;
    let clone = client.clone();
    let (first, second) = tokio::join!(client.track(1), clone.track(2));
    first.unwrap();
    second.unwrap();
    assert_eq!(client.client_id().as_deref(), Some(NEW_ID));
    let requests = task.await.unwrap();
    assert!(
        requests
            .iter()
            .all(|request| !request.contains("authorization:"))
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("GET /home "))
            .count(),
        1
    );
}

#[tokio::test]
async fn pagination_replaces_stale_client_ids_and_maps_like_wrappers() {
    let (client, task) = server(vec![ok(include_str!("../testdata/soundcloud/search_tracks.json")), ok(json!({"collection":[{"created_at":"2026-10-28T00:00:00Z","track":{"id":9,"title":"Liked"}}],"next_href":null}).to_string())]).await;
    let next = client
        .shared
        .api
        .join(&format!(
            "search/tracks?client_id={NEW_ID}&offset=2&limit=2"
        ))
        .unwrap();
    let page: Page<Track> = client.next_page(next.as_str()).await.unwrap();
    assert_eq!(page.collection.len(), 2);
    assert!(page.next_href.is_some());
    let page = client
        .next_likes(
            client
                .shared
                .api
                .join("users/2/track_likes?offset=1")
                .unwrap()
                .as_str(),
        )
        .await
        .unwrap();
    assert_eq!(page.collection[0].id, 9);
    let requests = task.await.unwrap();
    assert!(!requests[0].contains(NEW_ID));
    assert_eq!(requests[0].matches("client_id=").count(), 1);
    assert!(requests[0].contains("offset=2"));
}

#[tokio::test]
async fn resolves_stream_with_auth_and_falls_back_from_a_removed_format() {
    let (client, task) = server(vec![
        (404, String::new(), ""),
        ok(r#"{"url":"https://playback.media-streaming.soundcloud.cloud/audio/playlist.m3u8"}"#),
    ])
    .await;
    let mut track = fixture_formats();
    track.track_authorization = Some("fixture-track-authorization".into());
    for item in &mut track.media.as_mut().unwrap().transcodings {
        item.url = client
            .shared
            .api
            .join(Url::parse(&item.url).unwrap().path())
            .unwrap()
            .to_string();
    }
    let info = client.resolve_stream(&track).await.unwrap();
    assert_eq!(info.protocol, Protocol::Hls);
    assert_eq!(info.codec, "aac");
    assert!(!info.is_preview);
    let requests = task.await.unwrap();
    assert!(requests[0].contains("/media/progressive-mp3?"));
    assert!(requests[1].contains("/media/aac-160?"));
    assert!(requests.iter().all(|request| {
        request.contains("track_authorization=fixture-track-authorization")
            && request.contains(OLD_ID)
    }));
}

#[tokio::test]
async fn retries_429_and_keeps_long_cooldowns_shared_between_clones() {
    let (client, task) = server(vec![
        (429, String::new(), "Retry-After: 0\r\n"),
        ok(include_str!("../testdata/soundcloud/track.json")),
    ])
    .await;
    client.track(1).await.unwrap();
    assert_eq!(task.await.unwrap().len(), 2);
    let (client, task) = server(vec![(429, String::new(), "Retry-After: 86400\r\n")]).await;
    assert!(
        matches!(client.track(1).await, Err(Error::RateLimited { retry_after }) if retry_after == Duration::from_secs(86400))
    );
    task.await.unwrap();
    assert!(
        matches!(client.clone().track(2).await, Err(Error::RateLimited { retry_after }) if retry_after > Duration::from_secs(86390))
    );
}

#[tokio::test]
async fn waits_for_retry_after_and_bounds_the_number_of_retries() {
    let (client, task) = server(vec![
        (429, String::new(), "Retry-After: 1\r\n"),
        ok(include_str!("../testdata/soundcloud/track.json")),
    ])
    .await;
    let started = Instant::now();
    client.track(1).await.unwrap();
    assert!(started.elapsed() >= Duration::from_secs(1));
    task.await.unwrap();
    let (client, task) = server(
        (0..4)
            .map(|_| (429, String::new(), "Retry-After: 0\r\n"))
            .collect(),
    )
    .await;
    assert!(matches!(
        client.track(1).await,
        Err(Error::RateLimited { .. })
    ));
    assert_eq!(task.await.unwrap().len(), 4);
}

#[test]
fn debug_output_omits_playback_credentials_and_signed_urls() {
    let mut track = fixture_formats();
    track.track_authorization = Some("private-playback-authorization".into());
    assert!(!format!("{track:?}").contains("private-playback-authorization"));
    let stream = stream_info(
        &track,
        &track.media.as_ref().unwrap().transcodings[4],
        "https://cdn.invalid/audio?secret=private-signed-query".into(),
    );
    assert!(!format!("{stream:?}").contains("private-signed-query"));
}

#[tokio::test]
async fn rejects_foreign_pagination_urls_and_me_without_oauth() {
    let client = Client::new(Http::default(), Some(OLD_ID.into()), None);
    assert!(matches!(
        client
            .next_page::<Track>("https://attacker.invalid/tracks")
            .await,
        Err(Error::InvalidUrl)
    ));
    assert!(matches!(
        client
            .resolve("https://soundcloud.com.attacker.invalid/track")
            .await,
        Err(Error::InvalidUrl)
    ));
    assert!(matches!(client.me().await, Err(Error::NotSignedIn)));
    assert!(matches!(
        client.user_collection_playlists(1, 10, 0).await,
        Err(Error::NotSignedIn)
    ));
    for path in ["ads", "users/1/ads", "search/ads"] {
        assert!(matches!(
            client.next_page::<Track>(path).await,
            Err(Error::InvalidUrl)
        ));
    }
    let mut track = fixture_formats();
    for item in &mut track.media.as_mut().unwrap().transcodings {
        item.url = "https://api-v2.soundcloud.com/ads".into();
    }
    assert!(matches!(
        client.resolve_stream(&track).await,
        Err(Error::InvalidUrl)
    ));
}

#[tokio::test]
#[ignore = "uses SoundCloud's live anonymous API and stream resolver"]
async fn live_search_lofi_and_resolve_a_stream() {
    let client = Client::new(Http::default(), None, None);
    let page = client.search_tracks("lofi", 10, 0).await.unwrap();
    let track = page
        .collection
        .iter()
        .find(|track| track.is_playable())
        .expect("a playable lofi result");
    let stream = client.resolve_stream(track).await.unwrap();
    assert!(stream.url.starts_with("https://"));
    assert!(client.client_id().is_some());
}

#[tokio::test]
async fn browse_search_runs_all_sections_and_pages_the_selected_cursor() {
    use crate::model::SoundCloudSearchKind;
    use crate::soundcloud::browse::{self, SearchRequest};
    let body = json!({"collection":[{"id":7,"title":"A track or set","username":"Uploader"}],"next_href":"search/tracks?cursor=one"}).to_string();
    let (client, task) = server(vec![ok(body.clone()), ok(body.clone()), ok(body)]).await;
    let mut request = SearchRequest {
        query: "ambient".into(),
        serial: 3,
        request: 4,
        more: None,
    };
    let results = browse::search(&client, &request).await;
    assert_eq!(
        (
            results.tracks.len(),
            results.playlists.len(),
            results.users.len()
        ),
        (1, 1, 1)
    );
    assert!(results.errors.is_empty());
    let requests = task.await.unwrap();
    for kind in ["tracks", "playlists", "users"] {
        assert!(requests.iter().any(|line| {
            line.starts_with(&format!("GET /search/{kind}?q=ambient&limit=20&offset=0&"))
        }));
    }
    let (client, task) = server(vec![ok(
        json!({"collection":[{"id":8,"title":"Next"}],"next_href":null}).to_string(),
    )])
    .await;
    request.more = Some((
        SoundCloudSearchKind::Tracks,
        results.next[0].clone().unwrap(),
    ));
    let page = browse::search(&client, &request).await;
    assert_eq!(page.tracks[0].uri, "soundcloud:track:8");
    assert!(page.playlists.is_empty() && page.users.is_empty());
    assert!(task.await.unwrap()[0].starts_with("GET /search/tracks?cursor=one&"));
}

#[tokio::test]
async fn browse_url_search_resolves_instead_of_searching_the_url() {
    use crate::soundcloud::browse::{self, SearchRequest};
    let body = json!({"kind":"user","id":9,"username":"Linked uploader"}).to_string();
    let (client, task) = server(vec![ok(body)]).await;
    let request = SearchRequest {
        query: "https://soundcloud.com/uploader".into(),
        serial: 1,
        request: 1,
        more: None,
    };
    let results = browse::search(&client, &request).await;
    assert_eq!(results.resolved.unwrap().users[0].id, 9);
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /resolve?"));
}

#[tokio::test]
async fn browse_search_keeps_successful_sections_when_one_fails() {
    use crate::soundcloud::browse::{self, SearchRequest};
    let body =
        json!({"collection":[{"id":7,"title":"Result","username":"Uploader"}],"next_href":null})
            .to_string();
    let (client, task) = server(vec![(500, "{}".into(), ""), ok(body.clone()), ok(body)]).await;
    let results = browse::search(
        &client,
        &SearchRequest {
            query: "ambient".into(),
            serial: 1,
            request: 1,
            more: None,
        },
    )
    .await;
    assert_eq!(results.errors.len(), 1);
    assert_eq!(
        results.tracks.len() + results.playlists.len() + results.users.len(),
        2
    );
    assert_eq!(task.await.unwrap().len(), 3);
}
