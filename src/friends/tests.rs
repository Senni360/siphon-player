use super::*;
use crate::api::models::{ExternalUrls, PlayableItem, Track};
use crate::http::Http;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn item(uri: &str, url: Option<&str>) -> PlayableItem {
    PlayableItem::Track(Track {
        uri: uri.into(),
        name: "Beat".into(),
        duration_ms: 1234,
        external_urls: ExternalUrls {
            spotify: url.map(str::to_owned),
        },
        ..Default::default()
    })
}

#[test]
fn shares_three_sources_without_losing_private_soundcloud_links() {
    let private = "https://soundcloud.com/a/beat/s-private?secret_token=s-private";
    let post = NewPost::from_item(&item("soundcloud:track:123", Some(private))).unwrap();
    assert_eq!(post.url, private);
    assert_eq!(post.source, Source::Soundcloud);
    assert!(NewPost::from_item(&item("soundcloud:track:123", None)).is_err());
    assert_eq!(
        NewPost::from_item(&item("spotify:track:ABC123", None))
            .unwrap()
            .url,
        "https://open.spotify.com/track/ABC123"
    );
    assert_eq!(
        NewPost::from_item(&item("youtube:video:abcdefghijk", None))
            .unwrap()
            .url,
        "https://www.youtube.com/watch?v=abcdefghijk"
    );
    assert!(NewPost::from_item(&item("youtube:video:bad/id", None)).is_err());
    assert!(NewPost::from_item(&item("spotify:local:a:b:c:1", None)).is_err());
    assert!(!format!("{post:?}").contains("s-private"));
    assert!(
        !format!(
            "{:?}",
            Action::Join {
                invite_code: "do-not-print-me".into(),
                display_name: "A".into()
            }
        )
        .contains("do-not-print-me")
    );
}

async fn server(
    responses: Vec<(u16, serde_json::Value)>,
) -> (Client, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap_or("0")
                        .parse::<usize>()
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            requests.push(String::from_utf8(request).unwrap());
            let body = body.to_string();
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
        requests
    });
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    (Client::new(Http::new(http), &base).unwrap(), task)
}

fn post_json() -> serde_json::Value {
    json!({"id":"00000000-0000-4000-8000-000000000001","author_id":"a","author_name":"A","created_at":1,"liked":false,"like_count":0,"url":"https://open.spotify.com/track/ABC123","source":"spotify","title":"Beat","artists":[],"artwork_url":null,"duration_ms":1234,"note":""})
}

#[tokio::test]
async fn client_speaks_the_service_contract_and_never_exposes_error_bodies() {
    let secret = "a".repeat(64);
    let (client, task) = server(vec![
        (201, json!({"user_id":"a","secret":secret})),
        (200, json!({"user_id":"a","display_name":"A"})),
        (201, post_json()),
        (
            200,
            json!({"posts":[post_json()],"next_cursor":"cursor+&="}),
        ),
        (200, json!({"posts":[],"next_cursor":null})),
        (200, json!({"ok":true})),
        (200, json!({"ok":true})),
        (200, json!({"user_id":"a","display_name":"B"})),
        (200, json!({"ok":true})),
        (429, json!({"error":secret})),
        (200, json!({"ok":true})),
    ])
    .await;
    let identity = client.join("test-invite", "A").await.unwrap();
    assert_eq!(client.profile(&identity).await.unwrap().display_name, "A");
    let payload = NewPost::from_item(&item("spotify:track:ABC123", None)).unwrap();
    let post = client.share(&identity, &payload).await.unwrap();
    let feed = client.feed(&identity, None).await.unwrap();
    client
        .feed(&identity, feed.next_cursor.as_deref())
        .await
        .unwrap();
    client.like(&identity, &post.id, true).await.unwrap();
    client.like(&identity, &post.id, false).await.unwrap();
    assert_eq!(
        client.rename(&identity, "B").await.unwrap().display_name,
        "B"
    );
    client.delete(&identity, &post.id).await.unwrap();
    let error = client.feed(&identity, None).await.unwrap_err();
    assert!(!error.contains(&identity.secret));
    client.leave(&identity).await.unwrap();
    let requests = task.await.unwrap();
    assert!(requests[0].starts_with("POST /api/join "));
    assert!(!requests[0].contains("authorization:"));
    assert!(
        requests[1]
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {secret}"))
    );
    assert!(requests[4].contains("cursor=cursor%2B%26%3D"));
    assert!(requests[5].starts_with(&format!("PUT /api/posts/{}/like ", post.id)));
    assert!(requests[6].starts_with(&format!("DELETE /api/posts/{}/like ", post.id)));
    assert!(requests[10].starts_with("DELETE /api/me "));
}

#[tokio::test]
async fn credentials_are_bound_to_service_and_redirects_are_not_followed() {
    let (client, task) = server(vec![(307, json!({"secret":"never display"}))]).await;
    let identity = Identity {
        user_id: "a".into(),
        secret: "a".repeat(64),
        base_url: "https://other.invalid/api".into(),
    };
    assert!(
        client
            .profile(&identity)
            .await
            .unwrap_err()
            .contains("another service")
    );
    let identity = Identity {
        base_url: client.base_url().into(),
        ..identity
    };
    assert!(client.profile(&identity).await.is_err());
    assert_eq!(task.await.unwrap().len(), 1);
    assert!(Client::new(Http::default(), "http://remote.invalid/api").is_err());
    assert!(Client::new(Http::default(), "https://user:secret@remote.invalid/api").is_err());
}

#[test]
fn optimistic_state_does_not_accept_stale_events_after_leave() {
    use super::state::Update;
    let post: Post = serde_json::from_value(post_json()).unwrap();
    let mut model = Model::default();
    model.profile = Some(Profile {
        user_id: "a".into(),
        display_name: "A".into(),
    });
    model.posts.push(post.clone());
    let request = model
        .begin(Action::Like {
            id: post.id.clone(),
            liked: true,
        })
        .unwrap();
    assert!(model.posts[0].liked);
    assert_eq!(model.posts[0].like_count, 1);
    assert!(model.begin(Action::Refresh).is_none());
    assert!(!model.poll_due(true));
    model.receive(Event {
        generation: request.generation,
        result: Ok(Update::Changed),
    });
    assert!(!model.busy);
    let request = model.begin(Action::Refresh).unwrap();
    let leave = model.begin(Action::Leave).unwrap();
    assert!(model.posts.is_empty());
    model.receive(Event {
        generation: request.generation,
        result: Ok(Update::Feed(
            Feed {
                posts: vec![post],
                next_cursor: None,
                members: None,
            },
            false,
        )),
    });
    assert!(model.posts.is_empty());
    model.receive(Event {
        generation: leave.generation,
        result: Ok(Update::Left),
    });
    assert!(!model.busy);
}

#[test]
fn refresh_errors_preserve_feed_and_back_off_while_hidden_never_polls() {
    let mut model = Model::default();
    model.profile = Some(Profile {
        user_id: "a".into(),
        display_name: "A".into(),
    });
    model
        .posts
        .push(serde_json::from_value(post_json()).unwrap());
    assert!(model.poll_due(true));
    assert!(!model.poll_due(false));
    let request = model.begin(Action::Refresh).unwrap();
    model.receive(Event {
        generation: request.generation,
        result: Err("offline".into()),
    });
    assert_eq!(model.posts.len(), 1);
    assert!(!model.poll_due(true));
}

#[test]
fn replacing_identity_clears_private_rows_and_ignores_previous_request_events() {
    use super::state::Update;
    let post: Post = serde_json::from_value(post_json()).unwrap();
    let mut model = Model::default();
    model.profile = Some(Profile {
        user_id: "a".into(),
        display_name: "A".into(),
    });
    model.posts.push(post.clone());
    model.next_cursor = Some("old-private-cursor".into());
    model.recovery_secret = Some("old-secret".into());
    let first = model.begin(Action::Refresh).unwrap();
    model.receive(Event {
        generation: first.generation,
        result: Ok(Update::Feed(Feed::default(), false)),
    });
    model.posts.push(post.clone());
    let join = model
        .begin(Action::Join {
            invite_code: "another-group-invite".into(),
            display_name: "B".into(),
        })
        .unwrap();
    assert!(model.profile.is_none());
    assert!(model.posts.is_empty());
    assert!(model.next_cursor.is_none());
    assert!(model.recovery_secret.is_none());
    assert_ne!(join.generation, first.generation);
    model.receive(Event {
        generation: first.generation,
        result: Ok(Update::Opened(
            Some(Profile {
                user_id: "a".into(),
                display_name: "A".into(),
            }),
            Feed {
                posts: vec![post],
                next_cursor: None,
                members: None,
            },
        )),
    });
    assert!(model.profile.is_none());
    assert!(model.posts.is_empty());
    assert!(model.busy);
}

#[tokio::test]
async fn leave_cancels_a_stalled_join_and_revokes_the_saved_identity() {
    use super::state::Update;
    use crate::credentials::{Slot, Store};
    use crate::paths::AppDirs;
    use std::time::Duration;

    let root = std::env::temp_dir().join(format!(
        "siphon-friends-cancel-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let dirs = AppDirs {
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
    };
    dirs.ensure().unwrap();
    let credentials = Store::in_memory(dirs);
    credentials
        .lease(Slot::Friends)
        .save_secret(b"old-fixture-identity".to_vec())
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api", listener.local_addr().unwrap());
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 4096];
        assert!(stream.read(&mut bytes).await.unwrap() > 0);
        started_tx.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
    let requests = Runtime::spawn_with_client(
        credentials.clone(),
        move |event| {
            let _ = events_tx.send(event);
        },
        move |http| Client::new(http, &base),
    );
    let http = Http::new(reqwest::Client::builder().no_proxy().build().unwrap());
    requests
        .send((
            Request {
                generation: 1,
                action: Action::Join {
                    invite_code: "fixture-invite".into(),
                    display_name: "B".into(),
                },
            },
            http.clone(),
        ))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), started_rx)
        .await
        .unwrap()
        .unwrap();
    requests
        .send((
            Request {
                generation: 2,
                action: Action::Leave,
            },
            http,
        ))
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.generation, 2);
    assert!(matches!(event.result, Ok(Update::Left)));
    assert_eq!(
        credentials
            .lease(Slot::Friends)
            .load_secret()
            .await
            .unwrap(),
        None
    );
    server.abort();
    let _ = server.await;
    drop(requests);
    drop(credentials);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn joining_stops_before_network_when_previous_identity_cannot_be_revoked() {
    use crate::credentials::{Slot, Store};
    use crate::paths::AppDirs;
    use std::time::Duration;

    let root = std::env::temp_dir().join(format!(
        "siphon-friends-revoke-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let dirs = AppDirs {
        config: root.join("config"),
        state: root.join("state"),
        cache: root.join("cache"),
    };
    dirs.ensure().unwrap();
    let marker = dirs.state.join("credential-storage/friends-session.json");
    let credentials = Store::in_memory(dirs);
    credentials
        .lease(Slot::Friends)
        .save_secret(b"previous-identity".to_vec())
        .await
        .unwrap();
    std::fs::remove_file(&marker).unwrap();
    std::fs::create_dir(&marker).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api", listener.local_addr().unwrap());
    let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
    let requests = Runtime::spawn_with_client(
        credentials.clone(),
        move |event| {
            let _ = events_tx.send(event);
        },
        move |http| Client::new(http, &base),
    );
    requests
        .send((
            Request {
                generation: 1,
                action: Action::Join {
                    invite_code: "fixture-invite".into(),
                    display_name: "B".into(),
                },
            },
            Http::new(reqwest::Client::builder().no_proxy().build().unwrap()),
        ))
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), events_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(event.result.is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(10), listener.accept())
            .await
            .is_err()
    );
    drop(requests);
    drop(credentials);
    std::fs::remove_dir_all(root).unwrap();
}

/// Explicitly opt in against the disposable local Node runner, never the
/// production group. Does not load or write any real app credentials.
#[tokio::test]
#[ignore = "requires the local friends service on 127.0.0.1:8787"]
async fn local_service_round_trip() {
    let client = Client::new(
        Http::new(
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        ),
        "http://127.0.0.1:8787/api",
    )
    .unwrap();
    let identity = client
        .join("siphon-local-test-invite", "Rust end-to-end")
        .await
        .unwrap();
    assert_eq!(
        client.profile(&identity).await.unwrap().user_id,
        identity.user_id
    );
    let payload = NewPost::from_item(&item("youtube:video:abcdefghijk", None)).unwrap();
    let post = client.share(&identity, &payload).await.unwrap();
    client.like(&identity, &post.id, true).await.unwrap();
    let page = client.feed(&identity, None).await.unwrap();
    assert!(page.posts.iter().any(|p| p.id == post.id && p.liked));
    // Threads and members.
    let members = client.members(&identity).await.unwrap();
    assert!(members.iter().any(|m| m.user_id == identity.user_id));
    let reply = client.reply(&identity, &post.id, "what key").await.unwrap();
    assert_eq!(reply.author_name, "Rust end-to-end");
    let page = client.feed(&identity, None).await.unwrap();
    let shared = page.posts.iter().find(|p| p.id == post.id).unwrap();
    assert_eq!(shared.replies[0].text, "what key");
    client
        .delete_reply(&identity, &post.id, &reply.id)
        .await
        .unwrap();
    // A share sent to yourself only comes back to you.
    let mut direct = payload.clone();
    direct.to = vec![identity.user_id.clone()];
    let direct = client.share(&identity, &direct).await.unwrap();
    assert_eq!(direct.item.to, vec![identity.user_id.clone()]);
    client.delete(&identity, &direct.id).await.unwrap();
    client.rename(&identity, "Renamed Rust").await.unwrap();
    client.like(&identity, &post.id, false).await.unwrap();
    client.delete(&identity, &post.id).await.unwrap();
    assert!(
        client
            .feed(&identity, None)
            .await
            .unwrap()
            .posts
            .iter()
            .all(|p| p.id != post.id)
    );
    client.leave(&identity).await.unwrap();
    assert!(client.profile(&identity).await.is_err());
}

fn joined_model() -> (Model, Post) {
    let post: Post = serde_json::from_value(post_json()).unwrap();
    let mut model = Model::default();
    model.profile = Some(Profile {
        user_id: "a".into(),
        display_name: "A".into(),
    });
    model.posts.push(post.clone());
    (model, post)
}

#[test]
fn a_write_replaces_a_running_read_and_waits_behind_another_write() {
    use super::state::Update;
    let (mut model, post) = joined_model();
    let read = model.begin(Action::Refresh).unwrap();
    // A heart while the feed is being read goes out at once.
    let like = model
        .begin(Action::Like {
            id: post.id.clone(),
            liked: true,
        })
        .unwrap();
    assert_ne!(like.generation, read.generation);
    // A reply while the heart is in flight is shown and waits its turn.
    assert!(
        model
            .begin(Action::Reply {
                post: post.id.clone(),
                text: "nice".into(),
            })
            .is_none()
    );
    assert_eq!(model.posts[0].replies.len(), 1);
    assert!(
        model.next_request().is_none(),
        "the heart is still in flight"
    );
    model.receive(Event {
        generation: like.generation,
        result: Ok(Update::Changed),
    });
    let reply = model.next_request().expect("the reply goes out next");
    assert!(matches!(reply.action, Action::Reply { .. }));
    // The service's answer takes the optimistic row's place.
    model.receive(Event {
        generation: reply.generation,
        result: Ok(Update::Replied {
            post: post.id.clone(),
            reply: Reply {
                id: "00000000-0000-4000-8000-0000000000aa".into(),
                author_id: "a".into(),
                author_name: "A".into(),
                created_at: 5,
                text: "nice".into(),
            },
        }),
    });
    assert_eq!(model.posts[0].replies.len(), 1);
    assert_eq!(
        model.posts[0].replies[0].id,
        "00000000-0000-4000-8000-0000000000aa"
    );
    assert!(model.next_request().is_none());
}

#[test]
fn a_reply_the_service_refused_is_taken_back() {
    let (mut model, post) = joined_model();
    let request = model
        .begin(Action::Reply {
            post: post.id.clone(),
            text: "lost".into(),
        })
        .unwrap();
    assert_eq!(model.posts[0].replies.len(), 1);
    model.receive(Event {
        generation: request.generation,
        result: Err("offline".into()),
    });
    assert!(model.posts[0].replies.is_empty());
    assert_eq!(model.error.as_deref(), Some("offline"));
}

#[test]
fn unread_counts_only_what_others_wrote_since_the_last_visit() {
    let (mut model, mut post) = joined_model();
    post.created_at = 100;
    post.author_id = "b".into();
    post.replies = vec![
        Reply {
            id: "r1".into(),
            author_id: "a".into(),
            author_name: "A".into(),
            created_at: 200,
            text: "mine".into(),
        },
        Reply {
            id: "r2".into(),
            author_id: "c".into(),
            author_name: "C".into(),
            created_at: 300,
            text: "theirs".into(),
        },
    ];
    model.posts = vec![post];
    assert_eq!(model.unread_since(50), 2);
    assert_eq!(model.unread_since(150), 1);
    assert_eq!(model.unread_since(300), 0);
    assert_eq!(model.newest(), 300);
}

#[test]
fn shares_and_replies_read_back_with_recipients_and_threads() {
    let mut json = post_json();
    json["to"] = serde_json::json!(["00000000-0000-4000-8000-000000000001"]);
    json["replies"] = serde_json::json!([{
        "id": "00000000-0000-4000-8000-000000000002",
        "author_id": "00000000-0000-4000-8000-000000000003",
        "author_name": "Teo",
        "created_at": 7,
        "text": "what key"
    }]);
    let post: Post = serde_json::from_value(json).unwrap();
    assert_eq!(post.item.to.len(), 1);
    assert_eq!(post.replies[0].text, "what key");
    // An open share sends no recipients at all.
    let open = NewPost::from_url(
        "https://youtu.be/dQw4w9WgXcQ".into(),
        super::Source::Youtube,
        "t".into(),
    );
    assert!(serde_json::to_value(&open).unwrap().get("to").is_none());
}

#[test]
fn a_share_the_reader_will_not_care_about_arrives_quietly() {
    let (mut model, _) = joined_model();
    let share = |id: &str, created_at: u64, affinity: Option<f64>, to: &[&str]| {
        let mut json = post_json();
        json["id"] = id.into();
        json["author_id"] = "b".into();
        json["created_at"] = created_at.into();
        json["affinity"] = affinity.map_or(serde_json::Value::Null, Into::into);
        json["to"] = to
            .iter()
            .map(|&id| id.to_owned())
            .collect::<Vec<_>>()
            .into();
        serde_json::from_value::<Post>(json).unwrap()
    };
    let quiet = share("q", 100, Some(0.2), &[]);
    assert_eq!(quiet.affinity, Some(20));
    let direct = share("d", 101, Some(0.1), &["a"]);
    let unknown = share("u", 102, None, &[]);
    let loved = share("l", 103, Some(0.91), &[]);
    model.posts = vec![loved, unknown, direct, quiet];
    // The quiet group share is the only one that does not count.
    assert_eq!(model.unread_since(50), 3);
    // Home leads with the loved share, then keeps the feed's order.
    let picks: Vec<&str> = model.picks(3).iter().map(|p| p.id.as_str()).collect();
    assert_eq!(picks, ["l", "u", "d"]);
    model.posts[0].affinity = Some(40);
    let picks: Vec<&str> = model.picks(2).iter().map(|p| p.id.as_str()).collect();
    assert_eq!(picks, ["l", "u"]);
}

#[test]
fn an_out_of_range_affinity_is_ignored() {
    let mut json = post_json();
    json["affinity"] = 1.5.into();
    assert_eq!(serde_json::from_value::<Post>(json).unwrap().affinity, None);
    assert_eq!(
        serde_json::from_value::<Post>(post_json())
            .unwrap()
            .affinity,
        None
    );
}

#[test]
fn autoplay_picks_only_friends_spotify_songs_it_has_not_just_played() {
    use super::pick::{Song, candidates, choose, track_uri};
    let post = |id: &str, author: &str, url: &str, source: &str, affinity: Option<f64>| {
        let mut json = post_json();
        json["id"] = id.into();
        json["author_id"] = author.into();
        json["url"] = url.into();
        json["source"] = source.into();
        json["title"] = format!("Song {id}").into();
        json["artists"] = json!(["", "Artist\u{7}"]);
        json["affinity"] = affinity.map_or(serde_json::Value::Null, Into::into);
        serde_json::from_value::<Post>(json).unwrap()
    };
    let posts = vec![
        post(
            "1",
            "b",
            "https://open.spotify.com/intl-nl/track/AAA",
            "spotify",
            None,
        ),
        post(
            "2",
            "a",
            "https://open.spotify.com/track/BBB",
            "spotify",
            None,
        ),
        post(
            "3",
            "b",
            "https://open.spotify.com/album/CCC",
            "spotify",
            None,
        ),
        post("4", "b", "https://soundcloud.com/x/y", "soundcloud", None),
        post(
            "5",
            "c",
            "https://open.spotify.com/track/DDD",
            "spotify",
            Some(0.1),
        ),
        post(
            "6",
            "c",
            "https://open.spotify.com/track/EEE",
            "spotify",
            Some(0.8),
        ),
        post(
            "7",
            "c",
            "https://open.spotify.com/track/AAA",
            "spotify",
            None,
        ),
        post(
            "8",
            "c",
            "https://open.spotify.com/track/FFF",
            "spotify",
            None,
        ),
    ];
    assert_eq!(track_uri(&posts[0]).as_deref(), Some("spotify:track:AAA"));
    assert_eq!(track_uri(&posts[2]), None);
    // Not my own share, not an album, not SoundCloud, not a quiet one, no
    // repeats, and nothing already in the list.
    let picked = candidates(&posts, "a", &["spotify:track:FFF"]);
    let ids: Vec<&str> = picked.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["spotify:track:AAA", "spotify:track:EEE"]);
    assert_eq!(picked[0].artists, ["Artist"]);

    let ranked = |pairs: &[(&str, f64)]| {
        pairs
            .iter()
            .map(|(id, p)| ((*id).to_owned(), *p))
            .collect::<std::collections::HashMap<_, _>>()
    };
    assert_eq!(
        choose(&ranked(&[("x", 0.8), ("y", 0.15), ("z", 0.05)]), 3).as_deref(),
        Some("x")
    );
    // A close call or a weak favourite leaves Spotify's autoplay alone.
    assert_eq!(choose(&ranked(&[("x", 0.55), ("y", 0.45)]), 2), None);
    assert_eq!(choose(&ranked(&[("x", 0.4), ("y", 0.1)]), 2), None);
    assert_eq!(choose(&ranked(&[("x", 0.6), ("y", 0.1)]), 2), None);
    assert_eq!(choose(&ranked(&[]), 2), None);

    let long = "\u{1F3B5}".repeat(200);
    let song = Song::new(&long, ["  ", "Name"].into_iter());
    assert_eq!(song.title.encode_utf16().count(), 300);
    assert_eq!(song.artists, ["Name"]);
}
