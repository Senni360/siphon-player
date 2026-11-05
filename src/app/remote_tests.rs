// Included in app::tests so these exercise the existing offline app harness.
use crate::remote::{Command as PhoneCommand, Incoming as PhoneIncoming};

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
