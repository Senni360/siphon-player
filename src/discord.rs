//! Shows what plays here on the listener's Discord profile, through the Discord
//! app running on this computer. Nothing goes over the network from here: this
//! writes to Discord's local socket, and Discord does the rest.
//!
//! A private SoundCloud link is a secret that happens to be a URL, so a song
//! from one shows up as nothing more than "Listening on Encore".

use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// The Discord application whose name reads "Listening to Encore". Not a
/// secret: every Discord presence carries its application's id.
pub const APPLICATION_ID: &str = "1566731432385314876";
const RETRY: Duration = Duration::from_secs(20);
const ANSWER: Duration = Duration::from_secs(5);
const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
/// A frame much larger than a presence is not something Discord sent us.
const LARGEST_FRAME: u32 = 64 * 1024;
/// What Discord accepts in a text field, and in a button's label.
const TEXT: usize = 128;
const BUTTON: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Party {
    pub id: String,
    pub name: String,
    pub size: u32,
    /// Where the Join button goes. It only lets in those already in the group.
    pub join_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub title: String,
    pub artists: String,
    pub artwork_url: Option<String>,
    pub link: Option<(&'static str, String)>,
    /// Unix milliseconds this song began and will end, when it is playing.
    pub started_ms: Option<u64>,
    pub ends_ms: Option<u64>,
    pub paused: bool,
    /// A line about this listener, shown when pointing at the artwork.
    pub hover: Option<String>,
    pub party: Option<Party>,
}

fn short(text: &str, most: usize) -> String {
    let mut kept: String = text.chars().take(most).collect();
    // Discord rejects a field under two characters.
    while kept.chars().count() < 2 {
        kept.push(' ');
    }
    kept
}

/// Whether a song's link, or the artwork that came with it, must stay private.
pub fn is_private_link(url: &str) -> bool {
    url.contains("secret_token=") || url.contains("/s-")
}

pub fn activity(status: &Status) -> Value {
    let mut activity = json!({
        "type": 2,
        "details": short(&status.title, TEXT),
        "state": short(
            &match (&status.party, status.paused) {
                (Some(party), _) => format!("{} · {} listening", party.name, party.size),
                (None, true) => format!("Paused · {}", status.artists),
                (None, false) => status.artists.clone(),
            },
            TEXT,
        ),
    });
    let hover = status
        .hover
        .clone()
        .unwrap_or_else(|| crate::brand::NAME.to_owned());
    let mut assets = json!({ "large_text": short(&hover, TEXT) });
    if let Some(art) = &status.artwork_url {
        assets["large_image"] = json!(art);
    }
    activity["assets"] = assets;
    if !status.paused {
        let mut timestamps = json!({});
        if let Some(started) = status.started_ms {
            timestamps["start"] = json!(started);
        }
        if let Some(ends) = status.ends_ms {
            timestamps["end"] = json!(ends);
        }
        activity["timestamps"] = timestamps;
    }
    let mut buttons = Vec::new();
    if let Some(party) = &status.party {
        activity["party"] = json!({ "id": party.id, "size": [party.size.max(1), 50] });
        if let Some(url) = &party.join_url {
            buttons.push(json!({ "label": short("Join in Encore", BUTTON), "url": url }));
        }
    }
    if let Some((label, url)) = &status.link {
        buttons.push(json!({ "label": short(label, BUTTON), "url": url }));
    }
    if !buttons.is_empty() {
        activity["buttons"] = json!(buttons);
    }
    activity
}

fn frame(op: u32, body: &Value) -> Vec<u8> {
    let body = body.to_string().into_bytes();
    let mut bytes = Vec::with_capacity(body.len() + 8);
    bytes.extend_from_slice(&op.to_le_bytes());
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes
}

fn set_activity(status: Option<&Status>, nonce: u64) -> Value {
    json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": std::process::id(), "activity": status.map(activity) },
        "nonce": nonce.to_string(),
    })
}

trait Socket: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Socket for T {}

#[cfg(windows)]
async fn open(index: u8) -> Option<Box<dyn Socket>> {
    let path = format!(r"\\?\pipe\discord-ipc-{index}");
    tokio::net::windows::named_pipe::ClientOptions::new()
        .open(path)
        .ok()
        .map(|pipe| Box::new(pipe) as Box<dyn Socket>)
}

#[cfg(unix)]
async fn open(index: u8) -> Option<Box<dyn Socket>> {
    let name = format!("discord-ipc-{index}");
    // Flatpak and Snap keep Discord's socket under their own runtime folders.
    let base = std::env::var("XDG_RUNTIME_DIR")
        .or_else(|_| std::env::var("TMPDIR"))
        .unwrap_or_else(|_| "/tmp".to_owned());
    for folder in ["", "app/com.discordapp.Discord", "snap.discord"] {
        let path = std::path::Path::new(&base).join(folder).join(&name);
        if let Ok(socket) = tokio::net::UnixStream::connect(path).await {
            return Some(Box::new(socket));
        }
    }
    None
}

async fn answer(socket: &mut Box<dyn Socket>) -> Option<()> {
    let mut head = [0u8; 8];
    tokio::time::timeout(ANSWER, socket.read_exact(&mut head))
        .await
        .ok()?
        .ok()?;
    let length = u32::from_le_bytes(head[4..8].try_into().ok()?);
    if length > LARGEST_FRAME {
        return None;
    }
    let mut body = vec![0; length as usize];
    tokio::time::timeout(ANSWER, socket.read_exact(&mut body))
        .await
        .ok()?
        .ok()?;
    Some(())
}

async fn connect(application: &str) -> Option<Box<dyn Socket>> {
    for index in 0..10 {
        let Some(mut socket) = open(index).await else {
            continue;
        };
        let hello = json!({ "v": 1, "client_id": application });
        if socket.write_all(&frame(OP_HANDSHAKE, &hello)).await.is_ok()
            && answer(&mut socket).await.is_some()
        {
            return Some(socket);
        }
    }
    None
}

async fn run(application: String, mut updates: UnboundedReceiver<Option<Status>>) {
    let mut latest: Option<Status> = None;
    let mut socket: Option<Box<dyn Socket>> = None;
    let mut nonce = 0u64;
    loop {
        // Without Discord open, this asks again now and then, and otherwise sleeps.
        let update = if socket.is_some() || latest.is_none() {
            updates.recv().await
        } else {
            match tokio::time::timeout(RETRY, updates.recv()).await {
                Ok(update) => update,
                Err(_) => Some(latest.clone()),
            }
        };
        let Some(status) = update else {
            return;
        };
        latest = status;
        if socket.is_none() {
            if latest.is_none() {
                continue;
            }
            socket = connect(&application).await;
        }
        let Some(open) = socket.as_mut() else {
            continue;
        };
        nonce = nonce.wrapping_add(1);
        let message = frame(OP_FRAME, &set_activity(latest.as_ref(), nonce));
        if open.write_all(&message).await.is_err() || answer(open).await.is_none() {
            // Discord was closed. The status comes back when it does.
            socket = None;
        }
    }
}

/// None stops showing anything. Ids come from the build, with an override for a copy.
pub fn spawn() -> Option<UnboundedSender<Option<Status>>> {
    let application = std::env::var("SIPHON_DISCORD_APP_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| APPLICATION_ID.to_owned());
    if application.is_empty() {
        return None;
    }
    let (tx, rx) = unbounded_channel();
    tokio::spawn(run(application, rx));
    Some(tx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> Status {
        Status {
            title: "Teardrop".into(),
            artists: "Massive Attack".into(),
            artwork_url: Some("https://i.scdn.co/image/abc".into()),
            link: Some((
                "Open in Spotify",
                "https://open.spotify.com/track/abc".into(),
            )),
            started_ms: Some(1_000),
            ends_ms: Some(181_000),
            paused: false,
            hover: None,
            party: None,
        }
    }

    #[test]
    fn a_playing_song_reads_as_listening_with_a_progress_bar() {
        let activity = activity(&status());
        assert_eq!(activity["type"], 2);
        assert_eq!(activity["details"], "Teardrop");
        assert_eq!(activity["state"], "Massive Attack");
        assert_eq!(activity["timestamps"]["start"], 1_000);
        assert_eq!(activity["timestamps"]["end"], 181_000);
        assert_eq!(
            activity["assets"]["large_image"],
            "https://i.scdn.co/image/abc"
        );
        assert_eq!(activity["buttons"][0]["label"], "Open in Spotify");
    }

    #[test]
    fn a_paused_song_has_no_clock_running() {
        let activity = activity(&Status {
            paused: true,
            ..status()
        });
        assert_eq!(activity["state"], "Paused · Massive Attack");
        assert!(activity.get("timestamps").is_none());
    }

    #[test]
    fn a_party_shows_its_size_and_a_way_in() {
        let activity = activity(&Status {
            party: Some(Party {
                id: "p".into(),
                name: "Friday".into(),
                size: 3,
                join_url: Some("https://example.com/join".into()),
            }),
            ..status()
        });
        assert_eq!(activity["state"], "Friday · 3 listening");
        assert_eq!(activity["party"]["size"][0], 3);
        assert_eq!(activity["buttons"][0]["label"], "Join in Encore");
        assert_eq!(activity["buttons"][1]["label"], "Open in Spotify");
    }

    #[test]
    fn the_artwork_says_something_about_the_listener() {
        let activity = activity(&Status {
            hover: Some("41 hours this month".into()),
            ..status()
        });
        assert_eq!(activity["assets"]["large_text"], "41 hours this month");
    }

    #[test]
    fn long_and_tiny_text_are_made_acceptable() {
        let activity = activity(&Status {
            title: "x".repeat(400),
            artists: "a".into(),
            ..status()
        });
        assert_eq!(activity["details"].as_str().unwrap().chars().count(), 128);
        assert_eq!(activity["state"].as_str().unwrap().chars().count(), 2);
    }

    #[test]
    fn private_links_are_recognised() {
        assert!(is_private_link("https://soundcloud.com/a/b/s-AbC123"));
        assert!(is_private_link(
            "https://soundcloud.com/a/b?secret_token=s-AbC"
        ));
        assert!(!is_private_link("https://soundcloud.com/a/b"));
        assert!(!is_private_link("https://open.spotify.com/track/abc"));
    }

    #[test]
    fn frames_carry_their_op_and_length() {
        let bytes = frame(OP_FRAME, &json!({ "a": 1 }));
        assert_eq!(&bytes[..4], &1u32.to_le_bytes());
        assert_eq!(&bytes[4..8], &7u32.to_le_bytes());
        assert_eq!(&bytes[8..], b"{\"a\":1}");
    }

    #[test]
    fn clearing_sends_no_activity() {
        assert!(set_activity(None, 1)["args"]["activity"].is_null());
    }
}
