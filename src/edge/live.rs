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

/// A handle the runtime sends state and replies through. Sending while the
/// socket is down simply drops the message; the next state is a whole picture.
#[derive(Clone)]
pub struct Live {
    tx: tokio::sync::mpsc::UnboundedSender<Message>,
}

impl Live {
    pub fn state(&self, snapshot: Value) {
        let envelope = serde_json::json!({
            "v": 1,
            "t": "state",
            "id": format!("state-{}", super::now_secs()),
            "state": snapshot,
        });
        let _ = self.tx.send(Message::Text(envelope.to_string().into()));
    }

    pub fn reply(&self, id: &str, value: Value) {
        let mut envelope = serde_json::Map::new();
        envelope.insert("v".into(), Value::from(1));
        envelope.insert("t".into(), Value::from("reply"));
        envelope.insert("id".into(), Value::from(id));
        if let Value::Object(fields) = value {
            for (key, value) in fields {
                envelope.entry(key).or_insert(value);
            }
        }
        let _ = self
            .tx
            .send(Message::Text(Value::Object(envelope).to_string().into()));
    }
}

pub fn spawn(source: Arc<Source>, emit: Arc<dyn Fn(Event) + Send + Sync>) -> Live {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(run(source, rx, emit));
    Live { tx }
}

async fn run(
    source: Arc<Source>,
    mut outgoing: tokio::sync::mpsc::UnboundedReceiver<Message>,
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
    match message.get("t").and_then(Value::as_str) {
        Some("hello") => {
            if message
                .get("reset")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                emit(Event::Reset);
            }
            if let Some(state) = message
                .get("state")
                .and_then(|state| state.get("body"))
                .and_then(|body| body.get("state"))
                && let Some(phone) = phone_state(state)
            {
                emit(Event::Phone(Some(phone)));
            }
        }
        Some("command") => {
            if let Ok(command) = serde_json::from_value::<crate::remote::Incoming>(message) {
                emit(Event::Commands(vec![command]));
            }
        }
        Some("state") => {
            let body = message.get("state").unwrap_or(&message);
            match phone_state(body) {
                Some(phone) => emit(Event::Phone(Some(phone))),
                None => emit(Event::Phone(None)),
            }
        }
        Some("phone") => {
            let body = message.get("phone").unwrap_or(&message);
            match phone_state(body) {
                Some(phone) => emit(Event::Phone(Some(phone))),
                None => emit(Event::Phone(None)),
            }
        }
        _ => {}
    }
}

/// The phone's playback as the edge relays it. The edge's clock is not in
/// this frame, so the local arrival time is the best age available.
fn phone_state(value: &Value) -> Option<crate::friends::PhonePlaying> {
    if !value.is_object() {
        return None;
    }
    let mut phone: crate::friends::PhonePlaying = serde_json::from_value(value.clone()).ok()?;
    if phone.at == 0 {
        phone.at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or_default();
    }
    phone.song.is_some().then_some(phone)
}
