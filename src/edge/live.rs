//! The realtime room on the edge: one socket as `computer`, always connected
//! while a phone is linked. A command arrives here exactly as it would over the
//! group service's relay, and state or a reply goes back the same way.
//!
//! Reconnects keep the last sequence number so the edge replays what was
//! missed. A token is replaced shortly before it expires; the socket is opened
//! again with the fresh one. Nothing here logs a token or a URL carrying one.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

use super::{Event, Source, Tokens, live_url};

/// The server pings every 15 seconds; an application ping is cheap and keeps
/// a middlebox from dropping an idle socket.
const HEARTBEAT: Duration = Duration::from_secs(15);
const RETRY: [u64; 6] = [1, 2, 4, 8, 16, 30];

/// A handle the runtime sends state and replies through. The queue is small
/// and lossy on purpose: state is a whole picture and a reply that waits out
/// a long outage is of no use to the phone about to time out. A bounded queue
/// keeps a disconnected edge from growing this computer's memory.
#[derive(Clone)]
pub struct Live {
    tx: tokio::sync::mpsc::Sender<Message>,
    task: Arc<tokio::task::JoinHandle<()>>,
}

impl Live {
    pub fn state(&self, snapshot: Value) {
        let _ = self
            .tx
            .try_send(Message::Text(state_frame(&snapshot).into()));
    }

    pub fn reply(&self, id: &str, value: Value) {
        let _ = self
            .tx
            .try_send(Message::Text(reply_frame(id, &value).into()));
    }

    /// Closes the socket and stops trying. Nothing else is left running.
    pub fn stop(&self) {
        self.task.abort();
    }
}

/// The state frame the phone reads: one whole picture of what plays here.
fn state_frame(snapshot: &Value) -> String {
    serde_json::json!({
        "v": 1,
        "t": "state",
        "id": format!("state-{}", super::now_secs()),
        "state": snapshot,
    })
    .to_string()
}

/// The answer to one phone command. `v`, `t` and `id` belong to the envelope,
/// so a value cannot overwrite them; the rest is passed through as it is.
fn reply_frame(id: &str, value: &Value) -> String {
    let mut envelope = serde_json::Map::new();
    envelope.insert("v".into(), Value::from(1));
    envelope.insert("t".into(), Value::from("reply"));
    envelope.insert("id".into(), Value::from(id));
    if let Value::Object(fields) = value {
        for (key, value) in fields {
            envelope.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    Value::Object(envelope).to_string()
}

pub fn spawn(source: Arc<Source>, emit: Arc<dyn Fn(Event) + Send + Sync>) -> Live {
    // Deep enough for a burst of replies while a socket drains, small enough
    // that a long outage cannot pile up a state picture every eight seconds.
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let task = Arc::new(tokio::spawn(run(source, rx, emit)));
    Live { tx, task }
}

async fn run(
    source: Arc<Source>,
    mut outgoing: tokio::sync::mpsc::Receiver<Message>,
    emit: Arc<dyn Fn(Event) + Send + Sync>,
) {
    let mut last_seq = 0u64;
    let mut retry = 0usize;
    loop {
        let tokens = match source.tokens().await {
            Ok(tokens) => tokens,
            Err(_) => {
                tokio::time::sleep(backoff(&mut retry)).await;
                continue;
            }
        };
        let url = live_url(&tokens, last_seq);
        let connect = tokio_tungstenite::connect_async(url).await;
        let Ok((mut socket, _)) = connect else {
            tokio::time::sleep(backoff(&mut retry)).await;
            continue;
        };
        retry = 0;
        let refresh_in = refresh_after(&tokens);
        let refresh = tokio::time::sleep(refresh_in);
        tokio::pin!(refresh);
        let mut beat = tokio::time::interval(HEARTBEAT);
        beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let mut connected = true;
        while connected {
            tokio::select! {
                message = socket.next() => {
                    let Some(Ok(message)) = message else { break; };
                    match message {
                        Message::Text(text) => heard(text.as_str(), &emit, &mut last_seq),
                        Message::Ping(payload) => {
                            let _ = socket.send(Message::Pong(payload)).await;
                        }
                        Message::Close(_) => connected = false,
                        _ => {}
                    }
                }
                out = outgoing.recv() => {
                    let Some(out) = out else { return; };
                    if socket.send(out).await.is_err() {
                        connected = false;
                    }
                }
                _ = beat.tick() => {
                    let ping = Message::Text(r#"{"v":1,"t":"ping"}"#.to_owned().into());
                    if socket.send(ping).await.is_err() {
                        connected = false;
                    }
                }
                _ = &mut refresh => {
                    // A token is replaced before it can expire; the phone's
                    // next hello (or the edge's buffer) finds this computer
                    // at the same sequence number.
                    connected = false;
                }
            }
        }
        let _ = socket.close(None).await;
        tokio::time::sleep(backoff(&mut retry)).await;
    }
}

fn backoff(retry: &mut usize) -> Duration {
    let seconds = RETRY[(*retry).min(RETRY.len() - 1)];
    *retry = retry.saturating_add(1);
    Duration::from_secs(seconds)
}

fn refresh_after(tokens: &Tokens) -> Duration {
    let left = tokens.expires_at - super::now_secs() - super::REFRESH_BEFORE;
    Duration::from_secs(left.max(1) as u64)
}

/// Reads one frame. The sequence number is kept so the next connection can
/// ask for everything after it.
fn heard(text: &str, emit: &Arc<dyn Fn(Event) + Send + Sync>, last_seq: &mut u64) {
    let Ok(message) = serde_json::from_str::<Value>(text) else {
        return;
    };
    if let Some(seq) = message.get("seq").and_then(Value::as_u64) {
        *last_seq = (*last_seq).max(seq);
    }
    // Every frame the edge sends is version 1; anything else is not a frame
    // this computer understands.
    if message.get("v").and_then(Value::as_u64) != Some(1) {
        return;
    }
    match message.get("t").and_then(Value::as_str) {
        Some("hello") => {
            if message
                .get("reset")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                emit(Event::Reset);
            }
            // The edge's own hello carries the last thing the phone said, in
            // the same shape a relayed `phone` frame has.
            if let Some(body) = message.get("state").and_then(|state| state.get("body"))
                && let Some(phone) = phone_state(body)
            {
                emit(Event::Phone(Some(phone)));
            }
        }
        Some("command") => {
            if let Ok(command) = serde_json::from_value::<crate::remote::Incoming>(message) {
                emit(Event::Commands(vec![command]));
            }
        }
        // A phone sends `phone`; the same playback picture may also arrive as
        // a `state` frame, so both are read the same way.
        Some("state") | Some("phone") => match phone_state(&message) {
            Some(phone) => emit(Event::Phone(Some(phone))),
            None => emit(Event::Phone(None)),
        },
        _ => {}
    }
}

/// The phone's playback as the edge relays it. The frame carries the edge's
/// clock, so a replay is judged by when it was heard there, not now.
fn phone_state(frame: &Value) -> Option<crate::friends::PhonePlaying> {
    let at = frame.get("at").and_then(Value::as_i64).unwrap_or(0);
    let value = frame
        .get("state")
        .or_else(|| frame.get("phone"))
        .unwrap_or(frame);
    if !value.is_object() {
        return None;
    }
    let mut phone: crate::friends::PhonePlaying = serde_json::from_value(value.clone()).ok()?;
    if at > 0 {
        phone.at = at;
    }
    if phone.at == 0 {
        phone.at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or_default();
    }
    phone.song.is_some().then_some(phone)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything the socket's loop emitted, in order.
    type Emitted = Arc<std::sync::Mutex<Vec<Event>>>;

    fn collected() -> (Emitted, Arc<dyn Fn(Event) + Send + Sync>) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let held = Arc::clone(&seen);
        let emit = Arc::new(move |event: Event| {
            held.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        });
        (seen, emit)
    }

    /// The frame this computer sends is the one the phone's edge module applies.
    #[test]
    fn the_state_frame_is_the_shape_the_phone_applies() {
        let snapshot = serde_json::json!({
            "song": { "uri": "spotify:track:a", "name": "A", "artists": ["X"] },
            "position_ms": 12_000,
            "playing": true,
        });
        let frame: Value = serde_json::from_str(&state_frame(&snapshot)).unwrap();
        assert_eq!(frame["v"], 1);
        assert_eq!(frame["t"], "state");
        assert!(frame["id"].as_str().unwrap().starts_with("state-"));
        // The phone reads `state` first and applies it as its own picture.
        assert_eq!(frame["state"]["position_ms"], 12_000);
        assert_eq!(frame["state"]["song"]["uri"], "spotify:track:a");
    }

    /// A reply keeps the id the phone is waiting on, and its value cannot
    /// overwrite the envelope's own fields.
    #[test]
    fn a_reply_keeps_its_id_and_its_value() {
        let value = serde_json::json!({
            "ok": true,
            "url": "https://example/a.m4a",
            "id": "not-the-envelope",
            "t": "control",
        });
        let frame: Value = serde_json::from_str(&reply_frame("phone-7", &value)).unwrap();
        assert_eq!(frame["v"], 1);
        assert_eq!(frame["t"], "reply");
        assert_eq!(frame["id"], "phone-7");
        assert_eq!(frame["url"], "https://example/a.m4a");
        assert_eq!(frame["ok"], true);
    }

    /// A command arrives exactly as the edge relays it: the server stamps
    /// `from` and `seq` onto what the phone sent.
    #[test]
    fn a_relayed_command_is_read_with_the_servers_stamp() {
        let (seen, emit) = collected();
        let mut seq = 0;
        heard(
            r#"{"v":1,"t":"command","id":"c1","kind":"seek","ms":42000,"seq":9,"from":"phone"}"#,
            &emit,
            &mut seq,
        );
        assert_eq!(seq, 9);
        let seen = seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match &seen[0] {
            Event::Commands(commands) => {
                assert_eq!(commands[0].id, "c1");
                assert_eq!(
                    commands[0].command,
                    crate::remote::Command::Seek { ms: 42_000 }
                );
            }
            _ => panic!("a command was expected"),
        }
    }

    /// The hello carries the last thing the phone said, in the same shape a
    /// relayed `phone` frame has, and a reset means the store must be read again.
    #[test]
    fn a_hello_with_the_phones_state_and_a_reset_is_believed() {
        let (seen, emit) = collected();
        let mut seq = 0;
        heard(
            &serde_json::json!({
                "v": 1, "t": "hello", "id": "srv-1", "ok": true,
                "role": "computer", "seq": 4, "epoch": "e", "reset": true, "replayed": 0,
                "state": {
                    "t": "phone", "seq": 3,
                    "body": {
                        "v": 1, "t": "phone", "id": "p1", "from": "phone",
                        "phone": {
                            "song": { "uri": "soundcloud:track:7", "name": "Cloud" },
                            "playing": true, "position_ms": 1000, "at": 0,
                        },
                    },
                },
            })
            .to_string(),
            &emit,
            &mut seq,
        );
        assert_eq!(seq, 4);
        let seen = seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(matches!(seen[0], Event::Reset));
        match &seen[1] {
            Event::Phone(Some(phone)) => {
                assert_eq!(phone.song.as_ref().unwrap().uri, "soundcloud:track:7");
                assert!(phone.playing);
                // The edge's clock is not in the frame, so arrival time is used.
                assert!(phone.at > 0);
            }
            _ => panic!("the phone's playback was expected"),
        }
    }

    /// The edge stamps a frame with its own clock; a replayed frame keeps that
    /// time so the app can tell it is old.
    #[test]
    fn a_replayed_frame_keeps_the_edges_clock() {
        let (seen, emit) = collected();
        let mut seq = 0;
        heard(
            &serde_json::json!({
                "v": 1, "t": "phone", "id": "p3", "seq": 5,
                "at": 1_700_000_000_000i64, "from": "phone",
                "phone": {
                    "song": { "uri": "soundcloud:track:7", "name": "Cloud" },
                    "playing": false, "position_ms": 1000,
                },
            })
            .to_string(),
            &emit,
            &mut seq,
        );
        let seen = seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match &seen[0] {
            Event::Phone(Some(phone)) => assert_eq!(phone.at, 1_700_000_000_000),
            _ => panic!("the phone's playback was expected"),
        }
    }

    /// A phone that stopped says `phone: null`; that clears what this computer
    /// shows, and is not an error.
    #[test]
    fn a_phone_that_stopped_clears_the_shown_song() {
        let (seen, emit) = collected();
        let mut seq = 0;
        heard(
            r#"{"v":1,"t":"phone","id":"p2","phone":null,"seq":5,"from":"phone"}"#,
            &emit,
            &mut seq,
        );
        let seen = seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        assert!(matches!(seen[0], Event::Phone(None)));
    }

    /// A frame this computer does not understand is passed over, never fatal.
    #[test]
    fn nonsense_and_unknown_frames_are_ignored() {
        let (seen, emit) = collected();
        let mut seq = 0;
        heard("not json", &emit, &mut seq);
        heard(r#"{"v":1,"t":"control","id":"x","seq":2}"#, &emit, &mut seq);
        heard(r#"{"v":2,"t":"state","id":"x"}"#, &emit, &mut seq);
        assert_eq!(seq, 2);
        assert!(
            seen.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        );
    }
}
