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
    assert!(fair(&PhoneCommand::Source { uri: "soundcloud:user:12".into() }));
    assert!(fair(&PhoneCommand::Source { uri: "youtube:channel:UC1234567890123456789012".into() }));
    assert!(!fair(&PhoneCommand::Source { uri: "https://evil.example".into() }));
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
