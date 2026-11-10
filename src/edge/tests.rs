//! The edge socket's tests: a local WebSocket answers, and nothing here ever
//! touches the real service.
//!
//! `tokio-tungstenite` answers a bad handshake with a large response type of
//! its own, which the test's callback has to return as it is.
#![allow(clippy::result_large_err)]

use std::sync::{Arc, Mutex};

use tokio::net::TcpListener;

use crate::edge::test_support::{Server, group_source};
use crate::edge::{Event, Scope, Tokens};

fn tokens() -> Tokens {
    Tokens {
        url: "https://edge.example".into(),
        live: "live-secret".into(),
        media: "media-secret".into(),
        sync: "sync-secret".into(),
        expires_at: crate::edge::now_secs() + 300,
        account: "test-account".into(),
    }
}

#[test]
fn no_token_is_ever_printed() {
    let printed = format!("{:?}", tokens());
    assert!(printed.contains("edge.example"));
    assert!(!printed.contains("live-secret"));
    assert!(!printed.contains("media-secret"));
    assert!(!printed.contains("sync-secret"));
    // The minted grant itself has no Debug at all, by construction.
    let source = tokens();
    assert_eq!(source.get(Scope::Live), "live-secret");
    assert_eq!(source.get(Scope::Media), "media-secret");
    assert_eq!(source.get(Scope::Sync), "sync-secret");
}

/// The edge's socket cannot carry an answer over 64 KiB; a larger one is
/// replaced by a small error so the phone asks over the relay instead.
#[test]
fn an_answer_too_large_for_the_edge_becomes_a_small_error() {
    let small = serde_json::json!({ "ok": true, "url": "https://example/a.m4a" });
    assert_eq!(crate::edge::edge_reply(small.clone()), small);
    let large = serde_json::json!({
        "songs": "x".repeat(crate::edge::MAX_REPLY_BYTES),
    });
    assert_eq!(
        crate::edge::edge_reply(large),
        serde_json::json!({ "error": "answer_too_large" })
    );
}

#[tokio::test]
async fn a_reconnect_asks_for_everything_after_the_last_sequence() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let ws = format!("http://127.0.0.1:{port}");
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let recorded = Arc::clone(&seen);
    let count = Arc::clone(&connections);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let recorded = Arc::clone(&recorded);
            let number = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::spawn(async move {
                let query = Arc::new(Mutex::new(String::new()));
                let captured = Arc::clone(&query);
                let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(
                    stream,
                    move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                          response| {
                        *captured
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                            request.uri().query().unwrap_or_default().to_owned();
                        Ok(response)
                    },
                )
                .await
                else {
                    return;
                };
                let query = query
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                recorded
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(query);
                use futures_util::SinkExt;
                let hello = if number == 0 {
                    serde_json::json!({
                        "v": 1, "t": "hello", "id": "s", "ok": true,
                        "role": "computer", "seq": 5, "epoch": "e",
                        "reset": false, "replayed": 0,
                    })
                } else {
                    serde_json::json!({
                        "v": 1, "t": "hello", "id": "s", "ok": true,
                        "role": "computer", "seq": 6, "epoch": "e",
                        "reset": false, "replayed": 0,
                    })
                };
                let _ = socket
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        hello.to_string().into(),
                    ))
                    .await;
                let command = if number == 0 {
                    serde_json::json!({
                        "v": 1, "t": "command", "id": "c1", "kind": "next",
                        "seq": 6, "from": "phone",
                    })
                } else {
                    serde_json::json!({
                        "v": 1, "t": "command", "id": "c2", "kind": "next",
                        "seq": 7, "from": "phone",
                    })
                };
                let _ = socket
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        command.to_string().into(),
                    ))
                    .await;
                if number == 0 {
                    let _ = socket.close(None).await;
                } else {
                    // Keep the second socket open while the test looks at it.
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
            });
        }
    });

    let server = Server::new(move |request| {
        if request.path == "/edge/token" {
            return (
                200,
                serde_json::json!({
                    "url": ws, "live": "live", "media": "media", "sync": "sync"
                }),
            );
        }
        (404, serde_json::json!({ "error": "not_found" }))
    });
    let source = group_source(&server).await;
    let (events_tx, mut events) = tokio::sync::mpsc::unbounded_channel();
    let _live = crate::edge::live::spawn(
        source,
        Arc::new(move |event: Event| {
            let _ = events_tx.send(match event {
                Event::Commands(commands) => commands[0].id.clone(),
                Event::Reset => "reset".into(),
                Event::Phone(_) => "phone".into(),
            });
        }),
    );

    let mut ids = Vec::new();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    while ids.len() < 2 {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !left.is_zero(),
            "the live socket did not reconnect in time: {ids:?}"
        );
        if let Ok(Some(id)) = tokio::time::timeout(left, events.recv()).await {
            ids.push(id);
        }
    }
    assert_eq!(ids, ["c1", "c2"]);
    let queries = seen
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    assert_eq!(queries.len(), 2, "one initial connection and one resume");
    assert!(!queries[0].contains("since="));
    assert!(
        queries[1].contains("since=6"),
        "resumed after seq 6: {queries:?}"
    );
}

#[tokio::test]
async fn turning_uploads_off_stops_a_waiting_local_file() {
    let server = Server::new(|request| {
        if request.path == "/edge/token" {
            return (
                200,
                serde_json::json!({
                    "url": "SELF", "live": "live", "media": "media", "sync": "sync"
                }),
            );
        }
        if request.path.starts_with("/v1/vault/") {
            return (200, serde_json::json!({ "v": 1 }));
        }
        if request.path == "/v1/media-token" {
            return (404, serde_json::json!({ "error": "not_found" }));
        }
        if request.path.starts_with("/v1/upload/") {
            return (201, serde_json::json!({ "key": "k" }));
        }
        (404, serde_json::json!({ "error": "not_found" }))
    });
    let root = crate::test_dirs::temp_dir()
        .join(format!("siphon-edge-uploads-off-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("song.m4a");
    let mut bytes = b"\0\0\0\x18ftypM4A ".to_vec();
    bytes.resize(64, b'a');
    std::fs::write(&file, bytes).unwrap();
    let source = group_source(&server).await;
    let runtime = crate::edge::Runtime::spawn(
        source,
        crate::downloads::saved::Saved::new(root.join("matches.json")),
        root.join("fetch"),
        Arc::new(|_| {}),
    );
    runtime.send(crate::edge::Request::UploadsEnabled(false));
    runtime.send(crate::edge::Request::Upload(vec![crate::edge::UploadJob {
        key: crate::edge::local_key("spotify:local:a:b:c:1"),
        path: file,
        fetched: false,
    }]));
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        server
            .hits()
            .iter()
            .all(|hit| !hit.path.starts_with("/v1/upload/")),
        "nothing is sent while the setting is off"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_live_address_uses_wss_and_keeps_the_sequence() {
    let mut source = tokens();
    source.expires_at = crate::edge::now_secs() + 300;
    let url = crate::edge::live_url(&source, 42);
    assert!(url.starts_with("wss://edge.example/v1/live?token=live-secret"));
    assert!(url.ends_with("&since=42"));
    source.url = "http://127.0.0.1:9".into();
    assert!(crate::edge::live_url(&source, 0).starts_with("ws://127.0.0.1:9/v1/live"));
}
