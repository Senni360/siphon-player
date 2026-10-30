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
        }),
    );
    app.youtube_track_received(generation, &track.uri, Ok(Box::new(track.clone())));
    assert!(app.youtube_likes().is_none());
    assert!(!app.track_cache.contains_key(&track.uri));
    app.youtube_likes_received("bob", app.youtube.generation, Ok(Default::default()));
    assert!(app.youtube_likes().unwrap().is_empty());
    app.backend.shutdown();
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
