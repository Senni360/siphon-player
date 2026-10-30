use super::*;

pub(super) fn fixture() -> Video {
    serde_json::from_str(include_str!("../testdata/youtube/video.json")).unwrap()
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("youtube-fixture-{:016x}", rand::random::<u64>()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn links_are_canonical_and_never_accept_arbitrary_hosts_or_commands() {
    for link in [
        "https://youtube.com/watch?v=aB9_-012xyz&list=ignored",
        "https://www.youtube.com/watch?v=aB9_-012xyz&t=42",
        "https://m.youtube.com/watch?v=aB9_-012xyz",
        "https://music.youtube.com/watch?v=aB9_-012xyz",
        "https://youtu.be/aB9_-012xyz?si=ignored",
        "https://youtube.com/shorts/aB9_-012xyz/",
        "youtube:video:aB9_-012xyz",
    ] {
        assert_eq!(parse(link).as_deref(), Some("youtube:video:aB9_-012xyz"));
        assert_eq!(crate::link::parse(link), parse(link));
        assert_eq!(crate::link::playlist_item(link), parse(link));
    }
    for link in [
        "https://youtube.com.attacker.test/watch?v=aB9_-012xyz",
        "https://youtube.com@attacker.test/watch?v=aB9_-012xyz",
        "https://user:pass@youtube.com/watch?v=aB9_-012xyz",
        "https://youtube.com:444/watch?v=aB9_-012xyz",
        "file:///youtube.com/watch?v=aB9_-012xyz",
        "https://youtu.be/aB9_-012xyz/extra",
        "https://youtube.com/playlist?list=aB9_-012xyz",
        "youtube:video:--exec=x",
        "https://youtube.com/watch?v=aB9_-012xy%0A",
    ] {
        assert!(parse(link).is_none(), "{link}");
    }
}

#[test]
fn metadata_retains_source_provenance_and_real_optional_fields() {
    let track = fixture().to_track().unwrap();
    assert_eq!(track.uri, "youtube:video:aB9_-012xyz");
    assert_eq!(track.artists[0].name, "Fixture Producer");
    assert_eq!(track.duration_ms, 182_500);
    assert_eq!(track.release_date.as_deref(), Some("2026-10-23"));
    assert_eq!(track.play_count, Some(1204));
    assert_eq!(track.album.unwrap().images[0].width, Some(480));
    assert_eq!(
        crate::soundcloud::Source::of(&track.uri),
        crate::soundcloud::Source::YouTube
    );
    assert_eq!(crate::util::uri_kind(&track.uri), None);
    assert!(!is_external("spotify:track:a"));
    assert!(is_external("soundcloud:track:7"));
}

#[test]
fn unsupported_and_missing_metadata_are_honest() {
    let mut video = fixture();
    video.is_live = true;
    assert_eq!(video.to_track(), Err(Error::Unavailable));
    video.is_live = false;
    video.availability = Some("private".into());
    assert_eq!(video.to_track(), Err(Error::SignIn));
    video.availability = Some("unlisted".into());
    video.upload_date = Some("20260231".into());
    video.view_count = None;
    video.duration = Some(f64::NAN);
    video.thumbnails[0].url = "https://attacker.test/image".into();
    video.thumbnails.truncate(1);
    let track = video.to_track().unwrap();
    assert!(track.release_date.is_none() && track.play_count.is_none());
    assert_eq!(track.duration_ms, 0);
    assert!(track.album.unwrap().images.is_empty());
}

#[test]
fn expired_streams_are_not_reused() {
    assert!(url_expired(
        "https://r1.googlevideo.com/videoplayback?expire=1"
    ));
    assert!(!url_expired(
        "https://r1.googlevideo.com/videoplayback?expire=99999999999"
    ));
    assert!(!url_expired("https://cdn.example/audio"));
}

#[tokio::test]
async fn cached_metadata_works_without_a_helper_and_never_contains_audio_urls() {
    let scratch = Scratch::new();
    let video = fixture();
    let path = scratch.0.join(format!("{}.json", video.id));
    std::fs::write(path, serde_json::to_vec(&video).unwrap()).unwrap();
    let catalogue = Catalogue {
        resolver: Resolver::default(),
        cache: scratch.0.clone(),
    };
    let track = catalogue.track("youtube:video:aB9_-012xyz").await.unwrap();
    assert_eq!(track.name, video.title);
    let text = serde_json::to_string(&track).unwrap();
    assert!(!text.contains("googlevideo.com") && !text.contains("expire="));
}

#[test]
fn local_likes_are_account_scoped_versioned_and_preserve_unreadable_state() {
    let scratch = Scratch::new();
    let path = likes::path(&scratch.0, "alice");
    assert_ne!(path, likes::path(&scratch.0, "bob"));
    assert_eq!(
        likes::path(&scratch.0, "../../escape").parent(),
        path.parent()
    );
    let liked = fixture().to_track().unwrap();
    let saved = likes::Likes {
        tracks: vec![liked.clone(), liked],
        ..Default::default()
    };
    fastframe_file::write_private(&path, saved.encode().unwrap()).unwrap();
    assert_eq!(likes::Likes::load(&path).unwrap().tracks.len(), 1);
    assert!(
        likes::Likes::load(&likes::path(&scratch.0, "bob"))
            .unwrap()
            .tracks
            .is_empty()
    );
    let future = br#"{"version":2,"tracks":[]}"#;
    std::fs::write(&path, future).unwrap();
    assert!(likes::Likes::load(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), future);
}

#[test]
fn youtube_downloads_keep_the_selected_video_instead_of_matching_spotify() {
    let item = crate::api::models::PlayableItem::Track(fixture().to_track().unwrap());
    let source = crate::downloads::Source::from_item(&item).unwrap();
    assert!(
        matches!(source, crate::downloads::Source::YouTube { url, .. } if url == "https://www.youtube.com/watch?v=aB9_-012xyz")
    );
}
