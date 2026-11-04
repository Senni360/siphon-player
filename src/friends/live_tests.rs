use super::live::*;
use super::{NewPost, Source};

const ME: &str = "me";
const ANA: &str = "ana";
const BO: &str = "bo";

fn song(title: &str) -> NewPost {
    NewPost {
        url: format!("https://open.spotify.com/track/{title}"),
        source: Source::Spotify,
        title: title.into(),
        artists: vec!["Artist".into()],
        artwork_url: None,
        duration_ms: 180_000,
        note: String::new(),
        uri: None,
        to: Vec::new(),
    }
}

fn presence(user: &str, title: Option<&str>, following: Option<&str>) -> Presence {
    Presence {
        user_id: user.into(),
        item: title.map(song),
        position_ms: 10_000,
        playing: true,
        at: 1_000,
        following: following.map(str::to_owned),
    }
}

fn entry(id: &str) -> Entry {
    Entry {
        id: id.into(),
        item: song(id),
        added_by: ANA.into(),
    }
}

fn channel(members: &[&str]) -> Channel {
    Channel {
        id: "c1".into(),
        name: "Friday".into(),
        host_id: ANA.into(),
        created_at: 1,
        members: members.iter().map(|m| (*m).to_owned()).collect(),
        current: Some(entry("one")),
        position_ms: 5_000,
        playing: true,
        at: 1_000,
        queue: vec![entry("two"), entry("three"), entry("four")],
        locked: false,
        controllers: Vec::new(),
        skip_requests: Vec::new(),
        background: None,
    }
}

fn snapshot(presence: Vec<Presence>, channels: Vec<Channel>) -> Snapshot {
    Snapshot {
        now: 1_000,
        presence,
        channels,
    }
}

fn read_answer(model: &mut Model, snapshot: Snapshot) -> Outcome {
    let request = model.read().unwrap();
    model.receive(
        answer(
            request.generation,
            true,
            false,
            1_000,
            1_000,
            Ok(Update::Snapshot(snapshot)),
        ),
        ME,
    )
}

fn titles(channel: &Channel) -> Vec<&str> {
    channel
        .queue
        .iter()
        .map(|e| e.item.title.as_str())
        .collect()
}

#[test]
fn listening_along_follows_the_leaders_song_and_position() {
    let mut model = Model::default();
    read_answer(
        &mut model,
        snapshot(vec![presence(ANA, Some("one"), None)], vec![]),
    );
    model.follow(ANA.into());
    let wanted = model.wanted(4_000).unwrap();
    assert_eq!(wanted.item.title, "one");
    // Ten seconds in at 1 s, read at 4 s.
    assert_eq!(wanted.position_ms, 13_000);
    assert!(wanted.playing);
    assert_eq!(wanted.entry, None);
}

#[test]
fn a_paused_leader_holds_the_position() {
    let mut model = Model::default();
    let mut ana = presence(ANA, Some("one"), None);
    ana.playing = false;
    read_answer(&mut model, snapshot(vec![ana], vec![]));
    model.follow(ANA.into());
    let wanted = model.wanted(60_000).unwrap();
    assert_eq!(wanted.position_ms, 10_000);
    assert!(!wanted.playing);
}

#[test]
fn following_a_follower_reaches_whoever_is_choosing() {
    let mut model = Model::default();
    let mut ana = presence(ANA, Some("old"), Some(BO));
    ana.position_ms = 1;
    read_answer(
        &mut model,
        snapshot(vec![ana, presence(BO, Some("new"), None)], vec![]),
    );
    model.follow(ANA.into());
    assert_eq!(model.wanted(1_000).unwrap().item.title, "new");
    assert_eq!(model.followers_of(BO), vec![ANA]);
}

#[test]
fn a_loop_of_followers_has_no_leader_and_ends_the_session() {
    let mut model = Model::default();
    model.follow(ANA.into());
    let outcome = read_answer(
        &mut model,
        snapshot(
            vec![
                presence(ANA, Some("one"), Some(BO)),
                presence(BO, Some("one"), Some(ANA)),
            ],
            vec![],
        ),
    );
    assert_eq!(outcome, Outcome::LeaderGone(ANA.into()));
    assert_eq!(model.session, Session::Alone);
}

#[test]
fn the_session_ends_when_the_leader_is_gone() {
    let mut model = Model::default();
    read_answer(
        &mut model,
        snapshot(vec![presence(ANA, Some("one"), None)], vec![]),
    );
    model.follow(ANA.into());
    let outcome = read_answer(&mut model, snapshot(vec![], vec![]));
    assert_eq!(outcome, Outcome::LeaderGone(ANA.into()));
    assert!(!model.together());
    assert_eq!(model.wanted(1_000), None);
}

#[test]
fn a_leader_with_nothing_shared_stays_followed_with_nothing_wanted() {
    let mut model = Model::default();
    model.follow(ANA.into());
    let outcome = read_answer(
        &mut model,
        snapshot(vec![presence(ANA, None, None)], vec![]),
    );
    assert_eq!(outcome, Outcome::Nothing);
    assert!(model.together());
    assert_eq!(model.wanted(1_000), None);
}

#[test]
fn following_someone_who_follows_you_does_not_stand() {
    let mut model = Model::default();
    model.follow(ANA.into());
    let outcome = read_answer(
        &mut model,
        snapshot(
            vec![
                presence(ANA, Some("one"), Some(ME)),
                presence(ME, Some("one"), None),
            ],
            vec![],
        ),
    );
    assert_eq!(outcome, Outcome::LeaderGone(ANA.into()));
}

#[test]
fn a_channel_wants_its_current_song_for_its_members() {
    let mut model = Model::default();
    read_answer(&mut model, snapshot(vec![], vec![channel(&[ANA])]));
    model.join_channel(ME, "c1");
    assert_eq!(model.my_channel().unwrap().members, vec![ANA, ME]);
    let wanted = model.wanted(3_000).unwrap();
    assert_eq!(wanted.item.title, "one");
    assert_eq!(wanted.position_ms, 7_000);
    assert_eq!(wanted.entry.as_deref(), Some("one"));
}

#[test]
fn channel_changes_show_at_once() {
    let mut model = Model::default();
    read_answer(&mut model, snapshot(vec![], vec![channel(&[ANA, ME])]));
    model.join_channel(ME, "c1");

    model.add(ME, song("five"), Place::End, 2_000);
    model.add(ME, song("zero"), Place::Next, 2_000);
    assert_eq!(
        titles(model.my_channel().unwrap()),
        ["zero", "two", "three", "four", "five"]
    );

    model.move_entry("three", 0);
    model.remove("two");
    assert_eq!(
        titles(model.my_channel().unwrap()),
        ["three", "zero", "four", "five"]
    );

    model.set_playback(false, 42_000, 3_000);
    let wanted = model.wanted(90_000).unwrap();
    assert_eq!((wanted.playing, wanted.position_ms), (false, 42_000));

    model.advance(None, 4_000);
    let now = model.my_channel().unwrap();
    assert_eq!(now.current.as_ref().unwrap().item.title, "three");
    assert!(now.playing);
    assert_eq!(now.position_ms, 0);
}

#[test]
fn skipping_to_a_row_drops_the_rows_above_it() {
    let mut model = Model::default();
    read_answer(&mut model, snapshot(vec![], vec![channel(&[ME])]));
    model.join_channel(ME, "c1");
    model.advance(Some("three"), 2_000);
    let now = model.my_channel().unwrap();
    assert_eq!(now.current.as_ref().unwrap().id, "three");
    assert_eq!(titles(now), ["four"]);
    // A row someone else already removed changes nothing.
    model.advance(Some("gone"), 2_000);
    assert_eq!(
        model.my_channel().unwrap().current.as_ref().unwrap().id,
        "three"
    );
}

#[test]
fn the_queue_running_out_stops_the_channel_until_a_song_is_added() {
    let mut model = Model::default();
    let mut last = channel(&[ME]);
    last.queue.clear();
    read_answer(&mut model, snapshot(vec![], vec![last]));
    model.join_channel(ME, "c1");
    model.advance(None, 2_000);
    assert_eq!(model.wanted(3_000), None);
    assert!(!model.my_channel().unwrap().playing);
    model.add(ME, song("five"), Place::End, 4_000);
    let wanted = model.wanted(4_000).unwrap();
    assert_eq!(wanted.item.title, "five");
    assert!(wanted.playing);
}

/// The interface is optimistic: an answer from before what the listener just
/// did must not undo it.
#[test]
fn a_read_from_before_a_change_does_not_undo_it() {
    let mut model = Model::default();
    read_answer(&mut model, snapshot(vec![], vec![channel(&[ANA, ME])]));
    model.join_channel(ME, "c1");
    let stale = model.read().unwrap();

    model.remove("two");
    let write = model.request(Op::Remove {
        channel: "c1".into(),
        entry: "two".into(),
    });
    assert_ne!(stale.generation, write.generation);

    let outcome = model.receive(
        answer(
            stale.generation,
            true,
            false,
            1_000,
            1_000,
            Ok(Update::Snapshot(snapshot(
                vec![presence(BO, Some("news"), None)],
                vec![channel(&[ANA, ME])],
            ))),
        ),
        ME,
    );
    assert_eq!(outcome, Outcome::Nothing);
    assert_eq!(titles(model.my_channel().unwrap()), ["three", "four"]);
    // Presence in the same answer is still news.
    assert!(model.presence_of(BO).is_some());
}

#[test]
fn only_the_latest_changes_answer_replaces_the_channel() {
    let mut model = Model::default();
    read_answer(&mut model, snapshot(vec![], vec![channel(&[ME])]));
    model.join_channel(ME, "c1");
    let first = model.request(Op::Remove {
        channel: "c1".into(),
        entry: "two".into(),
    });
    let second = model.request(Op::Remove {
        channel: "c1".into(),
        entry: "three".into(),
    });
    model.remove("two");
    model.remove("three");
    let mut after_first = channel(&[ME]);
    after_first.queue.remove(0);
    model.receive(
        answer(
            first.generation,
            false,
            true,
            1_000,
            1_000,
            Ok(Update::Channel(Some(Box::new(after_first)), 1_000)),
        ),
        ME,
    );
    assert_eq!(titles(model.my_channel().unwrap()), ["four"]);
    let mut after_second = channel(&[ME]);
    after_second.queue = vec![entry("four"), entry("added-by-ana")];
    model.receive(
        answer(
            second.generation,
            false,
            true,
            1_000,
            1_000,
            Ok(Update::Channel(Some(Box::new(after_second)), 1_000)),
        ),
        ME,
    );
    assert_eq!(
        titles(model.my_channel().unwrap()),
        ["four", "added-by-ana"]
    );
}

#[test]
fn a_new_channel_shows_at_once_and_takes_the_services_id() {
    let mut model = Model::default();
    model.start_channel(ME, "Friday", Some(song("one")), 3_000, true, 1_000);
    assert_eq!(model.session, Session::Channel(PENDING_CHANNEL.into()));
    assert_eq!(model.wanted(2_000).unwrap().position_ms, 4_000);
    let request = model.request(Op::Create {
        name: "Friday".into(),
        item: None,
        position_ms: 0,
        playing: true,
    });
    let mut named = channel(&[ME]);
    named.id = "real".into();
    model.receive(
        answer(
            request.generation,
            false,
            true,
            1_000,
            1_000,
            Ok(Update::Channel(Some(Box::new(named)), 1_000)),
        ),
        ME,
    );
    assert_eq!(model.session, Session::Channel("real".into()));
    assert_eq!(model.snapshot.channels.len(), 1);
}

#[test]
fn a_channel_that_could_not_start_takes_the_session_back() {
    let mut model = Model::default();
    model.start_channel(ME, "Friday", None, 0, true, 1_000);
    let request = model.request(Op::Create {
        name: "Friday".into(),
        item: None,
        position_ms: 0,
        playing: true,
    });
    let outcome = model.receive(
        answer(
            request.generation,
            false,
            true,
            1_000,
            1_000,
            Err("no".into()),
        ),
        ME,
    );
    assert_eq!(outcome, Outcome::WriteFailed("no".into()));
    assert_eq!(model.session, Session::Alone);
    assert!(model.snapshot.channels.is_empty());
}

#[test]
fn being_dropped_from_a_channel_ends_the_session() {
    let mut model = Model::default();
    read_answer(&mut model, snapshot(vec![], vec![channel(&[ME, ANA])]));
    model.join_channel(ME, "c1");
    let outcome = read_answer(&mut model, snapshot(vec![], vec![channel(&[ANA])]));
    assert_eq!(outcome, Outcome::ChannelGone);
    assert_eq!(model.session, Session::Alone);
}

#[test]
fn a_failed_read_changes_nothing_and_allows_the_next() {
    let mut model = Model::default();
    read_answer(
        &mut model,
        snapshot(vec![presence(ANA, Some("one"), None)], vec![]),
    );
    model.follow(ANA.into());
    let request = model.read().unwrap();
    assert!(model.read().is_none());
    let outcome = model.receive(
        answer(
            request.generation,
            true,
            false,
            1_000,
            2_000,
            Err("offline".into()),
        ),
        ME,
    );
    assert_eq!(outcome, Outcome::Nothing);
    assert!(model.together());
    assert!(model.read().is_some());
}

#[test]
fn the_clock_difference_comes_from_the_quickest_round_trip() {
    let mut model = Model::default();
    let slow = model.read().unwrap();
    model.receive(
        answer(
            slow.generation,
            true,
            false,
            10_000,
            12_000,
            Ok(Update::Now(50_000)),
        ),
        ME,
    );
    // Sent at 10 s, answered at 12 s: the service said 50 s around 11 s.
    assert_eq!(model.server_now(11_000), 50_000);
    let quick = model.read().unwrap();
    model.receive(
        answer(
            quick.generation,
            true,
            false,
            20_000,
            20_100,
            Ok(Update::Now(60_500)),
        ),
        ME,
    );
    assert_eq!(model.server_now(20_050), 60_500);
    let slower = model.read().unwrap();
    model.receive(
        answer(
            slower.generation,
            true,
            false,
            30_000,
            33_000,
            Ok(Update::Now(99_999)),
        ),
        ME,
    );
    assert_eq!(model.server_now(20_050), 60_500);
}

#[test]
fn leaving_says_which_channel_to_tell() {
    let mut model = Model::default();
    read_answer(&mut model, snapshot(vec![], vec![channel(&[ME, ANA])]));
    model.join_channel(ME, "c1");
    assert_eq!(model.leave(ME).as_deref(), Some("c1"));
    assert_eq!(model.channel("c1").unwrap().members, vec![ANA]);
    assert!(!model.together());
    model.follow(ANA.into());
    assert_eq!(model.leave(ME), None);
}

#[test]
fn the_services_answers_are_understood() {
    let text = serde_json::json!({
        "now": 5,
        "presence": [{"user_id": "a", "item": {"url": "https://open.spotify.com/track/x", "source": "spotify", "title": "T", "artists": ["A"], "artwork_url": null, "duration_ms": 1}, "position_ms": 2, "playing": true, "at": 3, "following": null}],
        "channels": [{"id": "c", "name": "N", "host_id": "a", "created_at": 1, "members": ["a"], "current": null, "position_ms": 0, "playing": false, "at": 1, "queue": []}],
    });
    let snapshot: Snapshot = serde_json::from_value(text).unwrap();
    assert_eq!(snapshot.presence[0].item.as_ref().unwrap().title, "T");
    assert_eq!(snapshot.channels[0].name, "N");
}

/// Every request this client makes, against a real service: run
/// `SIPHON_FRIENDS_INVITE_CODE=siphon-local-test-invite npm run dev` in
/// `services/friends`, set `SIPHON_FRIENDS_URL` to it, and run with `--ignored`.
#[tokio::test]
#[ignore = "needs a running group service"]
async fn every_request_works_against_a_running_service() {
    use crate::http::Http;
    let base = std::env::var("SIPHON_FRIENDS_URL").unwrap();
    let invite = std::env::var("SIPHON_FRIENDS_INVITE_CODE").unwrap();
    let http = Http::new(crate::http::build_collection_client(&Default::default()).unwrap());
    let client = super::Client::new(http, &base).unwrap().for_live();
    let ana = client.join(&invite, "Ana").await.unwrap();
    let bo = client.join(&invite, "Bo").await.unwrap();
    let channel_of = |update: Update| match update {
        Update::Channel(Some(channel), _) => *channel,
        other => panic!("expected a channel, got {other:?}"),
    };
    let read = async |identity| match run(&client, identity, Op::Read).await.unwrap() {
        Update::Snapshot(snapshot) => snapshot,
        other => panic!("expected a snapshot, got {other:?}"),
    };

    run(
        &client,
        &ana,
        Op::Presence {
            item: Some(song("4uLU6hMCjMI75M1A2tKUQC")),
            position_ms: 5_000,
            playing: true,
            following: None,
        },
    )
    .await
    .unwrap();
    run(
        &client,
        &bo,
        Op::Presence {
            item: None,
            position_ms: 0,
            playing: false,
            following: Some(ana.user_id.clone()),
        },
    )
    .await
    .unwrap();
    let seen = read(&bo).await;
    let of_ana = seen
        .presence
        .iter()
        .find(|p| p.user_id == ana.user_id)
        .unwrap();
    assert_eq!(
        of_ana.item.as_ref().unwrap().title,
        "4uLU6hMCjMI75M1A2tKUQC"
    );
    assert_eq!(of_ana.position_ms, 5_000);
    let of_bo = seen
        .presence
        .iter()
        .find(|p| p.user_id == bo.user_id)
        .unwrap();
    assert_eq!(of_bo.following.as_deref(), Some(ana.user_id.as_str()));

    let made = channel_of(
        run(
            &client,
            &ana,
            Op::Create {
                name: "Friday".into(),
                item: Some(song("one")),
                position_ms: 1_000,
                playing: true,
            },
        )
        .await
        .unwrap(),
    );
    let id = made.id.clone();
    assert_eq!(made.current.as_ref().unwrap().item.title, "one");
    assert_eq!(
        channel_of(run(&client, &bo, Op::Join(id.clone())).await.unwrap())
            .members
            .len(),
        2
    );
    for (title, place) in [
        ("two", Place::End),
        ("three", Place::End),
        ("front", Place::Next),
    ] {
        run(
            &client,
            &bo,
            Op::Add {
                channel: id.clone(),
                item: song(title),
                place,
            },
        )
        .await
        .unwrap();
    }
    let channel = read(&ana)
        .await
        .channels
        .into_iter()
        .find(|c| c.id == id)
        .unwrap();
    let order = |c: &Channel| {
        c.queue
            .iter()
            .map(|e| e.item.title.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&channel), ["front", "two", "three"]);
    let moved = channel_of(
        run(
            &client,
            &ana,
            Op::Move {
                channel: id.clone(),
                entry: channel.queue[2].id.clone(),
                index: 0,
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(order(&moved), ["three", "front", "two"]);
    let removed = channel_of(
        run(
            &client,
            &bo,
            Op::Remove {
                channel: id.clone(),
                entry: moved.queue[1].id.clone(),
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(order(&removed), ["three", "two"]);
    // In a party only the host moves the music, and the service says so.
    run(
        &client,
        &bo,
        Op::Playback {
            channel: id.clone(),
            playing: false,
            position_ms: 42_000,
        },
    )
    .await
    .expect_err("a guest cannot pause the party");
    let paused = channel_of(
        run(
            &client,
            &ana,
            Op::Playback {
                channel: id.clone(),
                playing: false,
                position_ms: 42_000,
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!((paused.playing, paused.position_ms), (false, 42_000));
    let next = channel_of(
        run(
            &client,
            &ana,
            Op::Next {
                channel: id.clone(),
                from: paused.current.as_ref().map(|e| e.id.clone()),
                to: None,
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(next.current.as_ref().unwrap().item.title, "three");
    assert!(next.playing);
    let jumped = channel_of(
        run(
            &client,
            &ana,
            Op::Next {
                channel: id.clone(),
                from: None,
                to: Some(next.queue[0].id.clone()),
            },
        )
        .await
        .unwrap(),
    );
    assert_eq!(jumped.current.as_ref().unwrap().item.title, "two");
    assert!(jumped.queue.is_empty());

    assert!(matches!(
        run(&client, &bo, Op::LeaveChannel(id.clone()))
            .await
            .unwrap(),
        Update::Channel(None, _)
    ));
    run(&client, &ana, Op::LeaveChannel(id.clone()))
        .await
        .unwrap();
    assert!(read(&ana).await.channels.iter().all(|c| c.id != id));
    let summary = crate::stats::Summary {
        month: "2026-11".into(),
        minutes: 120,
        plays: 30,
        songs: vec![crate::stats::Top {
            name: "One".into(),
            detail: "Artist".into(),
            art: None,
            uri: "spotify:track:one".into(),
            plays: 3,
            minutes: 9,
        }],
        hours: vec![0; 24],
        ..Default::default()
    };
    run(
        &client,
        &ana,
        Op::PutStats {
            period: "month",
            summary: Box::new(summary.clone()),
        },
    )
    .await
    .unwrap();
    let Update::Stats("month", stats) = run(&client, &bo, Op::GetStats("month")).await.unwrap()
    else {
        panic!("expected stats");
    };
    assert!(
        stats
            .iter()
            .any(|row| row.user_id == ana.user_id && row.summary == summary)
    );
    run(&client, &ana, Op::DeleteStats).await.unwrap();
    let Update::Stats("month", stats) = run(&client, &bo, Op::GetStats("month")).await.unwrap()
    else {
        panic!("expected stats");
    };
    assert!(stats.iter().all(|row| row.user_id != ana.user_id));
    run(&client, &ana, Op::ClearPresence).await.unwrap();
    assert!(
        read(&bo)
            .await
            .presence
            .iter()
            .all(|p| p.user_id != ana.user_id)
    );
    client.leave(&ana).await.unwrap();
    client.leave(&bo).await.unwrap();
}

/// The app's own code against a local service and the real file store. Run the
/// service with `BLOB_READ_WRITE_TOKEN` and `SIPHON_FRIENDS_INVITE_CODE` set, set
/// `SIPHON_FRIENDS_URL` and the invite here too, and run with `--ignored`.
#[tokio::test]
#[ignore = "needs a local service connected to a real Blob store"]
async fn a_file_goes_up_plays_and_is_deleted_through_the_real_store() {
    let base = std::env::var("SIPHON_FRIENDS_URL").unwrap();
    let invite = std::env::var("SIPHON_FRIENDS_INVITE_CODE").unwrap();
    let http = crate::http::Http::default();
    let client = super::Client::new(http.clone(), &base).unwrap();
    let identity = client.join(&invite, "Upload check").await.unwrap();

    let file =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/testdata/soundcloud/tone.mp3");
    let bytes = std::fs::read(&file).unwrap();
    let song = client.upload_song(&identity, file.clone()).await.unwrap();
    assert!(crate::shared_songs::is_song(&song.uri()), "{}", song.id);
    assert_eq!(song.size, bytes.len() as u64);
    assert!(song.duration_ms > 0);

    let (songs, used, capacity) = client.shared_songs(&identity).await.unwrap();
    assert_eq!(songs.len(), 1);
    assert_eq!(used, bytes.len() as u64);
    assert!(capacity > 0);

    let (url, mime) = client.song_stream(&identity, &song.id).await.unwrap();
    assert_eq!(mime, "audio/mpeg");
    let got = http.client().unwrap().get(&url).send().await.unwrap();
    assert!(got.status().is_success());
    assert_eq!(got.bytes().await.unwrap().as_ref(), bytes.as_slice());

    let again = client.upload_song(&identity, file).await.unwrap_err();
    assert!(again.contains("already shared"), "{again}");

    client.delete_song(&identity, &song.id).await.unwrap();
    assert!(client.shared_songs(&identity).await.unwrap().0.is_empty());
}
