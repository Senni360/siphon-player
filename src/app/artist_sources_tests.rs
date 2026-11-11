// Included in app::tests so these exercise the existing offline app harness.
fn artist_page_app(name: &str) -> App {
    let mut app = test_app(name);
    app.network_reachable = true;
    let albums = PagedList {
        loaded_once: true,
        ..Default::default()
    };
    app.artist_pages.insert(
        "art".into(),
        ArtistPage {
            artist: Loadable::Loaded(Artist {
                name: "Slayr".into(),
                ..Default::default()
            }),
            top_tracks: Loadable::Loaded(Vec::new()),
            albums: [("album".to_string(), albums)].into(),
            upcoming: Loadable::Loaded(Vec::new()),
            ..Default::default()
        },
    );
    app
}

fn upload(name: &str, uri: &str) -> Track {
    Track {
        name: name.into(),
        uri: uri.into(),
        ..Default::default()
    }
}

fn account(uri: &str) -> crate::artist_sources::Account {
    crate::artist_sources::Account {
        uri: uri.into(),
        name: "Slayr".into(),
        ..Default::default()
    }
}

#[test]
fn an_artist_page_asks_once_and_only_when_it_knows_what_spotify_has() {
    use crate::artist_sources::Side;
    let mut app = artist_page_app("artist-sources-asks");
    app.artist_pages.get_mut("art").unwrap().top_tracks = Loadable::Loading;
    app.ask_artist_sources("art");
    assert_eq!(app.artist_pages["art"].sources.asks, (0, 0));

    app.artist_pages.get_mut("art").unwrap().top_tracks = Loadable::Loaded(Vec::new());
    app.ask_artist_sources("art");
    let sources = &app.artist_pages["art"].sources;
    assert_eq!(sources.asks, (1, 1));
    assert_eq!(sources.soundcloud, Loadable::Loading);
    assert!(sources.looking());

    app.artist_source_found(
        "art".into(),
        1,
        Side::SoundCloud,
        crate::artist_sources::Finding::default(),
    );
    app.ask_artist_sources("art");
    assert_eq!(app.artist_pages["art"].sources.asks, (1, 1), "asked once");
}

#[test]
fn both_sources_make_one_list_without_a_song_twice() {
    use crate::artist_sources::{Finding, Side};
    let mut app = artist_page_app("artist-sources-merge");
    app.ask_artist_sources("art");
    app.artist_source_found(
        "art".into(),
        1,
        Side::YouTube,
        Finding {
            exclusives: vec![
                upload("slayr - Sloppy Joe (OFFICIAL MUSIC VIDEO)", "youtube:video:a"),
                upload("slayr - Brain Fog", "youtube:video:b"),
            ],
            ..Default::default()
        },
    );
    assert_eq!(app.artist_pages["art"].sources.songs.len(), 2);
    app.artist_source_found(
        "art".into(),
        1,
        Side::SoundCloud,
        Finding {
            exclusives: vec![upload("Sloppy Joe", "soundcloud:track:1")],
            ..Default::default()
        },
    );
    let sources = &app.artist_pages["art"].sources;
    assert!(!sources.looking());
    let uris: Vec<_> = sources.songs.iter().map(|track| track.uri.as_str()).collect();
    assert_eq!(uris, ["soundcloud:track:1", "youtube:video:b"]);
    assert!(
        app.track_cache.contains_key("soundcloud:track:1"),
        "a row can play what it shows"
    );
}

#[test]
fn a_choice_shows_at_once_and_a_late_answer_cannot_undo_it() {
    use crate::artist_sources::{Confidence, Finding, Side};
    use crate::model::ArtistChoice;
    let mut app = artist_page_app("artist-sources-choice");
    let ctx = egui::Context::default();
    app.ask_artist_sources("art");
    app.artist_source_found(
        "art".into(),
        1,
        Side::SoundCloud,
        Finding {
            candidates: vec![account("soundcloud:user:1"), account("soundcloud:user:2")],
            ..Default::default()
        },
    );
    app.apply(
        Action::ChooseArtistAccount {
            artist_id: "art".into(),
            side: Side::SoundCloud,
            choice: ArtistChoice::Account(account("soundcloud:user:2")),
        },
        &ctx,
    );
    let shown = |app: &App| {
        app.artist_pages["art"]
            .sources
            .soundcloud
            .get()
            .unwrap()
            .clone()
    };
    assert_eq!(shown(&app).account, Some(account("soundcloud:user:2")));
    assert_eq!(shown(&app).confidence, Some(Confidence::Chosen));
    assert_eq!(shown(&app).candidates, [account("soundcloud:user:1")]);
    assert!(app.artist_pages["art"].sources.looking(), "its songs are coming");

    // A search from before the choice, with another account.
    app.artist_source_found(
        "art".into(),
        1,
        Side::SoundCloud,
        Finding {
            account: Some(account("soundcloud:user:1")),
            ..Default::default()
        },
    );
    assert_eq!(shown(&app).account, Some(account("soundcloud:user:2")));

    app.artist_source_found(
        "art".into(),
        2,
        Side::SoundCloud,
        Finding {
            account: Some(account("soundcloud:user:2")),
            confidence: Some(Confidence::Chosen),
            exclusives: vec![upload("Song", "soundcloud:track:9")],
            ..Default::default()
        },
    );
    assert_eq!(app.artist_pages["art"].sources.songs.len(), 1);
    assert!(app.artist_pages["art"].sources.reading.is_empty());

    // YouTube's answer, still on its way, isn't caught up in any of that.
    app.artist_source_found("art".into(), 1, Side::YouTube, Finding::default());
    assert!(!app.artist_pages["art"].sources.looking());
}

#[test]
fn saying_an_artist_is_not_on_a_source_clears_what_was_there() {
    use crate::artist_sources::{Finding, Side};
    use crate::model::ArtistChoice;
    let mut app = artist_page_app("artist-sources-not-there");
    let ctx = egui::Context::default();
    app.ask_artist_sources("art");
    app.artist_source_found(
        "art".into(),
        1,
        Side::YouTube,
        Finding {
            account: Some(account("youtube:channel:UC1")),
            exclusives: vec![upload("Song", "youtube:video:a")],
            ..Default::default()
        },
    );
    app.apply(
        Action::ChooseArtistAccount {
            artist_id: "art".into(),
            side: Side::YouTube,
            choice: ArtistChoice::NotThere,
        },
        &ctx,
    );
    let sources = &app.artist_pages["art"].sources;
    assert!(sources.songs.is_empty());
    assert!(sources.youtube.get().unwrap().not_there);
}
