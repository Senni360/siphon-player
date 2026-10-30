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
}
