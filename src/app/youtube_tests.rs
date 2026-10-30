// Included in app::tests so these exercise the existing offline app harness.
fn youtube_fixture_track() -> Track {
    let video: crate::youtube::Video =
        serde_json::from_str(include_str!("../testdata/youtube/video.json")).unwrap();
    video.to_track().unwrap()
}

#[test]
fn youtube_mixed_queue_keeps_duplicates_transport_and_paused_restore() {
    use crate::player::HybridEvent;
    let mut app = hybrid_fixture();
    let ctx = egui::Context::default();
    let track = youtube_fixture_track();
    app.cache_soundcloud_tracks(std::slice::from_ref(&track));
    app.apply(
        Action::PlayUris {
            uris: vec![
                "spotify:track:a".into(),
                "soundcloud:track:7".into(),
                track.uri.clone(),
                track.uri.clone(),
            ],
            index: 0,
        },
        &ctx,
    );
    let spotify = app.hybrid_tag().unwrap();
    app.apply(Action::Next, &ctx);
    assert_eq!(
        app.current_track_uri().as_deref(),
        Some("soundcloud:track:7")
    );
    app.apply(Action::Next, &ctx);
    let youtube = app.hybrid_tag().unwrap();
    assert_eq!(app.current_track_uri().as_deref(), Some(track.uri.as_str()));
    assert!(!app.can_use_spotify_outputs());
    app.handle_hybrid_event(spotify, HybridEvent::EndOfTrack);
    assert_eq!(app.hybrid_tag(), Some(youtube));
    app.apply(Action::Seek(18_000), &ctx);
    app.apply(Action::TogglePlay, &ctx);
    let before = queue_uris(&app);
    let encoded = SessionState {
        hybrid: app.hybrid_snapshot(),
        ..Default::default()
    }
    .encode()
    .unwrap();
    let decoded: SessionState = serde_json::from_str(&encoded).unwrap();
    let mut restored = hybrid_fixture();
    restored.restore_hybrid(decoded.hybrid);
    assert_eq!(
        restored.current_track_uri().as_deref(),
        Some(track.uri.as_str())
    );
    assert!(!restored.now_playing().unwrap().playing);
    assert_eq!(restored.now_playing().unwrap().position_ms, 18_000);
    assert_eq!(queue_uris(&restored), before);
    app.apply(Action::Next, &ctx);
    assert_eq!(app.current_track_uri().as_deref(), Some(track.uri.as_str()));
    assert_ne!(app.hybrid_tag(), Some(youtube));
    app.backend.shutdown();
    restored.backend.shutdown();
}

#[test]
fn youtube_likes_refresh_immediately_and_old_account_results_are_ignored() {
    let mut app = hybrid_fixture();
    app.user = Some(User {
        id: "alice".into(),
        ..Default::default()
    });
    app.request_youtube_likes();
    let generation = app.youtube.generation;
    app.youtube_likes_received("alice", generation, Ok(Default::default()));
    let track = youtube_fixture_track();
    app.cache_soundcloud_tracks(std::slice::from_ref(&track));
    app.toggle_youtube_like(&track.uri);
    assert_eq!(app.is_saved(&track.uri), Some(true));
    assert_eq!(
        app.soundcloud_pages["youtube:likes"]
            .content
            .get()
            .unwrap()
            .tracks,
        vec![track.clone()]
    );
    app.toggle_youtube_like(&track.uri);
    assert_eq!(app.is_saved(&track.uri), Some(false));
    assert!(
        app.soundcloud_pages["youtube:likes"]
            .content
            .get()
            .unwrap()
            .tracks
            .is_empty()
    );
    app.reset_youtube();
    app.user = Some(User {
        id: "bob".into(),
        ..Default::default()
    });
    app.request_youtube_likes();
    app.youtube_likes_received(
        "alice",
        generation,
        Ok(crate::youtube::likes::Likes {
            tracks: vec![track.clone()],
            ..Default::default()
        }
        .into()),
    );
    app.youtube_track_received(generation, &track.uri, Ok(Box::new(track.clone())));
    assert!(app.youtube_likes().is_none());
    assert!(!app.track_cache.contains_key(&track.uri));
    app.youtube_likes_received("bob", app.youtube.generation, Ok(Default::default()));
    assert!(app.youtube_likes().unwrap().is_empty());
    app.backend.shutdown();
}

#[test]
fn youtube_likes_duplicate_load_and_round_trip_failure_cannot_undo_current_edits() {
    let mut app = hybrid_fixture();
    let track = youtube_fixture_track();
    app.user = Some(User {
        id: "alice".into(),
        ..Default::default()
    });
    app.request_youtube_likes();
    let old_generation = app.youtube.generation;
    app.youtube_likes_received("alice", old_generation, Ok(Default::default()));
    app.cache_soundcloud_tracks(std::slice::from_ref(&track));
    app.toggle_youtube_like(&track.uri);
    app.youtube_likes_received("alice", old_generation, Ok(Default::default()));
    assert_eq!(app.youtube_likes().unwrap(), std::slice::from_ref(&track));

    app.user = Some(User {
        id: "bob".into(),
        ..Default::default()
    });
    app.request_youtube_likes();
    assert!(app.youtube_likes().is_none());
    assert!(!app.track_cache.contains_key(&track.uri));
    assert!(!app.soundcloud_pages.contains_key("youtube:likes"));
    app.youtube_likes_received("bob", app.youtube.generation, Ok(Default::default()));
    app.user = Some(User {
        id: "alice".into(),
        ..Default::default()
    });
    app.request_youtube_likes();
    let generation = app.youtube.generation;
    assert_ne!(generation, old_generation);
    app.youtube_likes_received(
        "alice",
        generation,
        Ok(crate::youtube::likes::Restored {
            likes: crate::youtube::likes::Likes {
                tracks: vec![track.clone()],
                ..Default::default()
            },
            missed_save: true,
        }),
    );
    assert_eq!(
        app.soundcloud_pages["youtube:likes"].error.as_deref(),
        Some(crate::youtube::likes::RESTORE_WARNING)
    );
    app.youtube.write_revision = 9;
    // Accept synthetic local events while the backend remains network-offline.
    // Demo-mode App filtering would otherwise drop every tested callback.
    app.offline = false;
    app.handle_backend_events(vec![
        Event::YouTubeLikesWriteFailed {
            account: "alice".into(),
            generation: old_generation,
            revision: 9,
        },
        Event::YouTubeLikesWriteFailed {
            account: "bob".into(),
            generation,
            revision: 9,
        },
        Event::YouTubeLikesWriteFailed {
            account: "alice".into(),
            generation,
            revision: 8,
        },
    ]);
    assert_eq!(
        app.soundcloud_pages["youtube:likes"].error.as_deref(),
        Some(crate::youtube::likes::RESTORE_WARNING)
    );
    assert_eq!(app.youtube_likes().unwrap(), std::slice::from_ref(&track));
    app.handle_backend_events(vec![Event::YouTubeLikesWriteFailed {
        account: "alice".into(),
        generation,
        revision: 9,
    }]);
    assert_eq!(
        app.soundcloud_pages["youtube:likes"].error.as_deref(),
        Some(crate::youtube::likes::SAVE_ERROR)
    );
    assert_eq!(app.youtube_likes().unwrap(), std::slice::from_ref(&track));
    app.backend.shutdown();
}

#[test]
fn youtube_likes_failed_save_keeps_optimistic_rows_and_reopening_retries_them() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let mut app = hybrid_fixture();
    let scratch = std::env::current_dir()
        .unwrap()
        .join("target")
        .join(format!("youtube-save-retry-{:016x}", rand::random::<u64>()));
    app.dirs.state = scratch.clone();
    let first = AtomicBool::new(true);
    app.files = Arc::new(fastframe_file::Writer::with_write(
        move |path, bytes, access| {
            if first.swap(false, Ordering::SeqCst) {
                return Err(std::io::Error::other("injected save failure"));
            }
            fastframe_file::write_with(path, access, |file| std::io::Write::write_all(file, bytes))
        },
    ));
    app.user = Some(User {
        id: "alice".into(),
        ..Default::default()
    });
    app.request_youtube_likes();
    let generation = app.youtube.generation;
    app.youtube_likes_received("alice", generation, Ok(Default::default()));
    let track = youtube_fixture_track();
    app.cache_soundcloud_tracks(std::slice::from_ref(&track));
    // The backend remains offline; only this scratch-directory save is real.
    app.offline = false;
    app.toggle_youtube_like(&track.uri);
    let failed_revision = app.youtube.write_revision;
    assert!(app.files.flush(fastframe_file::FLUSH_TIMEOUT));
    app.youtube_likes_write_failed("alice", generation, failed_revision);
    assert_eq!(app.youtube_likes().unwrap(), std::slice::from_ref(&track));
    assert_eq!(
        app.soundcloud_pages["youtube:likes"].error.as_deref(),
        Some(crate::youtube::likes::SAVE_ERROR)
    );

    app.load_youtube_likes_page();
    assert_ne!(app.youtube.write_revision, failed_revision);
    assert!(app.files.flush(fastframe_file::FLUSH_TIMEOUT));
    assert!(app.soundcloud_pages["youtube:likes"].error.is_none());
    assert_eq!(app.youtube_likes().unwrap(), std::slice::from_ref(&track));
    let path = crate::youtube::likes::path(&scratch, "alice");
    assert_eq!(
        crate::youtube::likes::Likes::load(&path).unwrap().tracks,
        vec![track]
    );
    // A callback already queued before the retry must not reinstate its error.
    app.youtube_likes_write_failed("alice", generation, failed_revision);
    assert!(app.soundcloud_pages["youtube:likes"].error.is_none());
    assert!(!crate::youtube::likes::path(&scratch, "bob").exists());
    app.backend.shutdown();
    std::fs::remove_dir_all(scratch).unwrap();
}

#[test]
fn youtube_external_links_raise_window_and_use_the_common_queue() {
    let mut app = hybrid_fixture();
    let track = youtube_fixture_track();
    app.cache_soundcloud_tracks(std::slice::from_ref(&track));
    let uri = crate::link::parse("https://youtu.be/aB9_-012xyz?si=discarded").unwrap();
    app.apply(Action::OpenLink(uri), &egui::Context::default());
    assert!(
        app.actions
            .iter()
            .any(|action| matches!(action, Action::ShowWindow))
    );
    assert_eq!(app.current_track_uri().as_deref(), Some(track.uri.as_str()));
    assert!(app.hybrid_owned());
    assert!(app.pending_link.is_none());
    assert!(app.backend.take_remote_play_requests().is_empty());
    app.backend.shutdown();
}

#[test]
fn youtube_copy_link_accepts_both_menu_urls_and_canonical_uris() {
    let mut app = hybrid_fixture();
    let ctx = egui::Context::default();
    for link in [
        "youtube:video:aB9_-012xyz",
        "https://www.youtube.com/watch?v=aB9_-012xyz",
    ] {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.apply(Action::CopyLink(link.into()), ui.ctx());
        });
        output.textures_delta.clear();
        assert!(
            output
                .platform_output
                .commands
                .contains(&egui::OutputCommand::CopyText(
                    "https://www.youtube.com/watch?v=aB9_-012xyz".into(),
                ))
        );
    }
    app.backend.shutdown();
}

#[test]
fn youtube_playlist_link_uses_metadata_and_discards_a_closed_dialog_result() {
    let mut app = hybrid_fixture();
    let track = youtube_fixture_track();
    app.open_playlist_link("https://youtu.be/aB9_-012xyz");
    assert!(
        matches!(&app.dialog, Some(Dialog::AddToPlaylist { name, loading: true, .. }) if name == "YouTube")
    );
    app.youtube_track_received(
        app.youtube.generation,
        &track.uri,
        Ok(Box::new(track.clone())),
    );
    assert!(
        matches!(&app.dialog, Some(Dialog::AddToPlaylist { items, loading: false, .. }) if items == &vec![PlayableItem::Track(track.clone())])
    );
    app.cancel_playlist_link();
    app.track_cache.clear();
    app.open_playlist_link("https://youtu.be/aB9_-012xyz");
    app.cancel_playlist_link();
    app.youtube_track_received(
        app.youtube.generation,
        &track.uri,
        Ok(Box::new(track.clone())),
    );
    assert!(app.dialog.is_none());
    assert!(!app.track_cache.contains_key(&track.uri));
    app.backend.shutdown();
}
