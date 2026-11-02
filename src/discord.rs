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
/// What Discord accepts as a link. It refuses the whole card over a longer one.
const URL: usize = 256;
/// The mark in the artwork's corner: its rings pulse while music plays, and
/// hold still when it stops. Discord fetches these, this app never does.
const BADGE_PLAYING: &str = "https://raw.githubusercontent.com/Senni360/siphon-player/main/packaging/icons/encore-badge.gif";
const BADGE_STILL: &str = "https://raw.githubusercontent.com/Senni360/siphon-player/main/packaging/icons/encore-badge.png";
/// Bars of the sound in one small space, from the bass up.
const PULSE_BARS: usize = 6;
const PULSE_GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

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
    /// A line about this listener, shown when pointing at the badge.
    pub hover: Option<String>,
    /// What is special about this song right now, under its artist.
    pub moment: Option<String>,
    /// The sound at this moment, as bars. See [`pulse`].
    pub pulse: Option<String>,
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

/// What the song sounds like at this moment, in a few characters, from the
/// player bar's analyser. Silence draws nothing rather than a flat line.
pub fn pulse(levels: &[f32]) -> Option<String> {
    if levels.is_empty() || levels.iter().all(|level| *level < 0.02) {
        return None;
    }
    let each = levels.len().div_ceil(PULSE_BARS);
    Some(
        levels
            .chunks(each)
            .map(|band| {
                let level = band.iter().copied().fold(0.0, f32::max).clamp(0.0, 1.0);
                PULSE_GLYPHS[(level * (PULSE_GLYPHS.len() - 1) as f32).round() as usize]
            })
            .collect(),
    )
}

/// The line a song you keep coming back to earns: the name of the app, as a
/// count. `today` and `ever` are the plays before this one.
pub fn encore_line(today: usize, ever: usize) -> Option<String> {
    match (today, ever) {
        (0, 0..=3) => None,
        (0, ever) => Some(format!("{} plays and counting", ever + 1)),
        (today, _) => Some(format!("Encore ×{today} · {} plays today", today + 1)),
    }
}

pub fn activity(status: &Status) -> Value {
    // No artist means the song itself must not show: all there is to say is
    // the party, if any.
    let hidden = status.artists.is_empty();
    let party_line = status
        .party
        .as_ref()
        .map(|party| format!("{} · {} listening", party.name, party.size));
    let state = if hidden {
        party_line.clone()
    } else if status.paused {
        Some(format!("Paused · {}", status.artists))
    } else {
        Some(match &status.pulse {
            // First, so a long list of artists cannot push it out of sight.
            Some(pulse) => format!("{pulse}  {}", status.artists),
            None => status.artists.clone(),
        })
    };
    let mut activity = json!({
        "type": 2,
        "details": short(&status.title, TEXT),
        // In the member list, "Listening to" the song, not to the app.
        "status_display_type": if hidden { 0 } else { 2 },
    });
    if let Some(state) = state {
        activity["state"] = json!(short(&state, TEXT));
    }
    let about = status
        .hover
        .clone()
        .unwrap_or_else(|| crate::brand::NAME.to_owned());
    let line = party_line
        .filter(|_| !hidden)
        .or_else(|| status.moment.clone())
        .unwrap_or_else(|| about.clone());
    let mut assets = json!({ "large_text": short(&line, TEXT) });
    match &status.artwork_url {
        Some(art) => {
            assets["large_image"] = json!(art);
            assets["small_image"] = json!(if status.paused {
                BADGE_STILL
            } else {
                BADGE_PLAYING
            });
            assets["small_text"] = json!(short(&about, TEXT));
        }
        // Never an empty square: the mark alone, large.
        None => assets["large_image"] = json!(BADGE_STILL),
    }
    if let Some((_, url)) = status.link.as_ref().filter(|(_, url)| url.len() <= URL) {
        activity["details_url"] = json!(url);
        activity["state_url"] = json!(url);
        assets["large_url"] = json!(url);
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
            moment: None,
            pulse: None,
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
        assert_eq!(activity["state"], "Massive Attack");
        assert_eq!(activity["assets"]["large_text"], "Friday · 3 listening");
        assert_eq!(activity["party"]["size"][0], 3);
        assert_eq!(activity["buttons"][0]["label"], "Join in Encore");
        assert_eq!(activity["buttons"][1]["label"], "Open in Spotify");
    }

    #[test]
    fn the_card_says_something_about_the_listener() {
        let plain = activity(&Status {
            hover: Some("41 hours this month".into()),
            ..status()
        });
        assert_eq!(plain["assets"]["large_text"], "41 hours this month");
        assert_eq!(plain["assets"]["small_text"], "41 hours this month");
        let special = activity(&Status {
            hover: Some("41 hours this month".into()),
            moment: Some("Encore ×2 · 3 plays today".into()),
            ..status()
        });
        assert_eq!(special["assets"]["large_text"], "Encore ×2 · 3 plays today");
        assert_eq!(special["assets"]["small_text"], "41 hours this month");
    }

    #[test]
    fn a_song_you_keep_coming_back_to_earns_an_encore() {
        assert_eq!(encore_line(0, 0), None);
        assert_eq!(encore_line(0, 3), None);
        assert_eq!(encore_line(0, 4).unwrap(), "5 plays and counting");
        assert_eq!(encore_line(1, 1).unwrap(), "Encore ×1 · 2 plays today");
        assert_eq!(encore_line(3, 40).unwrap(), "Encore ×3 · 4 plays today");
    }

    #[test]
    fn the_pulse_draws_the_sound_and_nothing_for_silence() {
        assert_eq!(pulse(&[]), None);
        assert_eq!(pulse(&[0.0; 75]), None);
        let mut levels = [0.0; 75];
        levels[0] = 1.0;
        levels[74] = 0.5;
        let bars = pulse(&levels).unwrap();
        assert_eq!(bars.chars().count(), PULSE_BARS);
        assert_eq!(bars.chars().next(), Some('█'));
        assert_eq!(bars.chars().nth(2), Some('▁'));
        assert_eq!(bars.chars().last(), Some('▅'));
    }

    #[test]
    fn the_pulse_leads_the_artist_while_playing_and_rests_when_paused() {
        let playing = Status {
            pulse: Some("▂▅▇▆▃▂".into()),
            ..status()
        };
        assert_eq!(activity(&playing)["state"], "▂▅▇▆▃▂  Massive Attack");
        let paused = Status {
            paused: true,
            ..playing
        };
        assert_eq!(activity(&paused)["state"], "Paused · Massive Attack");
    }

    #[test]
    fn the_member_list_names_the_song_and_its_title_links_to_it() {
        let activity = activity(&status());
        assert_eq!(activity["status_display_type"], 2);
        assert_eq!(activity["details_url"], activity["buttons"][0]["url"]);
        assert_eq!(activity["state_url"], activity["buttons"][0]["url"]);
        assert_eq!(
            activity["assets"]["large_url"],
            activity["buttons"][0]["url"]
        );
    }

    #[test]
    fn the_badge_pulses_only_while_music_plays() {
        let playing = activity(&status());
        assert_eq!(playing["assets"]["small_image"], BADGE_PLAYING);
        let paused = activity(&Status {
            paused: true,
            ..status()
        });
        assert_eq!(paused["assets"]["small_image"], BADGE_STILL);
    }

    #[test]
    fn a_hidden_song_says_nothing_beyond_listening_on_the_app() {
        let hidden = activity(&Status {
            title: "Listening on Encore".into(),
            artists: String::new(),
            artwork_url: None,
            link: None,
            ..status()
        });
        assert_eq!(hidden["details"], "Listening on Encore");
        assert_eq!(hidden["status_display_type"], 0);
        assert!(hidden.get("state").is_none());
        assert!(hidden.get("details_url").is_none());
        assert!(hidden.get("buttons").is_none());
        assert_eq!(hidden["assets"]["large_image"], BADGE_STILL);
        assert_eq!(hidden["assets"]["large_text"], "Encore");
    }

    #[test]
    fn a_link_too_long_for_discord_cannot_cost_the_whole_card() {
        let long = activity(&Status {
            link: Some(("Open in Spotify", format!("https://x/{}", "y".repeat(300)))),
            ..status()
        });
        assert!(long.get("details_url").is_none());
        assert!(long["assets"].get("large_url").is_none());
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
