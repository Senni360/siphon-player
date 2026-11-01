//! Listening together: what members are playing, listening along with one of
//! them, and channels whose queue belongs to everyone in them.
//!
//! Nobody's audio travels. Every listener plays the song on their own account
//! from its own service, and this only says which song, where and whether.
//! `Model` is the state and the decisions, with no clock or network of its own,
//! so the rules can be tested; `crate::app::live` drives the player from it.

use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};

use crate::credentials::Store as CredentialStore;
use crate::http::Http;

use super::client::valid_id;
use super::{Client, Identity, NewPost};

/// How often the live state is read while listening with someone.
pub const TOGETHER_POLL: Duration = Duration::from_secs(3);
/// How often it is read while only looking at the Group page.
pub const LOOKING_POLL: Duration = Duration::from_secs(15);
/// Presence is repeated this often so it does not expire; the service keeps
/// it for 200 seconds.
pub const HEARTBEAT: Duration = Duration::from_secs(90);
/// The id a channel carries until the service names it.
pub const PENDING_CHANNEL: &str = "pending-channel";
/// The prefix of an optimistic queue row's id until the service names it.
const PENDING_ENTRY: &str = "pending-entry";
/// Following stops at this many people between you and the leader.
const LONGEST_CHAIN: usize = 8;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Presence {
    pub user_id: String,
    pub item: Option<NewPost>,
    #[serde(default)]
    pub position_ms: u64,
    #[serde(default)]
    pub playing: bool,
    pub at: u64,
    #[serde(default)]
    pub following: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub item: NewPost,
    pub added_by: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub host_id: String,
    pub created_at: u64,
    pub members: Vec<String>,
    pub current: Option<Entry>,
    #[serde(default)]
    pub position_ms: u64,
    #[serde(default)]
    pub playing: bool,
    pub at: u64,
    #[serde(default)]
    pub queue: Vec<Entry>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub now: u64,
    #[serde(default)]
    pub presence: Vec<Presence>,
    #[serde(default)]
    pub channels: Vec<Channel>,
}

#[derive(Deserialize)]
struct ChannelAnswer {
    now: u64,
    channel: Option<Channel>,
}

#[derive(Deserialize)]
struct NowAnswer {
    now: u64,
}

/// Where a song goes in a channel's queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    End,
    Next,
    Now,
}

/// What the Group page and the rest of the app ask of listening together.
#[derive(Clone, Debug)]
pub enum Action {
    /// Whether the group sees what this listener plays.
    SetSharing(bool),
    /// Follow one member: they lead, and only they change what plays.
    ListenAlong(String),
    /// Stop following, or leave the channel.
    Leave,
    StartChannel(String),
    JoinChannel(String),
    Add(NewPost, Place),
    Remove(String),
    Move {
        entry: String,
        index: usize,
    },
    /// Skip to a row of the channel's queue.
    PlayEntry(String),
}

#[derive(Clone, Debug)]
pub enum Op {
    Read,
    Presence {
        item: Option<NewPost>,
        position_ms: u64,
        playing: bool,
        following: Option<String>,
    },
    ClearPresence,
    Create {
        name: String,
        item: Option<NewPost>,
        position_ms: u64,
        playing: bool,
    },
    Join(String),
    LeaveChannel(String),
    Playback {
        channel: String,
        playing: bool,
        position_ms: u64,
    },
    Next {
        channel: String,
        from: Option<String>,
        to: Option<String>,
    },
    Add {
        channel: String,
        item: NewPost,
        place: Place,
    },
    Remove {
        channel: String,
        entry: String,
    },
    Move {
        channel: String,
        entry: String,
        index: usize,
    },
    PutStats {
        period: &'static str,
        summary: Box<crate::stats::Summary>,
    },
    GetStats(&'static str),
    DeleteStats,
    /// Forget this device's identity: the group was left.
    Forget,
}

impl Op {
    /// Whether this changes a channel, so that a read sent before it is stale.
    fn changes_channel(&self) -> bool {
        matches!(
            self,
            Self::Create { .. }
                | Self::Join(_)
                | Self::LeaveChannel(_)
                | Self::Playback { .. }
                | Self::Next { .. }
                | Self::Add { .. }
                | Self::Remove { .. }
                | Self::Move { .. }
        )
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub generation: u64,
    pub op: Op,
}

#[derive(Debug)]
pub enum Update {
    Snapshot(Snapshot),
    Channel(Option<Box<Channel>>, u64),
    Now(u64),
    Stats(&'static str, Vec<MemberStats>),
    Nothing,
}

/// A member's totals for one period, as they shared them.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct MemberStats {
    pub user_id: String,
    pub summary: crate::stats::Summary,
}

#[derive(Deserialize)]
struct StatsAnswer {
    stats: Vec<MemberStats>,
}

#[derive(Debug)]
pub struct Event {
    generation: u64,
    changed_channel: bool,
    read: bool,
    sent_ms: u64,
    received_ms: u64,
    result: Result<Update, String>,
}

/// Who this listener is listening with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Session {
    #[default]
    Alone,
    Following(String),
    Channel(String),
}

/// What the session says should be playing here right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub item: NewPost,
    pub position_ms: u64,
    pub playing: bool,
    /// The channel's entry, which reports the song's end.
    pub entry: Option<String>,
}

/// What a read or a write's answer did to the session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    /// The person being followed stopped playing or sharing.
    LeaderGone(String),
    /// The channel this listener was in no longer holds them.
    ChannelGone,
    WriteFailed(String),
}

#[derive(Default)]
pub struct Model {
    pub snapshot: Snapshot,
    pub session: Session,
    /// The group's shared totals, by the service's name for the period.
    pub group_stats: std::collections::HashMap<&'static str, Vec<MemberStats>>,
    /// The service's clock minus this computer's, in milliseconds.
    offset_ms: i64,
    best_trip: Option<u64>,
    /// Bumped by every change to a channel: an answer from before the latest
    /// one still tells the story from before what the listener just did.
    generation: u64,
    read_in_flight: bool,
    pending_entries: u32,
}

fn advanced(position_ms: u64, playing: bool, at: u64, now: u64) -> u64 {
    if playing {
        position_ms + now.saturating_sub(at)
    } else {
        position_ms
    }
}

impl Model {
    pub fn server_now(&self, local_ms: u64) -> u64 {
        local_ms.saturating_add_signed(self.offset_ms)
    }

    pub fn together(&self) -> bool {
        self.session != Session::Alone
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn presence_of(&self, user: &str) -> Option<&Presence> {
        self.snapshot.presence.iter().find(|p| p.user_id == user)
    }

    pub fn channel(&self, id: &str) -> Option<&Channel> {
        self.snapshot.channels.iter().find(|c| c.id == id)
    }

    pub fn channel_of(&self, user: &str) -> Option<&Channel> {
        self.snapshot
            .channels
            .iter()
            .find(|c| c.members.iter().any(|m| m == user))
    }

    pub fn my_channel(&self) -> Option<&Channel> {
        match &self.session {
            Session::Channel(id) => self.channel(id),
            _ => None,
        }
    }

    /// The person a member's song comes from, following the chain to whoever
    /// is actually choosing. None for a chain that loops or runs too long.
    pub fn leader_of<'a>(&'a self, user: &'a str) -> Option<&'a str> {
        let mut current = user;
        for _ in 0..LONGEST_CHAIN {
            match self
                .presence_of(current)
                .and_then(|p| p.following.as_deref())
            {
                Some(next) => current = next,
                None => return Some(current),
            }
        }
        None
    }

    /// Who is listening along with `user`, directly or through someone else.
    pub fn followers_of(&self, user: &str) -> Vec<&str> {
        self.snapshot
            .presence
            .iter()
            .filter(|p| p.following.is_some() && p.user_id != user)
            .filter(|p| self.leader_of(&p.user_id) == Some(user))
            .map(|p| p.user_id.as_str())
            .collect()
    }

    /// What should play here now, on the service's clock.
    pub fn wanted(&self, now: u64) -> Option<Wanted> {
        match &self.session {
            Session::Alone => None,
            Session::Following(user) => {
                let leader = self.presence_of(self.leader_of(user)?)?;
                let item = leader.item.clone()?;
                Some(Wanted {
                    item,
                    position_ms: advanced(leader.position_ms, leader.playing, leader.at, now),
                    playing: leader.playing,
                    entry: None,
                })
            }
            Session::Channel(id) => {
                let channel = self.channel(id)?;
                let current = channel.current.as_ref()?;
                Some(Wanted {
                    item: current.item.clone(),
                    position_ms: advanced(channel.position_ms, channel.playing, channel.at, now),
                    playing: channel.playing,
                    entry: Some(current.id.clone()),
                })
            }
        }
    }

    /// A read that was never answered does not hold up the next.
    pub fn read_lost(&mut self) {
        self.read_in_flight = false;
    }

    /// A request for a read, unless one is on its way already.
    pub fn read(&mut self) -> Option<Request> {
        if self.read_in_flight {
            return None;
        }
        self.read_in_flight = true;
        Some(Request {
            generation: self.generation,
            op: Op::Read,
        })
    }

    /// Wraps `op`, marking everything older than it as stale when it changes a
    /// channel.
    pub fn request(&mut self, op: Op) -> Request {
        if op.changes_channel() {
            self.generation = self.generation.wrapping_add(1);
        }
        Request {
            generation: self.generation,
            op,
        }
    }

    fn mine(&mut self) -> Option<&mut Channel> {
        let Session::Channel(id) = &self.session else {
            return None;
        };
        self.snapshot.channels.iter_mut().find(|c| c.id == *id)
    }

    /// Starts following at once. The next read says what the leader plays.
    pub fn follow(&mut self, user: String) {
        self.session = Session::Following(user);
    }

    /// Shows a new channel at once, under an id the answer replaces.
    pub fn start_channel(
        &mut self,
        me: &str,
        name: &str,
        item: Option<NewPost>,
        position_ms: u64,
        playing: bool,
        now: u64,
    ) {
        for channel in &mut self.snapshot.channels {
            channel.members.retain(|m| m != me);
        }
        self.snapshot.channels.retain(|c| !c.members.is_empty());
        let current = item.map(|item| Entry {
            id: PENDING_ENTRY.into(),
            item,
            added_by: me.to_owned(),
        });
        self.snapshot.channels.push(Channel {
            id: PENDING_CHANNEL.into(),
            name: name.to_owned(),
            host_id: me.to_owned(),
            created_at: now,
            members: vec![me.to_owned()],
            playing: playing && current.is_some(),
            position_ms,
            at: now,
            current,
            queue: Vec::new(),
        });
        self.session = Session::Channel(PENDING_CHANNEL.into());
    }

    pub fn join_channel(&mut self, me: &str, id: &str) {
        for channel in &mut self.snapshot.channels {
            if channel.id == id {
                if !channel.members.iter().any(|m| m == me) {
                    channel.members.push(me.to_owned());
                }
            } else {
                channel.members.retain(|m| m != me);
            }
        }
        self.snapshot.channels.retain(|c| !c.members.is_empty());
        self.session = Session::Channel(id.to_owned());
    }

    /// Stops listening together, and says which channel to tell.
    pub fn leave(&mut self, me: &str) -> Option<String> {
        let left = match std::mem::take(&mut self.session) {
            Session::Channel(id) => Some(id),
            _ => None,
        };
        if let Some(id) = &left
            && let Some(channel) = self.snapshot.channels.iter_mut().find(|c| c.id == *id)
        {
            channel.members.retain(|m| m != me);
        }
        self.snapshot.channels.retain(|c| !c.members.is_empty());
        left
    }

    pub fn set_playback(&mut self, playing: bool, position_ms: u64, now: u64) {
        if let Some(channel) = self.mine() {
            channel.playing = playing && channel.current.is_some();
            channel.position_ms = position_ms;
            channel.at = now;
        }
    }

    /// Moves on to the queue's head, or to one of its rows and past those above.
    pub fn advance(&mut self, to: Option<&str>, now: u64) {
        let Some(channel) = self.mine() else {
            return;
        };
        let index = match to {
            Some(to) => match channel.queue.iter().position(|e| e.id == to) {
                Some(index) => index,
                None => return,
            },
            None => 0,
        };
        channel.current = (index < channel.queue.len()).then(|| channel.queue[index].clone());
        channel.queue.drain(..(index + 1).min(channel.queue.len()));
        channel.position_ms = 0;
        channel.playing = channel.current.is_some();
        channel.at = now;
    }

    pub fn add(&mut self, me: &str, item: NewPost, place: Place, now: u64) {
        self.pending_entries = self.pending_entries.wrapping_add(1);
        let entry = Entry {
            id: format!("{PENDING_ENTRY}-{}", self.pending_entries),
            item,
            added_by: me.to_owned(),
        };
        let Some(channel) = self.mine() else {
            return;
        };
        if place == Place::Now || channel.current.is_none() {
            channel.current = Some(entry);
            channel.position_ms = 0;
            channel.playing = true;
            channel.at = now;
        } else if place == Place::Next {
            channel.queue.insert(0, entry);
        } else {
            channel.queue.push(entry);
        }
    }

    pub fn remove(&mut self, entry: &str) {
        if let Some(channel) = self.mine() {
            channel.queue.retain(|e| e.id != entry);
        }
    }

    pub fn move_entry(&mut self, entry: &str, index: usize) {
        if let Some(channel) = self.mine()
            && let Some(from) = channel.queue.iter().position(|e| e.id == entry)
        {
            let moved = channel.queue.remove(from);
            let index = index.min(channel.queue.len());
            channel.queue.insert(index, moved);
        }
    }

    /// Takes in an answer. `me` decides whether the session still stands.
    pub fn receive(&mut self, event: Event, me: &str) -> Outcome {
        if event.read {
            self.read_in_flight = false;
        }
        let trip = event.received_ms.saturating_sub(event.sent_ms);
        let server_time = match &event.result {
            Ok(Update::Snapshot(snapshot)) => Some(snapshot.now),
            Ok(Update::Channel(_, now) | Update::Now(now)) => Some(*now),
            _ => None,
        };
        // The quickest round trip says the most about the clocks' difference.
        if let Some(server) = server_time
            && self.best_trip.is_none_or(|best| trip <= best)
        {
            self.best_trip = Some(trip);
            let midpoint = event.sent_ms + trip / 2;
            self.offset_ms = server as i64 - midpoint as i64;
        }
        let current = event.generation == self.generation;
        match event.result {
            Err(error) => {
                if event.read || !event.changed_channel {
                    return Outcome::Nothing;
                }
                if current && self.session == Session::Channel(PENDING_CHANNEL.into()) {
                    self.snapshot.channels.retain(|c| c.id != PENDING_CHANNEL);
                    self.session = Session::Alone;
                }
                Outcome::WriteFailed(error)
            }
            Ok(Update::Snapshot(snapshot)) => {
                let channels = self.snapshot.channels.clone();
                self.snapshot = snapshot;
                if !current {
                    // Presence is news; these channels may predate a change.
                    self.snapshot.channels = channels;
                }
                self.check_session(me, current)
            }
            Ok(Update::Channel(channel, now)) => {
                self.snapshot.now = now;
                if !current {
                    return Outcome::Nothing;
                }
                let was_pending = self.session == Session::Channel(PENDING_CHANNEL.into());
                let Some(channel) = channel else {
                    return Outcome::Nothing;
                };
                self.snapshot
                    .channels
                    .retain(|c| c.id != channel.id && c.id != PENDING_CHANNEL);
                if was_pending {
                    self.session = Session::Channel(channel.id.clone());
                }
                self.snapshot.channels.push(*channel);
                Outcome::Nothing
            }
            Ok(Update::Now(now)) => {
                self.snapshot.now = now;
                Outcome::Nothing
            }
            Ok(Update::Stats(period, stats)) => {
                self.group_stats.insert(period, stats);
                Outcome::Nothing
            }
            Ok(Update::Nothing) => Outcome::Nothing,
        }
    }

    fn check_session(&mut self, me: &str, current: bool) -> Outcome {
        match self.session.clone() {
            Session::Alone => Outcome::Nothing,
            Session::Following(user) => {
                let chain = self.leader_of(&user);
                let gone = match chain.and_then(|leader| self.presence_of(leader)) {
                    Some(leader) => leader.user_id == me,
                    None => true,
                };
                if gone {
                    self.session = Session::Alone;
                    Outcome::LeaderGone(user)
                } else {
                    Outcome::Nothing
                }
            }
            Session::Channel(id) => {
                if current
                    && id != PENDING_CHANNEL
                    && !self
                        .channel(&id)
                        .is_some_and(|c| c.members.iter().any(|m| m == me))
                {
                    self.session = Session::Alone;
                    Outcome::ChannelGone
                } else {
                    Outcome::Nothing
                }
            }
        }
    }
}

pub(super) async fn run(client: &Client, identity: &Identity, op: Op) -> Result<Update, String> {
    let channel_answer =
        |answer: ChannelAnswer| Update::Channel(answer.channel.map(Box::new), answer.now);
    let path = |channel: &str, tail: &str| -> Result<String, String> {
        valid_id(channel)?;
        Ok(format!("/live/channels/{channel}/{tail}"))
    };
    let post = async |path: String, body: serde_json::Value| -> Result<Update, String> {
        let answer: ChannelAnswer = client
            .request(Method::POST, &path, Some(identity), Some(body), None)
            .await?;
        Ok(channel_answer(answer))
    };
    match op {
        Op::Read => Ok(Update::Snapshot(
            client
                .request(Method::GET, "/live", Some(identity), None, None)
                .await?,
        )),
        Op::Presence {
            item,
            position_ms,
            playing,
            following,
        } => {
            let answer: NowAnswer = client
                .request(
                    Method::PUT,
                    "/live/presence",
                    Some(identity),
                    Some(serde_json::json!({
                        "item": item,
                        "position_ms": position_ms,
                        "playing": playing,
                        "following": following,
                    })),
                    None,
                )
                .await?;
            Ok(Update::Now(answer.now))
        }
        Op::ClearPresence => {
            let answer: NowAnswer = client
                .request(Method::DELETE, "/live/presence", Some(identity), None, None)
                .await?;
            Ok(Update::Now(answer.now))
        }
        Op::Create {
            name,
            item,
            position_ms,
            playing,
        } => {
            post(
                "/live/channels".to_owned(),
                serde_json::json!({
                    "name": name,
                    "item": item,
                    "position_ms": position_ms,
                    "playing": playing,
                }),
            )
            .await
        }
        Op::Join(channel) => post(path(&channel, "join")?, serde_json::json!({})).await,
        Op::LeaveChannel(channel) => post(path(&channel, "leave")?, serde_json::json!({})).await,
        Op::Playback {
            channel,
            playing,
            position_ms,
        } => {
            post(
                path(&channel, "playback")?,
                serde_json::json!({ "playing": playing, "position_ms": position_ms }),
            )
            .await
        }
        Op::Next { channel, from, to } => {
            post(
                path(&channel, "next")?,
                serde_json::json!({ "from": from, "to": to }),
            )
            .await
        }
        Op::Add {
            channel,
            item,
            place,
        } => {
            post(
                path(&channel, "queue")?,
                serde_json::json!({ "item": item, "where": place }),
            )
            .await
        }
        Op::Remove { channel, entry } => {
            valid_id(&entry)?;
            let answer: ChannelAnswer = client
                .request(
                    Method::DELETE,
                    &path(&channel, &format!("queue/{entry}"))?,
                    Some(identity),
                    None,
                    None,
                )
                .await?;
            Ok(channel_answer(answer))
        }
        Op::Move {
            channel,
            entry,
            index,
        } => {
            valid_id(&entry)?;
            post(
                path(&channel, &format!("queue/{entry}/move"))?,
                serde_json::json!({ "index": index }),
            )
            .await
        }
        Op::PutStats { period, summary } => {
            let answer: NowAnswer = client
                .request(
                    Method::PUT,
                    &format!("/live/stats/{period}"),
                    Some(identity),
                    Some(serde_json::json!({ "summary": summary })),
                    None,
                )
                .await?;
            Ok(Update::Now(answer.now))
        }
        Op::GetStats(period) => {
            let answer: StatsAnswer = client
                .request(
                    Method::GET,
                    &format!("/live/stats/{period}"),
                    Some(identity),
                    None,
                    None,
                )
                .await?;
            Ok(Update::Stats(period, answer.stats))
        }
        Op::DeleteStats => {
            let answer: NowAnswer = client
                .request(Method::DELETE, "/live/stats", Some(identity), None, None)
                .await?;
            Ok(Update::Now(answer.now))
        }
        Op::Forget => Ok(Update::Nothing),
    }
}

fn wall_ms() -> u64 {
    jiff::Timestamp::now().as_millisecond().max(0) as u64
}

/// Runs live requests one at a time, so changes reach the service in the order
/// they were made. A read waiting behind others is simply a later read.
pub fn spawn(
    credentials: CredentialStore,
    http: Http,
    emit: impl Fn(Event) + Send + 'static,
) -> tokio::sync::mpsc::UnboundedSender<Request> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Request>();
    tokio::spawn(async move {
        let mut connection: Option<(Client, Identity)> = None;
        while let Some(request) = rx.recv().await {
            if matches!(request.op, Op::Forget) {
                connection = None;
                continue;
            }
            if connection.is_none() {
                connection = super::pick::connect(&credentials, http.clone())
                    .await
                    .map(|(client, identity)| (client.for_live(), identity));
            }
            let generation = request.generation;
            let read = matches!(request.op, Op::Read);
            let changed_channel = request.op.changes_channel();
            let sent_ms = wall_ms();
            let result = match &connection {
                Some((client, identity)) => run(client, identity, request.op).await,
                None => Err("Join the group first.".to_owned()),
            };
            // A revoked identity is read again, in case it was replaced.
            if result.is_err() {
                connection = None;
            }
            emit(Event {
                generation,
                changed_channel,
                read,
                sent_ms,
                received_ms: wall_ms(),
                result,
            });
        }
    });
    tx
}

#[cfg(test)]
pub(super) fn answer(
    generation: u64,
    read: bool,
    changed_channel: bool,
    sent_ms: u64,
    received_ms: u64,
    result: Result<Update, String>,
) -> Event {
    Event {
        generation,
        changed_channel,
        read,
        sent_ms,
        received_ms,
        result,
    }
}
