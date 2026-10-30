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

#[test]
fn hybrid_new_seek_after_load_settles_requires_numbered_ack() {
    use crate::player::HybridEvent;

    let mut app = hybrid_fixture();
    let ctx = egui::Context::default();
    start_hybrid_fixture(&mut app);
    app.apply(Action::Next, &ctx);
    let tag = app.hybrid_tag().unwrap();
    app.apply(Action::Seek(20_000), &ctx);
    app.handle_hybrid_event(
        tag,
        HybridEvent::State(Box::new(reliability_state(
            "soundcloud:track:7",
            10_000,
            0,
            true,
        ))),
    );
    // The first seek allowed an unnumbered load-position acknowledgment. A
    // newer seek issued after the decoder settles must not inherit that rule.
    app.apply(Action::Seek(10_500), &ctx);
    app.handle_hybrid_event(
        tag,
        HybridEvent::State(Box::new(reliability_state(
            "soundcloud:track:7",
            10_000,
            0,
            true,
        ))),
    );
    assert_eq!(app.hybrid_position(), 10_500);
    app.handle_hybrid_event(tag, HybridEvent::EndOfTrack);
    assert_eq!(app.hybrid_tag().unwrap().occurrence, tag.occurrence);
    assert_eq!(app.hybrid_position(), 10_500);
    app.backend.shutdown();
}

#[test]
fn hybrid_seek_decoder_reopen_does_not_restore_initial_load_exception() {
    use crate::player::HybridEvent;

    let mut app = hybrid_fixture();
    let ctx = egui::Context::default();
    start_hybrid_fixture(&mut app);
    app.apply(Action::Next, &ctx);
    let tag = app.hybrid_tag().unwrap();
    app.handle_hybrid_event(
        tag,
        HybridEvent::State(Box::new(reliability_state(
            "soundcloud:track:7",
            10_000,
            0,
            true,
        ))),
    );
    app.apply(Action::Seek(20_000), &ctx);
    let mut reopening = reliability_state("soundcloud:track:7", 20_000, 1, true);
    reopening.loading = true;
    app.handle_hybrid_event(tag, HybridEvent::State(Box::new(reopening)));
    app.apply(Action::Seek(20_500), &ctx);
    // Seek one's settled decoder state is close to seek two's target, but
    // its sequence still identifies it as an older observation.
    app.handle_hybrid_event(
        tag,
        HybridEvent::State(Box::new(reliability_state(
            "soundcloud:track:7",
            20_000,
            1,
            true,
        ))),
    );
    assert_eq!(app.hybrid_position(), 20_500);
    app.handle_hybrid_event(tag, HybridEvent::EndOfTrack);
    assert_eq!(app.hybrid_tag().unwrap().occurrence, tag.occurrence);
    assert_eq!(app.hybrid_position(), 20_500);
    app.backend.shutdown();
}

#[test]
fn hybrid_unresolved_context_survives_manual_exhaustion_and_waiting_navigation() {
    use crate::{backend::HybridCommand, player::HybridEvent};

    let mut app = hybrid_fixture();
    let ctx = egui::Context::default();
    app.apply(
        Action::PlayContext {
            uri: "soundcloud:playlist:91".into(),
            offset_uri: None,
            offset_index: None,
        },
        &ctx,
    );
    let generation = app
        .backend
        .take_hybrid_commands()
        .into_iter()
        .find_map(|command| match command {
            HybridCommand::Resolve { generation, .. } => Some(generation),
            _ => None,
        })
        .unwrap();
    app.apply(
        Action::QueueMany {
            songs: vec![
                ("spotify:track:manual".into(), "Manual Spotify".into()),
                ("youtube:video:dQw4w9WgXcQ".into(), "Manual YouTube".into()),
            ],
        },
        &ctx,
    );
    app.apply(Action::Next, &ctx);
    assert_eq!(
        app.current_track_uri().as_deref(),
        Some("spotify:track:manual")
    );
    app.apply(Action::Next, &ctx);
    assert_eq!(
        app.current_track_uri().as_deref(),
        Some("youtube:video:dQw4w9WgXcQ")
    );
    let old_manual = app.hybrid_tag().unwrap();
    app.backend.take_hybrid_commands();
    app.apply(Action::Next, &ctx);
    assert!(app.hybrid_tag().is_none());
    assert!(matches!(
        app.backend.take_hybrid_commands().as_slice(),
        [HybridCommand::Stop]
    ));
    app.apply(Action::Previous, &ctx);
    app.apply(Action::Next, &ctx);
    app.apply(Action::TogglePlay, &ctx);
    assert!(
        !app.backend
            .take_hybrid_commands()
            .iter()
            .any(|command| matches!(command, HybridCommand::Resolve { .. }))
    );
    app.handle_hybrid_context(
        generation,
        Ok(crate::hybrid_queue::Context {
            uri: "soundcloud:playlist:91".into(),
            name: "Delayed mixed set".into(),
            tracks: vec![
                song("soundcloud:track:91"),
                song("spotify:track:context"),
                song("youtube:video:abcdefghijk"),
            ],
        }),
    );
    assert_eq!(
        app.current_track_uri().as_deref(),
        Some("spotify:track:context")
    );
    assert!(!app.now_playing().unwrap().playing);
    assert!(app.manual_queue.is_empty());
    assert_eq!(queue_uris(&app).1, ["youtube:video:abcdefghijk"]);
    let resolved = app.hybrid_tag();
    app.handle_hybrid_event(old_manual, HybridEvent::EndOfTrack);
    assert_eq!(app.hybrid_tag(), resolved);
    app.apply(Action::Previous, &ctx);
    assert_eq!(
        app.current_track_uri().as_deref(),
        Some("youtube:video:dQw4w9WgXcQ")
    );
    assert_eq!(
        queue_uris(&app).1,
        ["spotify:track:context", "youtube:video:abcdefghijk"]
    );
    app.backend.shutdown();
}

mod hybrid_action_model {
    use super::*;
    use crate::hybrid_queue::{Entry, EntryId, HybridQueue, Origin, Repeat};
    use crate::player::{HybridEvent, PlaybackTag};
    use rand::{Rng, SeedableRng};

    // A small action ledger. Allocated IDs and the chosen shuffle permutation
    // are observations; navigation, manual priority, repeat, and history are
    // predicted independently of HybridQueue's transition methods.
    #[derive(Default)]
    struct Ledger {
        original: Vec<Entry>,
        order: Vec<Entry>,
        cursor: usize,
        anchor: Option<EntryId>,
        current: Option<Entry>,
        manual: Vec<Entry>,
        visits: Vec<(Entry, Option<EntryId>)>,
        allocated: HashSet<EntryId>,
        repeat: Repeat,
        shuffle: bool,
        playing: bool,
    }

    fn queue(app: &App) -> HybridQueue {
        let encoded = serde_json::to_string(&app.hybrid_snapshot().unwrap()).unwrap();
        HybridQueue::restore(serde_json::from_str(&encoded).unwrap()).unwrap()
    }

    fn ids(entries: &[Entry]) -> Vec<EntryId> {
        entries.iter().map(|entry| entry.id).collect()
    }

    fn uris(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| entry.track.uri.clone())
            .collect()
    }

    impl Ledger {
        fn context_tail(&self) -> Vec<Entry> {
            let mut tail = self.order[self.cursor..].to_vec();
            if self.repeat == Repeat::Context {
                tail.extend_from_slice(&self.order[..self.cursor]);
            }
            tail
        }

        fn shown_rows(&self) -> Vec<Entry> {
            let mut rows = self.manual.clone();
            rows.extend(self.context_tail());
            rows
        }

        fn leave_repeat_one(&mut self) {
            if self.repeat == Repeat::Track {
                self.repeat = Repeat::Context;
            }
        }

        fn next(&mut self) {
            if let Some(current) = self.current.take() {
                self.visits.push((current, self.anchor));
            }
            if !self.manual.is_empty() {
                self.current = Some(self.manual.remove(0));
            } else {
                if self.cursor == self.order.len() && self.repeat == Repeat::Context {
                    self.cursor = 0;
                }
                if let Some(entry) = self.order.get(self.cursor).cloned() {
                    self.anchor = Some(entry.id);
                    self.current = Some(entry);
                    self.cursor += 1;
                }
            }
            if self.current.is_none() {
                self.playing = false;
            }
        }

        fn previous(&mut self) {
            if let Some((previous, anchor)) = self.visits.pop() {
                if let Some(current) = self.current.take()
                    && current.origin == Origin::Manual
                {
                    self.manual.insert(0, current);
                }
                self.current = Some(previous);
                self.anchor = anchor;
                self.resume_after_anchor();
            }
        }

        fn resume_after_anchor(&mut self) {
            self.cursor = self.anchor.map_or(0, |anchor| {
                self.order
                    .iter()
                    .position(|entry| entry.id == anchor)
                    .unwrap()
                    + 1
            });
        }

        fn register(&mut self, entries: &[Entry], tracks: &[Track], origin: bool, detail: &str) {
            assert_eq!(entries.len(), tracks.len(), "{detail}");
            for (index, (entry, track)) in entries.iter().zip(tracks).enumerate() {
                assert!(
                    self.allocated.insert(entry.id),
                    "reused occurrence: {detail}"
                );
                assert_eq!(&entry.track, track, "{detail}");
                assert_eq!(
                    entry.origin,
                    if origin {
                        Origin::Context { index }
                    } else {
                        Origin::Manual
                    },
                    "{detail}"
                );
            }
        }

        fn check(&self, app: &App, detail: &str) {
            let actual = queue(app);
            assert_eq!(actual.current(), self.current.as_ref(), "current: {detail}");
            assert_eq!(
                actual
                    .playing_next(usize::MAX)
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>(),
                self.manual,
                "manual: {detail}"
            );
            assert_eq!(
                actual
                    .next_up(usize::MAX)
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>(),
                self.context_tail(),
                "context: {detail}"
            );
            assert_eq!(
                actual.history().cloned().collect::<Vec<_>>(),
                self.visits
                    .iter()
                    .map(|(entry, _)| entry.clone())
                    .collect::<Vec<_>>(),
                "history: {detail}"
            );
            assert_eq!(actual.shuffle(), self.shuffle, "{detail}");
            assert_eq!(actual.repeat(), self.repeat, "{detail}");
            assert_eq!(
                app.manual_queue,
                uris(&self.manual),
                "shown manual: {detail}"
            );
            let expected_uri = self.current.as_ref().map(|entry| entry.track.uri.clone());
            assert_eq!(
                queue_uris(app),
                (expected_uri.clone(), uris(&self.shown_rows())),
                "shown queue: {detail}"
            );
            assert_eq!(app.current_track_uri(), expected_uri, "{detail}");
            if let Some(now) = app.now_playing() {
                assert_eq!(now.playing, self.playing, "transport: {detail}");
                assert_eq!(now.shuffle, self.shuffle, "{detail}");
            }
            assert_eq!(
                app.hybrid_tag().map(|tag| tag.occurrence),
                self.current.as_ref().map(|entry| entry.id),
                "decoder occurrence: {detail}"
            );
        }
    }

    fn track(serial: usize, source: usize) -> Track {
        let uri = match source {
            0 => format!("spotify:track:q{serial}"),
            1 => format!("soundcloud:track:{serial}"),
            _ => format!("youtube:video:{serial:011}"),
        };
        Track {
            name: uri.clone(),
            uri,
            duration_ms: 120_000,
            ..Default::default()
        }
    }

    fn start(
        app: &mut App,
        model: &mut Ledger,
        serial: usize,
        index: usize,
        ctx: &egui::Context,
        detail: &str,
    ) {
        app.apply(Action::SetShuffle(false), ctx);
        model.shuffle = false;
        model.leave_repeat_one();
        let first = track(serial, 0);
        let tracks = vec![
            first.clone(),
            track(serial + 1, 1),
            track(serial + 2, 2),
            first,
        ];
        for track in &tracks {
            app.track_cache.insert(track.uri.clone(), track.clone());
        }
        app.apply(
            Action::PlayUris {
                uris: tracks.iter().map(|track| track.uri.clone()).collect(),
                index: index as u32,
            },
            ctx,
        );
        let original = queue(app).context().unwrap().entries.clone();
        model.register(&original, &tracks, true, detail);
        model.original = original.clone();
        model.order = original;
        model.current = Some(model.order[index].clone());
        model.anchor = model.current.as_ref().map(|entry| entry.id);
        model.cursor = index + 1;
        model.visits.clear();
        model.playing = true;
    }

    fn stale_events(app: &mut App, stale: &(PlaybackTag, String), detail: &str) {
        let before = queue(app).snapshot();
        let tag = app.hybrid_tag();
        let playing = app.now_playing().map(|now| now.playing);
        app.backend.take_hybrid_commands();
        for event in [
            HybridEvent::State(Box::new(reliability_state(
                &stale.1,
                119_999,
                99,
                !playing.unwrap_or(false),
            ))),
            HybridEvent::EndOfTrack,
            HybridEvent::Failed("late fixture failure".into()),
            HybridEvent::Interrupted,
            HybridEvent::PlayingAcknowledged { sequence: u64::MAX },
        ] {
            app.handle_hybrid_event(stale.0, event);
        }
        // Position may advance on the wall clock while playing. The ledger
        // below checks every queue field; normalize only elapsed position here.
        let mut after = queue(app);
        after.set_position_ms(HybridQueue::restore(before.clone()).unwrap().position_ms());
        assert_eq!(
            after.snapshot(),
            before,
            "late event changed queue: {detail}"
        );
        assert_eq!(
            app.hybrid_tag(),
            tag,
            "late event changed decoder: {detail}"
        );
        assert_eq!(
            app.now_playing().map(|now| now.playing),
            playing,
            "{detail}"
        );
        assert!(
            app.backend.take_hybrid_commands().is_empty(),
            "late event sent command: {detail}"
        );
    }

    #[test]
    fn hybrid_seeded_actions_follow_occurrence_ledger_and_ignore_late_engines() {
        let mut visited_sources = HashSet::new();
        let mut same_uri_late_events = 0;
        let mut source_handoffs = HashSet::new();
        for seed in [1_u64, 7, 19, 42, 31337, 0xdecafbad] {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let mut app = hybrid_fixture();
            app.restore_hybrid(Some(HybridQueue::new(seed).snapshot()));
            let ctx = egui::Context::default();
            let mut model = Ledger::default();
            let mut stale = Vec::new();
            let mut trace = Vec::new();
            start(&mut app, &mut model, 1, 0, &ctx, "initial context");
            app.backend.take_hybrid_commands();
            for step in 0..192 {
                let action = rng.random_range(0..16);
                trace.push(action);
                let detail = format!("seed={seed}, step={step}, actions={trace:?}");
                let old = app
                    .hybrid_tag()
                    .zip(model.current.as_ref().map(|entry| entry.track.uri.clone()));
                let old_source = model.current.as_ref().map(Entry::source);
                if let Some((tag, uri)) = &old {
                    let position = if action == 3 { 4_001 } else { 0 };
                    app.handle_hybrid_event(
                        *tag,
                        HybridEvent::State(Box::new(reliability_state(
                            uri,
                            position,
                            0,
                            model.playing,
                        ))),
                    );
                }
                match action {
                    0 => {
                        model.leave_repeat_one();
                        model.next();
                        app.apply(Action::Next, &ctx);
                    }
                    1 => {
                        if let Some(tag) = app.hybrid_tag() {
                            if model.repeat != Repeat::Track {
                                model.next();
                            }
                            app.handle_hybrid_event(tag, HybridEvent::EndOfTrack);
                        }
                    }
                    2 | 3 => {
                        if action == 2 || model.current.is_none() {
                            model.leave_repeat_one();
                            model.previous();
                        }
                        app.apply(Action::Previous, &ctx);
                    }
                    4..=6 => {
                        let track = track(1000 + step * 4, rng.random_range(0..3));
                        let tracks = vec![track.clone(), track];
                        for track in &tracks {
                            app.track_cache.insert(track.uri.clone(), track.clone());
                        }
                        let at = match action {
                            4 => model.manual.len(),
                            5 => 0,
                            _ => rng.random_range(0..=model.manual.len()),
                        };
                        let items = tracks.iter().cloned().map(PlayableItem::Track).collect();
                        app.apply(
                            match action {
                                4 => Action::QueueMany {
                                    songs: tracks
                                        .iter()
                                        .map(|track| (track.uri.clone(), track.name.clone()))
                                        .collect(),
                                },
                                5 => Action::PlayNext { items },
                                _ => Action::InsertInQueue {
                                    items,
                                    position: at,
                                },
                            },
                            &ctx,
                        );
                        let actual = queue(&app);
                        let allocated = actual.playing_next(usize::MAX)[at..at + 2]
                            .iter()
                            .map(|entry| (*entry).clone())
                            .collect::<Vec<_>>();
                        model.register(&allocated, &tracks, false, &detail);
                        model.manual.splice(at..at, allocated);
                    }
                    7 => {
                        if !model.manual.is_empty() {
                            let at = rng.random_range(0..model.manual.len());
                            let removed = model.manual.remove(at);
                            app.apply(
                                Action::RemoveFromQueue {
                                    rows: vec![
                                        (at, removed.track.uri),
                                        (usize::MAX, "stale row".into()),
                                    ],
                                },
                                &ctx,
                            );
                        }
                    }
                    8 => {
                        if !model.manual.is_empty() {
                            let from = rng.random_range(0..model.manual.len());
                            let to = rng.random_range(0..=model.manual.len() + 2);
                            let entry = model.manual.remove(from);
                            let final_index =
                                if to > from { to - 1 } else { to }.min(model.manual.len());
                            model.manual.insert(final_index, entry);
                            app.apply(Action::MoveInQueue { from, to }, &ctx);
                        }
                    }
                    9 => {
                        model.manual.clear();
                        app.apply(Action::ClearQueue, &ctx);
                    }
                    10 => {
                        model.shuffle = !model.shuffle;
                        app.apply(Action::SetShuffle(model.shuffle), &ctx);
                        if model.shuffle {
                            let expected = &model.order[model.cursor..];
                            let actual = queue(&app)
                                .next_up(usize::MAX)
                                .into_iter()
                                .cloned()
                                .collect::<Vec<_>>();
                            let suffix = &actual[..expected.len()];
                            assert_eq!(
                                ids(suffix).into_iter().collect::<HashSet<_>>(),
                                ids(expected).into_iter().collect(),
                                "shuffle lost/repeated occurrence: {detail}"
                            );
                            model.order.splice(model.cursor.., suffix.iter().cloned());
                        } else {
                            model.order.clone_from(&model.original);
                            model.resume_after_anchor();
                        }
                    }
                    11 => {
                        let mode = rng.random_range(0..3);
                        model.repeat = [Repeat::Off, Repeat::Context, Repeat::Track][mode];
                        app.apply(
                            Action::SetRepeat(
                                [RepeatMode::Off, RepeatMode::Context, RepeatMode::Track][mode],
                            ),
                            &ctx,
                        );
                    }
                    12 => start(
                        &mut app,
                        &mut model,
                        100_000 + step * 4,
                        rng.random_range(0..4),
                        &ctx,
                        &detail,
                    ),
                    13 => {
                        // Explicit row selection leaves repeat-one before the
                        // queue is read, so its wrap becomes visible too.
                        model.leave_repeat_one();
                        let rows = model.shown_rows();
                        if !rows.is_empty() {
                            let index = rng.random_range(0..rows.len());
                            let uri = rows[index].track.uri.clone();
                            for _ in 0..=index {
                                model.next();
                            }
                            model.playing = true;
                            app.apply(
                                Action::PlayFromRow {
                                    context: RowContext::Queue,
                                    uri,
                                    index: index as u32,
                                },
                                &ctx,
                            );
                        } else {
                            app.apply(Action::Next, &ctx);
                            model.next();
                        }
                    }
                    14 => {
                        if app.hybrid_tag().is_some() {
                            model.playing = !model.playing;
                        } else {
                            model.playing = true;
                            model.next();
                        }
                        app.apply(Action::TogglePlay, &ctx);
                    }
                    _ => {
                        if let Some(tag) = app.hybrid_tag() {
                            app.handle_hybrid_event(
                                tag,
                                HybridEvent::State(Box::new(reliability_state(
                                    "youtube:video:wrong______",
                                    70_000,
                                    99,
                                    !model.playing,
                                ))),
                            );
                        }
                    }
                }
                model.check(&app, &detail);
                for command in app.backend.take_hybrid_commands() {
                    match command {
                        crate::backend::HybridCommand::Load {
                            tag, track, play, ..
                        } => {
                            assert_eq!(Some(tag), app.hybrid_tag(), "load generation: {detail}");
                            assert_eq!(
                                Some(track.as_ref()),
                                model.current.as_ref().map(|entry| &entry.track),
                                "load source: {detail}"
                            );
                            assert_eq!(play, model.playing, "load intent: {detail}");
                        }
                        crate::backend::HybridCommand::Preload { tag, track }
                            if Some(tag) == app.hybrid_tag() =>
                        {
                            let upcoming = if model.repeat == Repeat::Track {
                                model.current.clone()
                            } else {
                                model.shown_rows().into_iter().next()
                            };
                            assert_eq!(
                                Some(track.as_ref()),
                                upcoming.as_ref().map(|entry| &entry.track),
                                "preload source: {detail}"
                            );
                        }
                        _ => {}
                    }
                }
                if let Some(source) = model.current.as_ref().map(Entry::source) {
                    visited_sources.insert(source);
                    if let Some(previous) = old_source
                        && previous != source
                    {
                        source_handoffs.insert((previous, source));
                    }
                }
                if let Some(old) = old
                    && Some(old.0) != app.hybrid_tag()
                {
                    stale.push(old);
                }
                if !stale.is_empty() {
                    let old = &stale[rng.random_range(0..stale.len())];
                    if model
                        .current
                        .as_ref()
                        .is_some_and(|entry| entry.track.uri == old.1)
                    {
                        same_uri_late_events += 1;
                    }
                    stale_events(&mut app, old, &detail);
                    model.check(&app, &detail);
                }
                // Matching-tag transport echoes also cannot reverse pause/play.
                if let Some((tag, entry)) = app.hybrid_tag().zip(model.current.as_ref()) {
                    app.handle_hybrid_event(
                        tag,
                        HybridEvent::State(Box::new(reliability_state(
                            &entry.track.uri,
                            0,
                            0,
                            !model.playing,
                        ))),
                    );
                    model.check(&app, &detail);
                }
                app.backend.take_hybrid_commands();
            }
            app.backend.shutdown();
        }
        assert_eq!(visited_sources.len(), 3);
        assert_eq!(
            source_handoffs.len(),
            6,
            "every directed source handoff must be exercised"
        );
        assert!(
            same_uri_late_events > 10,
            "late same-recording occurrences must be exercised"
        );
    }
}
