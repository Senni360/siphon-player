// Included in app::tests to exercise real Action dispatch with its offline backend.
fn reliability_state(uri: &str, position_ms: u32, seek_sequence: u64, playing: bool) -> LocalState {
    LocalState {
        playback: if playing {
            Playback::Playing
        } else {
            Playback::Paused
        },
        position_ms,
        seek_sequence,
        track: Some(crate::player::LocalTrack {
            uri: uri.into(),
            duration_ms: 120_000,
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[test]
fn hybrid_nearby_old_seek_snapshot_cannot_release_late_end_guard() {
    use crate::{backend::HybridCommand, player::HybridEvent};

    for uri in [
        "spotify:track:a",
        "soundcloud:track:7",
        "youtube:video:dQw4w9WgXcQ",
    ] {
        for playing in [false, true] {
            let mut app = hybrid_fixture();
            let ctx = egui::Context::default();
            app.apply(
                Action::PlayUris {
                    uris: vec![uri.into(), "soundcloud:track:9".into()],
                    index: 0,
                },
                &ctx,
            );
            if !playing {
                app.apply(Action::TogglePlay, &ctx);
            }
            let tag = app.hybrid_tag().unwrap();
            app.handle_hybrid_event(
                tag,
                HybridEvent::State(Box::new(reliability_state(uri, 10_000, 0, playing))),
            );
            app.apply(Action::Seek(10_500), &ctx);
            app.apply(Action::Seek(10_800), &ctx);
            // Neither the pre-seek snapshot nor the first seek's acknowledgment
            // confirms the latest seek, even when each is within one second.
            for (position, sequence) in [(10_000, 0), (10_500, 1)] {
                app.handle_hybrid_event(
                    tag,
                    HybridEvent::State(Box::new(reliability_state(
                        uri, position, sequence, playing,
                    ))),
                );
                assert_eq!(app.hybrid_position(), 10_800, "{uri}, sequence {sequence}");
            }
            app.backend.take_hybrid_commands();
            app.handle_hybrid_event(tag, HybridEvent::EndOfTrack);
            let restart = app.hybrid_tag().unwrap();
            assert_eq!(restart.occurrence, tag.occurrence, "{uri}");
            assert_ne!(restart.generation, tag.generation);
            assert!(
                app.backend
                    .take_hybrid_commands()
                    .iter()
                    .any(|command| matches!(
                        command,
                        HybridCommand::Load { tag, position_ms: 10_800, play, .. }
                            if *tag == restart && *play == playing
                    ))
            );
            app.handle_hybrid_event(tag, HybridEvent::EndOfTrack);
            assert_eq!(app.hybrid_tag(), Some(restart));
            // A new load starts numbering at zero. Its next numbered seek
            // really does release the guard and allows natural completion.
            app.handle_hybrid_event(
                restart,
                HybridEvent::State(Box::new(reliability_state(uri, 10_800, 0, playing))),
            );
            app.apply(Action::Seek(20_000), &ctx);
            app.handle_hybrid_event(
                restart,
                HybridEvent::State(Box::new(reliability_state(uri, 20_000, 1, playing))),
            );
            app.handle_hybrid_event(restart, HybridEvent::EndOfTrack);
            assert_eq!(
                app.current_track_uri().as_deref(),
                Some("soundcloud:track:9")
            );
            assert_eq!(app.now_playing().unwrap().playing, playing);
            app.backend.shutdown();
        }
    }
}

#[test]
fn hybrid_seek_folded_into_loading_request_can_finish_without_seeked() {
    use crate::player::HybridEvent;

    for initial_loading_state in [false, true] {
        let mut app = hybrid_fixture();
        let ctx = egui::Context::default();
        start_hybrid_fixture(&mut app);
        let tag = app.hybrid_tag().unwrap();
        if initial_loading_state {
            let mut loading = reliability_state("spotify:track:a", 0, 0, true);
            loading.loading = true;
            app.handle_hybrid_event(tag, HybridEvent::State(Box::new(loading)));
        }
        app.apply(Action::Seek(15_000), &ctx);
        app.handle_hybrid_event(
            tag,
            HybridEvent::State(Box::new(reliability_state(
                "spotify:track:a",
                15_000,
                0,
                true,
            ))),
        );
        app.handle_hybrid_event(tag, HybridEvent::EndOfTrack);
        assert_eq!(
            app.current_track_uri().as_deref(),
            Some("soundcloud:track:7")
        );
        assert_ne!(app.hybrid_tag().unwrap().occurrence, tag.occurrence);
        app.backend.shutdown();
    }
}
