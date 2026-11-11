// Included in app::tests so these exercise the existing offline app harness.
use crate::remote::{Command as PhoneCommand, Incoming as PhoneIncoming};

fn undo_row(uri: &str) -> PlayableItem {
    PlayableItem::Track(Track {
        uri: uri.into(),
        name: uri.into(),
        ..Default::default()
    })
}

#[test]
fn songs_put_back_land_exactly_where_they_were() {
    let list = |runs: &[(u32, Vec<PlayableItem>)]| -> Vec<(u32, Vec<String>)> {
        runs.iter()
            .map(|(at, items)| (*at, items.iter().map(|item| item.uri().to_owned()).collect()))
            .collect()
    };
    let runs = removal_runs(vec![
        (7, undo_row("h")),
        (1, undo_row("b")),
        (2, undo_row("c")),
        (5, undo_row("f")),
    ]);
    assert_eq!(
        list(&runs),
        [
            (1, vec!["b".to_owned(), "c".to_owned()]),
            (5, vec!["f".to_owned()]),
            (7, vec!["h".to_owned()]),
        ]
    );
    // Rebuilding a..h with those gone, in that order, gives the list back.
    let mut left: Vec<String> = ["a", "d", "e", "g"].map(str::to_owned).to_vec();
    for (at, items) in list(&runs) {
        let at = at as usize;
        left.splice(at..at, items);
    }
    assert_eq!(left.join(""), "abcdefgh");
    assert!(removal_runs(Vec::new()).is_empty());
}

#[test]
fn taking_something_out_of_the_library_offers_it_back() {
    assert_eq!(
        uri_for_undo(true, &["spotify:track:a".to_owned()]).as_deref(),
        Some("spotify:track:a")
    );
    assert_eq!(uri_for_undo(false, &["spotify:track:a".to_owned()]), None);
}

#[test]
fn a_phone_presses_the_same_buttons_as_this_computer() {
    use super::remote::phone_action as action;
    assert!(matches!(action(&PhoneCommand::Toggle), Some(Action::TogglePlay)));
    assert!(matches!(action(&PhoneCommand::Next), Some(Action::Next)));
    assert!(matches!(
        action(&PhoneCommand::Seek { ms: 5 }),
        Some(Action::Seek(5))
    ));
    assert!(matches!(
        action(&PhoneCommand::Volume { percent: 250 }),
        Some(Action::SetVolume(100))
    ));
}

#[test]
fn a_phone_cannot_have_this_computer_open_or_play_anything_else() {
    use super::remote::phone_action as action;
    let evil = "https://evil.example/x".to_owned();
    assert!(action(&PhoneCommand::Like { uri: evil.clone() }).is_none());
    assert!(action(&PhoneCommand::Queue { uri: evil.clone() }).is_none());
    assert!(action(&PhoneCommand::PlayContext { uri: evil.clone() }).is_none());
    assert!(
        action(&PhoneCommand::PlayLink {
            url: "https://evil.example/page".into()
        })
        .is_none(),
        "no browser opens over here"
    );
    assert!(
        action(&PhoneCommand::Play {
            uris: vec![evil],
            index: 0
        })
        .is_none()
    );
}

#[test]
fn the_song_pressed_on_a_phone_still_plays_when_others_are_filtered_out() {
    let played = super::remote::phone_action(&PhoneCommand::Play {
        uris: vec![
            "https://evil.example/x".into(),
            "spotify:track:a".into(),
            "soundcloud:track:7".into(),
        ],
        index: 2,
    });
    match played {
        Some(Action::PlayUris { uris, index }) => {
            assert_eq!(uris, ["spotify:track:a", "soundcloud:track:7"]);
            assert_eq!(uris[index as usize], "soundcloud:track:7");
        }
        _ => panic!("it should play"),
    }
    assert!(
        super::remote::phone_action(&PhoneCommand::Play {
            uris: vec!["spotify:track:a".into()],
            index: 9
        })
        .is_none()
    );
}

#[test]
fn a_phone_can_only_ask_about_things_with_plain_names() {
    use super::remote::fair_question as fair;
    assert!(fair(&PhoneCommand::Artist { id: "4uLU6hMCjMI75".into() }));
    assert!(!fair(&PhoneCommand::Artist { id: "../me/player".into() }));
    assert!(!fair(&PhoneCommand::Playlist { id: "a?b=c".into() }));
    assert!(!fair(&PhoneCommand::Album { id: String::new() }));
    assert!(fair(&PhoneCommand::Mixed { id: "0123456789abcdef".into() }));
    assert!(!fair(&PhoneCommand::Mixed { id: "../x".into() }));
    assert!(fair(&PhoneCommand::Source { uri: "soundcloud:user:12".into() }));
    assert!(fair(&PhoneCommand::Source { uri: "youtube:channel:UC1234567890123456789012".into() }));
    assert!(!fair(&PhoneCommand::Source { uri: "https://evil.example".into() }));
}

#[test]
fn a_phone_sees_this_computers_mixed_playlists() {
    let mut app = artist_page_app("remote-mixed");
    let ctx = egui::Context::default();
    let id = app.mixed_playlists.create("Across the streams".into(), String::new());
    let song = Track {
        uri: "soundcloud:track:7".into(),
        name: "Song".into(),
        ..Default::default()
    };
    app.mixed_playlists
        .get_mut(&id)
        .unwrap()
        .add_tracks(std::slice::from_ref(&song), None);
    app.remote_command(
        PhoneIncoming {
            id: "1".into(),
            command: PhoneCommand::Mixed { id: id.clone() },
        },
        &ctx,
    );
    assert!(app.phone.waiting.is_empty(), "answered at once");
    assert!(
        app.track_cache.contains_key("soundcloud:track:7"),
        "its songs can be played from the phone"
    );
}

#[test]
fn a_question_waits_for_its_page_and_is_answered_when_it_loads() {
    let mut app = artist_page_app("remote-question");
    let ctx = egui::Context::default();
    app.artist_pages.get_mut("art").unwrap().top_tracks = Loadable::Loading;
    app.remote_command(
        PhoneIncoming {
            id: "1".into(),
            command: PhoneCommand::Artist { id: "art".into() },
        },
        &ctx,
    );
    assert_eq!(app.phone.waiting.len(), 1, "still loading");

    app.settings.phone_linked = true;
    app.remote_tick(&ctx);
    assert_eq!(app.phone.waiting.len(), 1);

    app.artist_pages.get_mut("art").unwrap().top_tracks = Loadable::Loaded(Vec::new());
    app.remote_tick(&ctx);
    assert!(app.phone.waiting.is_empty(), "answered");
}

fn on_the_phone(playing: bool, position_ms: f64) -> crate::friends::PhonePlaying {
    crate::friends::PhonePlaying {
        song: Some(crate::friends::PhoneSong {
            uri: "soundcloud:track:7".into(),
            name: "On the phone".into(),
            artists: vec!["Someone".into()],
            image: None,
            duration_ms: 180_000.0,
        }),
        playing,
        position_ms,
        device: Some("This phone".into()),
        at: 0,
    }
}

#[test]
fn what_the_phone_plays_shows_here_and_goes_when_it_stops() {
    let mut app = artist_page_app("remote-there");
    app.remote_event(crate::backend::remote::RemoteEvent::Phone(Some((
        on_the_phone(true, 10_000.0),
        2_000,
    ))));
    let there = app.phone.there.clone().expect("shown");
    assert!(there.playing());
    let at = there.position_ms();
    assert!((12_000..13_000).contains(&at), "counted on from the word: {at}");

    // A look that read an older word than the stream already gave changes nothing.
    app.remote_event(crate::backend::remote::RemoteEvent::Phone(Some((
        on_the_phone(true, 0.0),
        60_000,
    ))));
    assert!(app.phone.there.as_ref().unwrap().position_ms() >= 12_000);

    app.remote_event(crate::backend::remote::RemoteEvent::Phone(None));
    assert!(app.phone.there.is_none(), "stopped there");
    app.remote_event(crate::backend::remote::RemoteEvent::Phone(Some((
        on_the_phone(true, 0.0),
        200_000,
    ))));
    assert!(app.phone.there.is_none(), "a phone long silent is forgotten");
}

#[test]
fn pausing_the_phone_from_here_shows_at_once_and_a_late_word_does_not_undo_it() {
    let mut app = artist_page_app("remote-there-pause");
    app.remote_event(crate::backend::remote::RemoteEvent::Phone(Some((
        on_the_phone(true, 0.0),
        0,
    ))));
    app.phone_control("toggle");
    assert!(!app.phone.there.as_ref().unwrap().playing(), "paused at once");
    // The phone hadn't heard yet: its word still says playing.
    app.remote_event(crate::backend::remote::RemoteEvent::Phone(Some((
        on_the_phone(true, 500.0),
        0,
    ))));
    assert!(!app.phone.there.as_ref().unwrap().playing(), "held");
    app.remote_event(crate::backend::remote::RemoteEvent::Phone(Some((
        on_the_phone(false, 900.0),
        0,
    ))));
    let there = app.phone.there.as_ref().unwrap();
    assert!(!there.playing(), "the phone agrees");
    assert_eq!(there.position_ms(), 900, "paused, the place stays put");
}

#[test]
fn playing_the_phones_song_here_takes_it_over() {
    let mut app = artist_page_app("remote-there-take");
    app.remote_event(crate::backend::remote::RemoteEvent::Phone(Some((
        on_the_phone(true, 30_000.0),
        0,
    ))));
    app.take_from_phone();
    assert!(app.phone.there.is_none(), "it's here now, not there");
}

#[test]
fn a_phone_sends_a_song_here_to_download() {
    let incoming: PhoneIncoming = serde_json::from_str(
        r#"{"id":"1","kind":"download","uri":"spotify:track:a","name":"Holding","artists":["slayr"],"duration_ms":181000,"image":null}"#,
    )
    .unwrap();
    let Some(Action::Download(items)) = super::remote::phone_action(&incoming.command) else {
        panic!("it should download");
    };
    let [PlayableItem::Track(track)] = items.as_slice() else {
        panic!("one song");
    };
    assert_eq!(track.uri, "spotify:track:a");
    assert_eq!(track.name, "Holding");
    assert_eq!(track.artists[0].name, "slayr");
    assert_eq!(track.duration_ms, 181_000);
    assert!(crate::downloads::Source::from_item(&items[0]).is_some());
}

#[test]
fn a_download_keeps_only_a_soundcloud_songs_own_page() {
    let download = |uri: &str, link: &str| {
        super::remote::phone_action(&PhoneCommand::Download {
            uri: uri.into(),
            name: "Song".into(),
            artists: vec!["Someone".into()],
            duration_ms: 1,
            link: Some(link.into()),
        })
    };
    let link_of = |action: Option<Action>| match action {
        Some(Action::Download(items)) => match &items[0] {
            PlayableItem::Track(track) => track.external_urls.spotify.clone(),
            _ => None,
        },
        _ => panic!("it should download"),
    };
    assert_eq!(
        link_of(download(
            "soundcloud:track:7",
            "https://soundcloud.com/someone/song"
        )),
        Some("https://soundcloud.com/someone/song".to_owned())
    );
    assert_eq!(
        link_of(download("soundcloud:track:7", "https://evil.example/song")),
        None
    );
    assert!(download("https://evil.example/x", "https://soundcloud.com/a/b").is_none());
}

#[test]
fn a_phone_can_only_pre_save_a_resolved_album() {
    let action = |uri: &str| {
        super::remote::phone_action(&PhoneCommand::Presave {
            uri: uri.into(),
            saved: true,
        })
    };
    let album = "spotify:album:5BYKG4MHfySZthe2W6r7n4";
    assert!(matches!(
        action(album),
        Some(Action::SetUpcomingSaved { album_uri, saved: true }) if album_uri == album
    ));
    assert!(action("spotify:prerelease:0kRaNkRxpO16BjxJU0IQAL").is_none());
    assert!(action("spotify:track:5BYKG4MHfySZthe2W6r7n4").is_none());
    assert!(action("spotify:album:short").is_none());
    assert!(action("https://evil.example/album/5BYKG4MHfySZthe2W6r7n4").is_none());
}

#[test]
fn an_artist_answer_carries_the_upcoming_release() {
    let mut app = artist_page_app("remote-upcoming");
    app.artist_pages.get_mut("art").unwrap().upcoming =
        Loadable::Loaded(vec![crate::session_reads::Upcoming {
            uri: "spotify:prerelease:0kRaNkRxpO16BjxJU0IQAL".into(),
            album_uri: Some("spotify:album:5BYKG4MHfySZthe2W6r7n4".into()),
            name: "Grand Theft Auto VI: The Album".into(),
            image: Some("https://i.scdn.co/image/ab67616d00001e02197fc499df23a7957415dffe".into()),
            release: Some(1_795_129_200),
            kind: Some("album".into()),
            saved: Some(false),
            ..Default::default()
        }]);
    let answer = app
        .remote_answer(&PhoneCommand::Artist { id: "art".into() })
        .expect("answered");
    assert_eq!(
        answer["upcoming"][0]["uri"],
        "spotify:prerelease:0kRaNkRxpO16BjxJU0IQAL"
    );
    assert_eq!(
        answer["upcoming"][0]["album_uri"],
        "spotify:album:5BYKG4MHfySZthe2W6r7n4"
    );
    assert_eq!(answer["upcoming"][0]["name"], "Grand Theft Auto VI: The Album");
    assert_eq!(answer["upcoming"][0]["release"], 1_795_129_200);
    assert_eq!(answer["upcoming"][0]["kind"], "album");
    assert_eq!(answer["upcoming"][0]["saved"], false);
    app.backend.shutdown();
}

#[test]
fn the_phone_is_told_this_computer_can_download() {
    let app = test_app("remote-can");
    let told = serde_json::to_value(app.remote_snapshot()).unwrap();
    assert_eq!(
        told["can"],
        serde_json::json!(["download", "presave", "add_to_playlist"])
    );
}

#[test]
fn the_phones_home_carries_new_releases_as_what_they_are() {
    let mut app = test_app("remote-releases");
    app.for_you = Loadable::Loaded(crate::for_you::ForYou {
        projects: vec![crate::for_you::Project {
            uri: "spotify:album:abc".into(),
            name: "Fragments".into(),
            artists: vec![ArtistRef {
                name: "Bonobo".into(),
                ..Default::default()
            }],
            images: Vec::new(),
            kind: crate::for_you::Kind::Album,
            released: "2026-11-01".into(),
            songs: Some(12),
            track: None,
            reason: "Out 10 days ago".into(),
        }],
        ..Default::default()
    });
    let answer = app.remote_answer(&PhoneCommand::Home).unwrap();
    let release = &answer["releases"][0];
    assert_eq!(release["id"], "abc");
    assert_eq!(release["kind"], "album");
    assert_eq!(release["songs"], 12);
    assert_eq!(release["artists"][0], "Bonobo");
    assert!(release["song"].is_null());
}

#[test]
fn a_phone_adds_a_song_only_to_a_playlist_this_computer_offered() {
    let mut app = test_app("remote-add-to-playlist");
    let ctx = egui::Context::default();
    app.mixed_playlists.create("Road".into(), String::new());
    let target = app.mixed_playlists.playlists[0].uri();
    let library = app.remote_answer(&PhoneCommand::Library).unwrap();
    assert_eq!(library["editable"][0]["id"], target.as_str());
    assert_eq!(library["editable"][0]["mixed"], true);

    let add = |target: &str, uri: &str| PhoneIncoming {
        id: "1".into(),
        command: PhoneCommand::AddToPlaylist {
            target: target.into(),
            uri: uri.into(),
            name: "Holding".into(),
            artists: vec!["slayr".into()],
            duration_ms: 154_000,
            image: None,
        },
    };
    app.remote_command(add(&target, "spotify:track:a"), &ctx);
    assert!(app.actions.iter().any(|action| matches!(action,
        Action::ConfirmAddToPlaylist { playlist_id, items, .. }
            if *playlist_id == target && items[0].uri() == "spotify:track:a")));
    app.actions.clear();
    app.remote_command(add("spotify:playlist:not-mine", "spotify:track:a"), &ctx);
    app.remote_command(add(&target, "https://evil.example/x"), &ctx);
    assert!(!app.actions.iter().any(|action| matches!(action, Action::ConfirmAddToPlaylist { .. })));
}

#[test]
fn a_phone_asks_which_songs_are_liked() {
    let mut app = test_app("remote-saved");
    app.saved.insert("spotify:track:liked".into(), true);
    app.saved.insert("spotify:track:not".into(), false);
    let ask = |uris: &[&str]| PhoneCommand::Saved {
        uris: uris.iter().map(|uri| (*uri).to_owned()).collect(),
    };
    let answer = app
        .remote_answer(&ask(&["spotify:track:liked", "spotify:track:not"]))
        .unwrap();
    assert_eq!(answer["saved"]["spotify:track:liked"], true);
    assert_eq!(answer["saved"]["spotify:track:not"], false);
    assert!(super::remote::fair_question(&ask(&["spotify:track:liked"])));
    assert!(!super::remote::fair_question(&ask(&["https://evil.example/x"])));
}
