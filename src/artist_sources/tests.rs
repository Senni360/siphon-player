use super::*;

fn track(name: &str) -> Track {
    Track {
        name: name.to_owned(),
        uri: format!("soundcloud:track:{name}"),
        duration_ms: 180_000,
        ..Default::default()
    }
}

fn candidate(name: &str, followers: u64, uploads: &[&str]) -> Candidate {
    Candidate {
        account: Account {
            uri: format!("soundcloud:user:{name}"),
            name: name.to_owned(),
            followers: Some(followers),
            ..Default::default()
        },
        uploads: uploads.iter().map(|title| track(title)).collect(),
    }
}

#[test]
fn account_names_match_past_what_labels_add() {
    assert_eq!(name_match("Slayer", "SLAYER"), 1.0);
    assert_eq!(name_match("Slayer", "Slayer Official"), 1.0);
    assert_eq!(name_match("Slayer", "SlayerVEVO"), 1.0);
    assert_eq!(name_match("Wa$ Era", "Was Era"), 1.0);
    assert_eq!(name_match("Beyoncé", "Beyonce"), 1.0);
    assert_eq!(name_match("Slayer", "Slayer Fans"), 0.6);
    assert_eq!(name_match("Slayer", "Buffy the Vampire Slayer Fans"), 0.0);
    assert_eq!(name_match("Slayer", "Dragon"), 0.0);
    assert_eq!(name_match("The Music", "The Music"), 1.0);
    assert_eq!(name_match("The Music", "Music"), 0.0);
}

#[test]
fn a_title_comes_down_to_the_song() {
    let words = |title| song_words(title, "Slayer").join(" ");
    assert_eq!(
        words("Slayer - Raining Blood (Official Video)"),
        "raining blood"
    );
    assert_eq!(words("Raining Blood - Slayer [HD]"), "raining blood");
    assert_eq!(words("SLAYER | Raining Blood"), "raining blood");
    assert_eq!(words("Raining Blood ft. Someone"), "raining blood");
    assert_eq!(words("Raining Blood (prod. Someone)"), "raining blood");
    assert_eq!(words("Raining Blood Official Music Video"), "raining blood");
    assert_eq!(words("Raining Blood (Live)"), "raining blood live");
    assert_eq!(words("Don't Stop"), "dont stop");
}

#[test]
fn titles_are_sorted_into_songs_edits_and_the_rest() {
    assert_eq!(kind_of("Raining Blood"), Kind::Song);
    assert_eq!(kind_of("Raining Blood (Live)"), Kind::Song);
    assert_eq!(kind_of("Raining Blood (sped up)"), Kind::Edit);
    assert_eq!(kind_of("Raining Blood - slowed + reverb"), Kind::Edit);
    assert_eq!(kind_of("Raining Blood Remix"), Kind::Edit);
    assert_eq!(kind_of("Interview with Slayer"), Kind::NotMusic);
    assert_eq!(kind_of("Slayer type beat"), Kind::NotMusic);
    assert_eq!(kind_of("Behind the scenes of the tour"), Kind::NotMusic);
    assert_eq!(kind_of("Up and Sped"), Kind::Song);
}

#[test]
fn an_upload_of_a_spotify_song_is_recognised_and_a_new_one_is_not() {
    let catalogue = Catalogue::new(
        "Slayer",
        [
            "Raining Blood",
            "Angel of Death - Remastered",
            "South of Heaven",
        ],
    );
    assert!(catalogue.has("Slayer - Raining Blood (Official Video)"));
    assert!(catalogue.has("Angel of Death"));
    assert!(catalogue.has("Slayer - South of Heaven (HD Version)"));
    assert!(!catalogue.has("Raining Blood (sped up)"));
    assert!(!catalogue.has("Some Song Nobody Released"));
    assert!(!catalogue.has("Heaven"));
    assert!(!Catalogue::new("Slayer", ["Heaven"]).has("South of Heaven"));
}

#[test]
fn a_catalogue_needs_a_whole_short_title_to_match() {
    let catalogue = Catalogue::new("Artist", ["Run"]);
    assert!(catalogue.has("Artist - Run"));
    assert!(!catalogue.has("Run Away Now"));
}

#[test]
fn an_account_sharing_songs_is_certain_and_a_namesake_is_not() {
    let catalogue = Catalogue::new("Slayer", ["Raining Blood", "South of Heaven", "Seasons"]);
    let (found, others) = pick(
        "Slayer",
        &catalogue,
        vec![
            candidate("Slayer", 900_000, &["Deep House Mix 12", "Summer Vibes"]),
            candidate("SLAYER", 40_000, &["Raining Blood", "South of Heaven"]),
        ],
    );
    let (found, confidence) = found.unwrap();
    assert_eq!(found.account.followers, Some(40_000));
    assert_eq!(confidence, Confidence::Sure);
    assert_eq!(others.len(), 1);
}

#[test]
fn with_nothing_shared_and_rivals_the_listener_is_asked() {
    let catalogue = Catalogue::new("Slayr", ["Song One", "Song Two"]);
    let (found, others) = pick(
        "Slayr",
        &catalogue,
        vec![
            candidate("slayr", 3_000, &["Unreleased Thing"]),
            candidate("SLAYR", 2_000, &["Another Thing"]),
        ],
    );
    assert!(found.is_none());
    assert_eq!(others.len(), 2);
}

#[test]
fn an_account_far_bigger_than_its_namesakes_is_likely_even_with_all_new_songs() {
    let catalogue = Catalogue::new("Slayr", ["Song One"]);
    let (found, _) = pick(
        "Slayr",
        &catalogue,
        vec![
            candidate("slayr", 50_000, &["Unreleased Thing"]),
            candidate("slayr 2", 100, &["Another Thing"]),
            candidate("SLAYR", 900, &["Third Thing"]),
        ],
    );
    let (found, confidence) = found.unwrap();
    assert_eq!(found.account.followers, Some(50_000));
    assert_eq!(confidence, Confidence::Likely);
}

#[test]
fn a_small_namesake_alone_is_not_taken_for_the_artist() {
    let catalogue = Catalogue::new("Slayr", ["Song One"]);
    let (found, others) = pick(
        "Slayr",
        &catalogue,
        vec![candidate("slayr", 40, &["Bedroom Demo"])],
    );
    assert!(found.is_none());
    assert_eq!(others.len(), 1);
}

#[test]
fn an_empty_account_is_never_chosen_by_size_alone() {
    let catalogue = Catalogue::new("Slayr", ["Song One"]);
    let (found, _) = pick("Slayr", &catalogue, vec![candidate("slayr", 500_000, &[])]);
    assert!(found.is_none());
}

#[test]
fn uploads_split_into_what_spotify_lacks_and_edits() {
    let catalogue = Catalogue::new("Slayer", ["Raining Blood"]);
    let mut popular = track("Scrapped Song");
    popular.play_count = Some(500);
    let (exclusives, edits, on_spotify) = sort_uploads(
        "Slayer",
        &catalogue,
        vec![
            track("Slayer - Raining Blood (Official Video)"),
            track("Demo Song"),
            popular,
            track("Raining Blood (sped up)"),
            track("Interview with Slayer"),
            track("Demo Song (Official Audio)"),
        ],
        Side::SoundCloud,
    );
    let names: Vec<_> = exclusives.iter().map(|track| track.name.as_str()).collect();
    assert_eq!(names, ["Scrapped Song", "Demo Song"]);
    assert_eq!(edits.len(), 1);
    assert_eq!(on_spotify, 1);
}

#[test]
fn a_channel_posts_more_than_songs_and_only_songs_are_kept() {
    let video = |name: &str, seconds: u32| Track {
        name: name.to_owned(),
        duration_ms: seconds * 1000,
        ..Default::default()
    };
    let (kept, _, _) = sort_uploads(
        "Slayer",
        &Catalogue::default(),
        vec![
            video("SLAYER - I Hate You (OFFICIAL MUSIC VIDEO)", 200),
            video("SLAYER - Repentless (Live At The Forum)", 300),
            video("SLAYER - Early Days: Episode 1", 500),
            video("SLAYER - The Killogy (SHORT FILM TEASER #2)", 100),
            video("Slayer - Giveaway: Enter To Win", 60),
            video("Coming Fall 2019", 50),
            video("The Killogy Documentary: Part One", 600),
            video("Plain Named Song", 180),
        ],
        Side::YouTube,
    );
    let names: Vec<_> = kept.iter().map(|track| track.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "SLAYER - I Hate You (OFFICIAL MUSIC VIDEO)",
            "SLAYER - Repentless (Live At The Forum)",
            "Plain Named Song",
        ]
    );
}

#[test]
fn youtube_leaves_out_shorts_and_long_uploads_and_soundcloud_keeps_them() {
    let catalogue = Catalogue::default();
    let mut short = track("Clip");
    short.duration_ms = 20_000;
    let mut long = track("Whole Show");
    long.duration_ms = 3_600_000;
    let uploads = vec![short, long, track("Song")];
    let (youtube, _, _) = sort_uploads("A", &catalogue, uploads.clone(), Side::YouTube);
    assert_eq!(youtube.len(), 1);
    let (soundcloud, _, _) = sort_uploads("A", &catalogue, uploads, Side::SoundCloud);
    assert_eq!(soundcloud.len(), 3);
}

#[test]
fn an_edit_search_keeps_only_edits_of_this_artist() {
    let catalogue = Catalogue::new("Slayer", ["Raining Blood"]);
    let edits = sort_edits(
        "Slayer",
        &catalogue,
        vec![
            track("slayer - raining blood (sped up)"),
            track("SLAYER - Raining Blood (Sped Up)"),
            track("another artist remix"),
            track("slayer live at wembley"),
            track("Demon Slayer x Broken Heart (Cinsky Edit)"),
            track("Demon Slayer - Raining Blood Edit"),
            track("Slayer edit compilation"),
            track("raining blood slayer edit"),
        ],
        Side::SoundCloud,
    );
    let names: Vec<_> = edits.iter().map(|track| track.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "slayer - raining blood (sped up)",
            "raining blood slayer edit"
        ]
    );
}

#[test]
fn the_same_song_on_two_sources_appears_once_from_the_first() {
    let mut video = track("Demo Song (Official Video)");
    video.uri = "youtube:video:x".into();
    let audio = [track("Demo Song")];
    let videos = [video];
    let all = merged("A", [&audio[..], &videos[..]]);
    assert_eq!(all.len(), 1);
    assert!(all[0].uri.starts_with("soundcloud"));
}

#[test]
fn links_lead_back_from_accounts_but_not_from_a_refusal() {
    let account = Account {
        uri: "soundcloud:user:1".into(),
        ..Default::default()
    };
    let mut links = Links::default();
    links.artists.insert(
        "spotify-id".into(),
        Linked {
            name: "Slayer".into(),
            soundcloud: Some(Link::Chosen(account)),
            youtube: Some(Link::NotThere),
            alternates: vec![Account {
                uri: "soundcloud:user:2".into(),
                ..Default::default()
            }],
        },
    );
    let back = links.back();
    assert_eq!(back.len(), 2);
    assert_eq!(back["soundcloud:user:1"].1, "Slayer");
    assert_eq!(back["soundcloud:user:2"].1, "Slayer");
}

#[test]
fn an_older_links_file_still_reads() {
    let links: Links = serde_json::from_str(r#"{"artists":{"a":{"name":"Slayer"}}}"#).unwrap();
    assert_eq!(links.artists["a"].soundcloud, None);
    let empty: Links = serde_json::from_str("{}").unwrap();
    assert!(empty.artists.is_empty());
}

fn posted(name: &str, by: &str, plays: u64) -> Track {
    Track {
        name: name.to_owned(),
        uri: format!("soundcloud:track:{name}"),
        duration_ms: 120_000,
        play_count: Some(plays),
        artists: vec![crate::api::models::ArtistRef {
            id: None,
            name: by.to_owned(),
            uri: Some(format!("soundcloud:user:{by}")),
        }],
        ..Default::default()
    }
}

#[test]
fn a_title_names_an_artist_only_when_the_name_stands_alone() {
    assert!(names_artist("slayr - know the words (snippet)", "slayr"));
    assert!(names_artist("SLAYR - KILL ME SLOW (2026 LEAK)", "Slayr"));
    assert!(names_artist("slayr-my oh my (bloodluxe snippet)", "slayr"));
    assert!(names_artist("Yeat & Slayr - Luh Star", "slayr"));
    assert!(names_artist("(LEAK) Che - GO BACK (ft. slayr)", "slayr"));
    assert!(names_artist("Wrap It Up (slayr unreleased)", "slayr"));
    assert!(names_artist("Freeze Tag - slayr leak", "slayr"));
    assert!(!names_artist("Demon Slayer - Hashira Leak", "Slayer"));
    assert!(!names_artist("SLAYER77 - February (leak)", "Slayer"));
    assert!(names_artist("Wa$ Era - thing", "Wa$ Era"));
}

#[test]
fn unreleased_songs_are_kept_once_with_full_versions_first() {
    let catalogue = Catalogue::new("slayr", ["Sloppy Joe"]);
    let kept = sort_unreleased(
        "slayr",
        &catalogue,
        vec![
            posted(
                "slayr - promise unreleased (Fabula Nova Snippet)",
                "zor",
                29_400,
            ),
            posted("Slayr - Promise (leak)", "other", 500),
            posted("slayr - Promise BEST VERSION (unreleased)", "third", 2),
            posted("slayr - know the words (snippet)", "srcfiles", 6_209),
            posted("SLAYR - KILL ME SLOW (2026 LEAK)", "osiris", 32_129),
            posted("slayr - Sloppy Joe (leak)", "someone", 90_000),
            posted("nothing", "conqr", 1_993),
            posted("Demon Slayer leak", "anime", 9_000_000),
        ],
        &[],
        Side::SoundCloud,
    );
    let names: Vec<_> = kept.iter().map(|track| track.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "SLAYR - KILL ME SLOW (2026 LEAK)",
            "Slayr - Promise (leak)",
            "slayr - know the words (snippet)",
        ]
    );
}

#[test]
fn an_archive_accounts_uploads_count_with_no_label_on_them() {
    assert!(is_archive("slayr archives", "slayr"));
    assert!(is_archive("$layrarchives", "slayr"));
    assert!(!is_archive("slayr", "slayr"));
    assert!(!is_archive("savvy luvs slayr", "slayr"));
    assert!(!is_archive("UDG Unreleased", "slayr"));
    let kept = sort_unreleased(
        "slayr",
        &Catalogue::default(),
        vec![
            posted("Wrap It Up", "archive", 4_567),
            posted("Other", "else", 9),
        ],
        &["soundcloud:user:archive".to_owned()],
        Side::SoundCloud,
    );
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].name, "Wrap It Up");
}

#[test]
fn a_producer_credit_is_found_alone_or_shared_and_only_for_that_name() {
    assert!(credits_producer("slayr - toxic (prod. waera)", "waera"));
    assert!(credits_producer("Song [prod by waera]", "waera"));
    assert!(credits_producer("Song (prod. someone + waera)", "waera"));
    assert!(credits_producer("Song produced by waera", "waera"));
    assert!(!credits_producer("waera - song (prod. someone)", "waera"));
    assert!(!credits_producer("Prod Song", "waera"));
}

#[test]
fn produced_songs_leave_out_their_own_and_type_beats() {
    let kept = sort_produced(
        "waera",
        vec![
            posted("slayr - toxic (prod. waera)", "slayr", 100),
            posted("slayr - toxic (prod waera)", "fan", 900),
            posted("my own thing (prod. waera)", "waera", 5_000),
            posted("slayr type beat (prod. waera)", "beats", 7_000),
            posted("yeat - song (prod. waera) (snippet)", "fan", 3),
            posted("ken - other (prod waera)", "ken", 300),
        ],
        Side::SoundCloud,
    );
    let names: Vec<_> = kept.iter().map(|track| track.name.as_str()).collect();
    assert_eq!(
        names,
        ["ken - other (prod waera)", "slayr - toxic (prod. waera)"]
    );
    assert!(enough_produced(kept).is_empty(), "two is a stray credit");
}

#[test]
fn what_the_artist_posted_is_not_listed_again_as_unreleased() {
    let kept = without_songs(
        "slayr",
        vec![
            posted("slayr - Toxic (leak)", "fan", 1),
            posted("slayr - New (leak)", "fan", 1),
        ],
        &[posted("Toxic", "slayr", 1)],
    );
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].name, "slayr - New (leak)");
}

#[test]
fn an_upload_reads_as_a_song_by_who_the_title_says() {
    let read = |name: &str, by: &str| {
        let track = tidied(posted(name, by, 1), "slayr");
        (
            track.name.clone(),
            track.artists[0].name.clone(),
            track.artists[0].uri.is_some(),
        )
    };
    assert_eq!(
        read("slayr - Holding (OFFICIAL MUSIC VIDEO)", "zor"),
        ("Holding".to_owned(), "slayr".to_owned(), false)
    );
    assert_eq!(
        read("Run It Back - slayr", "zor"),
        ("Run It Back".to_owned(), "slayr".to_owned(), false)
    );
    assert_eq!(
        read("slayr - toxic [prod. slayr+waera]", "x"),
        (
            "toxic [prod. slayr+waera]".to_owned(),
            "slayr".to_owned(),
            false
        )
    );
    assert_eq!(
        read("kuru - gracious prod waera", "kuru"),
        ("gracious prod waera".to_owned(), "kuru".to_owned(), false)
    );
    assert_eq!(
        read("Slayr- Promise (Fabula)", "zor"),
        ("Promise (Fabula)".to_owned(), "Slayr".to_owned(), false)
    );
    assert_eq!(
        read("(NO AI) slayr - Bird Brain", "zor"),
        ("Bird Brain".to_owned(), "slayr".to_owned(), false)
    );
    assert_eq!(
        read("Black T- Shirt", "zor"),
        ("Black T- Shirt".to_owned(), "zor".to_owned(), true)
    );
    // Nothing to go on: the song stays by whoever posted it, and leads there.
    assert_eq!(
        read("Sloppy Joe", "slayr"),
        ("Sloppy Joe".to_owned(), "slayr".to_owned(), true)
    );
    assert_eq!(
        read("Some Long Title With Many Words In It - Part Two", "x"),
        (
            "Some Long Title With Many Words In It - Part Two".to_owned(),
            "x".to_owned(),
            true
        )
    );
}

#[test]
fn only_sets_spotify_lacks_are_listed_with_albums_first() {
    let set = |id: u64, title: &str, is_album: bool, songs: u32, public: bool| models::Playlist {
        id,
        title: title.to_owned(),
        description: None,
        artwork_url: None,
        permalink_url: None,
        user: None,
        public: Some(public),
        is_album: Some(is_album),
        track_count: Some(songs),
        tracks: Vec::new(),
    };
    let catalogue = Catalogue::new("slayr", ["FABULA NOVA"]);
    let kept = sort_sets(
        &catalogue,
        vec![
            set(1, "old loosies", false, 8, true),
            set(2, "FABULA NOVA", true, 14, true),
            set(3, "lost tape", true, 6, true),
            set(6, "Lost Tape", true, 6, true),
            set(4, "one song", false, 1, true),
            set(5, "private", true, 9, false),
        ],
    );
    let names: Vec<_> = kept.iter().map(|set| set.name.as_str()).collect();
    assert_eq!(names, ["lost tape", "old loosies"]);
}

#[test]
fn an_older_saved_finding_without_the_newer_lists_still_reads() {
    let found: Finding = serde_json::from_str(r#"{"on_spotify":3}"#).unwrap();
    assert_eq!(found.on_spotify, 3);
    assert!(found.unreleased.is_empty());
    assert!(found.produced.is_empty());
}

#[test]
fn a_bio_names_a_handle_only_as_a_handle_or_a_link() {
    assert!(bio_names(
        "https://www.instagram.com/countdragons/",
        "countdragons"
    ));
    assert!(bio_names("ig: @countdragons", "CountDragons"));
    assert!(bio_names(
        "instagram.com/countdragons?hl=en",
        "countdragons"
    ));
    assert!(!bio_names(
        "I love count dragons and countdragons music",
        "countdragons"
    ));
    assert!(!bio_names(
        "https://instagram.com/countdragons2",
        "countdragons"
    ));
    assert!(!bio_names("@abc", "abc"), "too short to mean anything");
    assert!(!bio_names("", "countdragons"));
}

fn sized(uri: &str, name: &str, followers: u64) -> Account {
    Account {
        uri: uri.to_owned(),
        name: name.to_owned(),
        followers: Some(followers),
        ..Default::default()
    }
}

#[test]
fn only_the_artists_own_bio_makes_an_account_theirs() {
    let main = sized("soundcloud:user:1", "slayr", 83_000);
    let alt = sized("soundcloud:user:2", "countdragons", 10_000);
    let tied = |named_in_bio, follows_main, reposts| AccountTies {
        named_in_bio,
        follows_main,
        reposts,
    };
    assert!(is_alternate(&main, &alt, tied(true, true, 5)));
    assert!(is_alternate(&main, &alt, tied(true, true, 0)));
    assert!(is_alternate(&main, &alt, tied(true, false, 2)));
    // A close collaborator does all of this, except be named in the bio.
    assert!(!is_alternate(&main, &alt, tied(false, true, 50)));
    // A bio can name anyone.
    assert!(!is_alternate(&main, &alt, tied(true, false, 1)));
    let bigger = sized("soundcloud:user:3", "famous", 900_000);
    assert!(!is_alternate(&main, &bigger, tied(true, true, 5)));
    let label = sized("soundcloud:user:4", "Dragon Records", 900);
    assert!(!is_alternate(&main, &label, tied(true, true, 5)));
    assert!(!is_alternate(&main, &main, tied(true, true, 5)));
}
