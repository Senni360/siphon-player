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
fn cached_metadata_read_is_bounded_when_file_grows_after_metadata_check() {
    use std::io::{Seek, Write};

    let scratch = Scratch::new();
    let video = fixture();
    let path = scratch.0.join(format!("{}.json", video.id));
    let mut bytes = serde_json::to_vec(&video).unwrap();
    // Trailing JSON whitespace is valid, including at the exact size limit.
    bytes.resize(MAX_METADATA_BYTES as usize, b' ');
    std::fs::write(&path, &bytes).unwrap();
    let mut file = std::fs::File::open(&path).unwrap();
    assert_eq!(file.metadata().unwrap().len(), MAX_METADATA_BYTES);
    assert_eq!(read_cached_video(&mut file).unwrap().id, video.id);
    file.rewind().unwrap();

    // Deterministically grow the already-open file after checking its size.
    // This reproduces the race boundary without concurrent scheduling.
    let mut append = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    append.write_all(&[b' '; 128]).unwrap();
    drop(append);
    assert!(read_cached_video(&mut file).is_none());
    assert_eq!(file.stream_position().unwrap(), MAX_METADATA_BYTES + 1);
    assert_eq!(file.metadata().unwrap().len(), MAX_METADATA_BYTES + 128);
    // Ignoring an oversized cache does not truncate or rewrite its contents.
    assert_eq!(std::fs::read(&path).unwrap().len(), bytes.len() + 128);
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
fn local_likes_reject_partial_or_invalid_files_without_overwriting_them() {
    let scratch = Scratch::new();
    let path = likes::path(&scratch.0, "alice");
    for damaged in [
        "{",
        "{}",
        r#"{"version":1}"#,
        r#"{"version":1,"tracks":null}"#,
        r#"{"version":1,"tracks":[{"uri":"spotify:track:wrong-owner"}]}"#,
        r#"{"version":1,"tracks":[{"uri":"youtube:video:invalid"}]}"#,
    ] {
        fastframe_file::write_private(&path, damaged).unwrap();
        assert!(likes::Likes::load(&path).is_err(), "{damaged}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), damaged);
    }
    // The version-1 shape and its older omitted-version spelling
    // remain readable. Missing tracks is never interpreted as an empty file.
    fastframe_file::write_private(&path, br#"{"tracks":[]}"#).unwrap();
    assert_eq!(likes::Likes::load(&path).unwrap(), likes::Likes::default());

    let mut oversized = fixture().to_track().unwrap();
    oversized.name = "x".repeat(16 * 1024 * 1024);
    let saved = likes::Likes {
        tracks: vec![oversized],
        ..Default::default()
    };
    assert!(
        saved.encode().is_err(),
        "do not save a file that cannot be restored"
    );
    let bytes = serde_json::to_vec(&saved).unwrap();
    fastframe_file::write_private(&path, &bytes).unwrap();
    assert!(likes::Likes::load(&path).is_err());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), bytes.len() as u64);
}

#[test]
fn local_likes_restore_waits_for_delayed_saves_and_keeps_accounts_separate() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    use std::time::Duration;

    fn send_sync<T: Send + Sync>() {}
    send_sync::<fastframe_file::Writer>();
    let scratch = Scratch::new();
    let alice = likes::path(&scratch.0, "alice");
    let bob = likes::path(&scratch.0, "bob");
    fastframe_file::write_private(&alice, likes::Likes::default().encode().unwrap()).unwrap();
    let (started, start) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let first = AtomicBool::new(true);
    let writer = Arc::new(fastframe_file::Writer::with_write(
        move |path, bytes, access| {
            if first.swap(false, Ordering::SeqCst) {
                started.send(()).unwrap();
                gate.lock().unwrap().recv().unwrap();
            }
            fastframe_file::write_with(path, access, |file| std::io::Write::write_all(file, bytes))
        },
    ));
    let writes = Arc::new(likes::Writes::default());
    writes.begin("alice");
    writer.write(
        &alice,
        likes::Likes::default().encode().unwrap(),
        fastframe_file::Access::Private,
    );
    start.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut track = fixture().to_track().unwrap();
    track.name = "Latest Alice edit".into();
    let latest = likes::Likes {
        tracks: vec![track],
        ..Default::default()
    };
    writes.begin("alice");
    writer.write(
        &alice,
        latest.encode().unwrap(),
        fastframe_file::Access::Private,
    );
    writes.begin("bob");
    writer.write(
        &bob,
        likes::Likes::default().encode().unwrap(),
        fastframe_file::Access::Private,
    );

    let (reading, read_started) = mpsc::channel();
    let (finished, result) = mpsc::channel();
    let read_writer = writer.clone();
    let read_writes = writes.clone();
    let read_path = alice.clone();
    let reader = std::thread::spawn(move || {
        reading.send(()).unwrap();
        finished
            .send(read_writes.load_after_writes(&read_writer, &read_path, "alice"))
            .unwrap();
    });
    read_started.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        result.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    release.send(()).unwrap();
    let restored = result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    reader.join().unwrap();
    assert_eq!(restored.likes, latest);
    assert!(!restored.missed_save);
    assert!(
        writes
            .load_after_writes(&writer, &bob, "bob")
            .unwrap()
            .likes
            .tracks
            .is_empty()
    );
    // A fresh process restores exactly the durable account-specific snapshot.
    assert_eq!(
        likes::Writes::default()
            .load_after_writes(&writer, &alice, "alice")
            .unwrap()
            .likes,
        latest
    );
}

#[test]
fn local_likes_interrupted_atomic_save_keeps_old_file_and_retry_recovers() {
    use std::sync::Arc;

    let scratch = Scratch::new();
    let path = likes::path(&scratch.0, "alice");
    let before = likes::Likes {
        tracks: vec![fixture().to_track().unwrap()],
        ..Default::default()
    };
    let before_bytes = before.encode().unwrap();
    fastframe_file::write_private(&path, &before_bytes).unwrap();
    let writes = Arc::new(likes::Writes::default());
    let writer = fastframe_file::Writer::with_write(|path, _, access| {
        fastframe_file::write_with(path, access, |file| {
            std::io::Write::write_all(file, b"{\"version\":")?;
            Err(std::io::Error::other("injected interrupted write"))
        })
    });
    let revision = writes.begin("alice");
    let failure = writes.clone();
    writer.write_or(
        &path,
        likes::Likes::default().encode().unwrap(),
        fastframe_file::Access::Private,
        move || {
            assert!(failure.failed("alice", revision));
        },
    );
    let restored = writes.load_after_writes(&writer, &path, "alice").unwrap();
    assert!(restored.missed_save);
    assert_eq!(restored.likes, before);
    assert_eq!(std::fs::read(&path).unwrap(), before_bytes);
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1,
        "failed temporary file is removed"
    );

    let retry = fastframe_file::Writer::new();
    writes.begin("alice");
    retry.write(
        &path,
        likes::Likes::default().encode().unwrap(),
        fastframe_file::Access::Private,
    );
    let restored = writes.load_after_writes(&retry, &path, "alice").unwrap();
    assert!(!restored.missed_save);
    assert!(restored.likes.tracks.is_empty());
}

#[test]
fn local_likes_old_failure_does_not_poison_a_newer_save() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    use std::time::Duration;

    let scratch = Scratch::new();
    let path = likes::path(&scratch.0, "alice");
    let (started, start) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let first = AtomicBool::new(true);
    let writer = fastframe_file::Writer::with_write(move |path, bytes, access| {
        if first.swap(false, Ordering::SeqCst) {
            started.send(()).unwrap();
            gate.lock().unwrap().recv().unwrap();
            return Err(std::io::Error::other("injected superseded save failure"));
        }
        fastframe_file::write_with(path, access, |file| std::io::Write::write_all(file, bytes))
    });
    let writes = Arc::new(likes::Writes::default());
    let first_revision = writes.begin("alice");
    let failure = writes.clone();
    writer.write_or(
        &path,
        likes::Likes::default().encode().unwrap(),
        fastframe_file::Access::Private,
        move || {
            assert!(!failure.failed("alice", first_revision));
        },
    );
    start.recv_timeout(Duration::from_secs(5)).unwrap();
    let latest = likes::Likes {
        tracks: vec![fixture().to_track().unwrap()],
        ..Default::default()
    };
    writes.begin("alice");
    writer.write(
        &path,
        latest.encode().unwrap(),
        fastframe_file::Access::Private,
    );
    release.send(()).unwrap();
    let restored = writes.load_after_writes(&writer, &path, "alice").unwrap();
    assert!(!restored.missed_save);
    assert_eq!(restored.likes, latest);
}

#[test]
fn youtube_downloads_keep_the_selected_video_instead_of_matching_spotify() {
    let item = crate::api::models::PlayableItem::Track(fixture().to_track().unwrap());
    let source = crate::downloads::Source::from_item(&item).unwrap();
    assert!(
        matches!(source, crate::downloads::Source::YouTube { url, .. } if url == "https://www.youtube.com/watch?v=aB9_-012xyz")
    );
}
