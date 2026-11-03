use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::credentials::{Slot, Store as CredentialStore};
use crate::http::Http;

use super::account::{self, Account, Op, SharedPlaylist};
use super::{Client, Feed, Identity, NewPost, Post, Profile, Reply};

/// The id an optimistic share carries until the service names it.
pub const PENDING_SHARE: &str = "pending-share";
/// The prefix of an optimistic reply's id until the service names it.
pub const PENDING_REPLY: &str = "pending-reply";
/// Below this estimated affinity, in percent, a group share does not raise
/// the unread badge.
const QUIET_BELOW: u8 = 35;
/// From this estimated affinity, in percent, a recent share leads Home.
const LOVED_FROM: u8 = 70;
/// How far back in the feed Home looks for shares to lead with.
const PICK_FROM: usize = 10;

#[derive(Clone)]
pub enum Action {
    Open,
    Join {
        invite_code: String,
        display_name: String,
    },
    Rename(String),
    /// Forget the credential on this device, including while offline.
    Leave,
    /// Revoke group membership remotely, then forget the device credential.
    LeaveGroup,
    /// Prove who this listener is; the backend adds the Spotify token.
    ConnectSpotify,
    /// What `ConnectSpotify` becomes once the backend has the token.
    ConnectWithToken(String),
    /// A change to the account or to one of its groups.
    Account(Op),
    /// Look at another group. Nothing changes on the service.
    SelectGroup(String),
    SharePlaylist {
        name: String,
        items: Vec<NewPost>,
        to: Vec<String>,
    },
    DeleteSharedPlaylist(String),
    /// Upload a song from this computer for the group.
    UploadSong(std::path::PathBuf),
    DeleteSong(String),
    /// Fetch a shared list's songs, for `purpose`.
    OpenSharedPlaylist {
        id: String,
        purpose: SongsFor,
    },
    Refresh,
    More,
    Share(NewPost),
    Like {
        id: String,
        liked: bool,
    },
    Delete(String),
    /// Add a message to a share's thread.
    Reply {
        post: String,
        text: String,
    },
    /// Remove one of your own messages from a share's thread.
    DeleteReply {
        post: String,
        reply: String,
    },
    DismissSecret,
}

/// What a shared playlist's songs were fetched for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SongsFor {
    SaveCopy,
    PartyBackground,
    Play,
}

impl Action {
    /// Reads can be dropped or superseded; writes are what the person did
    /// and are never lost while another request runs.
    fn is_read(&self) -> bool {
        matches!(self, Self::Open | Self::Refresh | Self::More)
    }

    fn is_queueable_write(&self) -> bool {
        matches!(
            self,
            Self::Share(_)
                | Self::Like { .. }
                | Self::Delete(_)
                | Self::Rename(_)
                | Self::Reply { .. }
                | Self::DeleteReply { .. }
                | Self::Account(_)
                | Self::SharePlaylist { .. }
                | Self::DeleteSharedPlaylist(_)
                | Self::UploadSong(_)
                | Self::DeleteSong(_)
                | Self::OpenSharedPlaylist { .. }
        )
    }
}

// Parent Action/Command enums use Debug. Do not expose invites, URLs (private
// SoundCloud links), or the one-time credential through their debug output.
impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Open => "Friends::Open",
            Self::Join { .. } => "Friends::Join",
            Self::Rename(_) => "Friends::Rename",
            Self::Leave => "Friends::Leave",
            Self::LeaveGroup => "Friends::LeaveGroup",
            Self::ConnectSpotify | Self::ConnectWithToken(_) => "Friends::ConnectSpotify",
            Self::Account(_) => "Friends::Account",
            Self::SelectGroup(_) => "Friends::SelectGroup",
            Self::SharePlaylist { .. } => "Friends::SharePlaylist",
            Self::DeleteSharedPlaylist(_) => "Friends::DeleteSharedPlaylist",
            Self::UploadSong(_) => "Friends::UploadSong",
            Self::DeleteSong(_) => "Friends::DeleteSong",
            Self::OpenSharedPlaylist { .. } => "Friends::OpenSharedPlaylist",
            Self::Refresh => "Friends::Refresh",
            Self::More => "Friends::More",
            Self::Share(_) => "Friends::Share",
            Self::Like { .. } => "Friends::Like",
            Self::Delete(_) => "Friends::Delete",
            Self::Reply { .. } => "Friends::Reply",
            Self::DeleteReply { .. } => "Friends::DeleteReply",
            Self::DismissSecret => "Friends::DismissSecret",
        })
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub generation: u64,
    pub action: Action,
}

pub struct Event {
    pub generation: u64,
    pub result: Result<Update, String>,
}

impl std::fmt::Debug for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Friends::Event")
    }
}

pub enum Update {
    Opened(Option<Profile>, Feed),
    Joined(Profile, String, Option<String>),
    Renamed(Profile),
    Left,
    Feed(Feed, bool),
    Shared(Post),
    Replied {
        post: String,
        reply: Reply,
    },
    Changed,
    /// Signed in with Spotify: this computer is this account now.
    Connected(Profile, Box<Feed>),
    /// The account as it stands after a change.
    Account(Box<Account>),
    PlaylistShared(SharedPlaylist),
    SongUploaded(crate::shared_songs::SharedSong),
    SharedSongs {
        name: String,
        songs: Vec<NewPost>,
        purpose: SongsFor,
    },
}

#[derive(Default)]
pub struct Model {
    pub profile: Option<Profile>,
    pub posts: Vec<Post>,
    /// Everyone in the group, for sending a share to some of them.
    pub members: Vec<Profile>,
    /// None until read, and from a service that predates accounts.
    pub account: Option<Account>,
    /// What the chosen group has had shared with it, or with this listener.
    pub shared_playlists: Vec<SharedPlaylist>,
    /// Songs people shared from their own files.
    pub shared_songs: Vec<crate::shared_songs::SharedSong>,
    pub songs_used: u64,
    /// Zero until read, and from a service with no file store.
    pub songs_capacity: u64,
    /// Songs fetched for a purpose, for the app to act on.
    pub songs_ready: Option<(String, Vec<NewPost>, SongsFor)>,
    pub next_cursor: Option<String>,
    pub busy: bool,
    pub error: Option<String>,
    /// Available once after join. Never persisted in ordinary settings.
    pub recovery_secret: Option<String>,
    pub restored: bool,
    generation: u64,
    next_poll: Option<Instant>,
    failures: u32,
    optimistic: bool,
    /// Whether the request in flight only reads, so a write may replace it.
    reading: bool,
    /// Writes made while another write was in flight, already shown, with
    /// the optimistic reply row each one stands for.
    queued: VecDeque<(Action, Option<String>)>,
    /// The optimistic reply row the request in flight will replace.
    in_flight_reply: Option<String>,
    pending_replies: u64,
}

impl Model {
    pub fn begin(&mut self, action: Action) -> Option<Request> {
        if matches!(action, Action::DismissSecret) {
            self.recovery_secret = None;
            return None;
        }
        let pending = self.apply_optimistic(&action);
        if let Action::SelectGroup(id) = &action {
            // Another group's songs must not linger under its name.
            account::set_current_group(Some(id.clone()));
            self.posts.clear();
            self.shared_playlists.clear();
            self.shared_songs.clear();
            self.next_cursor = None;
            self.members = self
                .account
                .as_ref()
                .and_then(|account| account.groups.iter().find(|group| group.id == *id))
                .map(account::Group::profiles)
                .unwrap_or_default();
        }
        if self.busy && !matches!(action, Action::Leave | Action::LeaveGroup) {
            // A write never waits on a read: it replaces it, and the read
            // runs again after it. A write behind another write waits its
            // turn, already shown.
            if self.reading && !action.is_read() {
                self.next_poll = Some(Instant::now());
            } else {
                if action.is_queueable_write() {
                    self.queued.push_back((action, pending));
                }
                return None;
            }
        }
        if matches!(action, Action::More) && self.next_cursor.is_none() {
            return None;
        }
        Some(self.request(action, pending))
    }

    fn request(&mut self, action: Action, pending: Option<String>) -> Request {
        let generation = self.generation.wrapping_add(1);
        if matches!(
            action,
            Action::Leave | Action::LeaveGroup | Action::Join { .. }
        ) {
            *self = Self {
                generation,
                restored: true,
                ..Self::default()
            };
        }
        self.generation = generation;
        self.in_flight_reply = pending;
        self.busy = true;
        self.reading = action.is_read();
        self.error = None;
        Request {
            generation: self.generation,
            action,
        }
    }

    /// Shows a write at once, before the service has it. Returns the id
    /// of an optimistic reply row, which the service's answer replaces.
    fn apply_optimistic(&mut self, action: &Action) -> Option<String> {
        match action {
            Action::Like { id, liked } => {
                if let Some(post) = self.posts.iter_mut().find(|p| p.id == *id)
                    && post.liked != *liked
                {
                    post.like_count = if *liked {
                        post.like_count.saturating_add(1)
                    } else {
                        post.like_count.saturating_sub(1)
                    };
                    post.liked = *liked;
                }
                self.optimistic = true;
            }
            Action::Delete(id) => {
                self.posts.retain(|p| p.id != *id);
                self.optimistic = true;
            }
            Action::Rename(name) => {
                if let Some(profile) = &mut self.profile {
                    profile.display_name.clone_from(name);
                    for p in &mut self.posts {
                        if p.author_id == profile.user_id {
                            p.author_name.clone_from(name);
                        }
                    }
                }
                self.optimistic = true;
            }
            Action::Share(item) => {
                if let Some(profile) = &self.profile {
                    self.posts.insert(
                        0,
                        Post {
                            id: PENDING_SHARE.into(),
                            author_id: profile.user_id.clone(),
                            author_name: profile.display_name.clone(),
                            created_at: jiff::Timestamp::now().as_millisecond().max(0) as u64,
                            like_count: 0,
                            liked: false,
                            replies: Vec::new(),
                            affinity: None,
                            item: item.clone(),
                        },
                    );
                }
                self.optimistic = true;
            }
            Action::Reply { post, text } => {
                self.pending_replies = self.pending_replies.wrapping_add(1);
                let pending = format!("{PENDING_REPLY}-{}", self.pending_replies);
                if let Some(profile) = &self.profile
                    && let Some(target) = self.posts.iter_mut().find(|p| p.id == *post)
                {
                    target.replies.push(Reply {
                        id: pending.clone(),
                        author_id: profile.user_id.clone(),
                        author_name: profile.display_name.clone(),
                        created_at: jiff::Timestamp::now().as_millisecond().max(0) as u64,
                        text: text.clone(),
                    });
                }
                self.optimistic = true;
                return Some(pending);
            }
            Action::DeleteReply { post, reply } => {
                if let Some(target) = self.posts.iter_mut().find(|p| p.id == *post) {
                    target.replies.retain(|r| r.id != *reply);
                }
                self.optimistic = true;
            }
            Action::Account(op) => {
                if let Some(account) = &mut self.account {
                    account::apply_locally(account, op);
                }
                self.optimistic = true;
            }
            Action::DeleteSharedPlaylist(id) => {
                self.shared_playlists.retain(|list| list.id != *id);
                self.optimistic = true;
            }
            Action::DeleteSong(id) => {
                self.shared_songs.retain(|song| song.id != *id);
                self.optimistic = true;
            }
            _ => {}
        }
        None
    }

    /// The next write made while another was in flight, once that one has
    /// answered. Call after `receive`.
    pub fn next_request(&mut self) -> Option<Request> {
        if self.busy {
            return None;
        }
        let (action, pending) = self.queued.pop_front()?;
        Some(self.request(action, pending))
    }

    pub fn poll_due(&self, visible: bool) -> bool {
        visible
            && self.profile.is_some()
            && !self.busy
            && !self.optimistic
            && self.next_poll.is_none_or(|at| Instant::now() >= at)
    }

    /// Shares and replies newer than `seen` (milliseconds) that someone
    /// else wrote, for the unread badge. A share to the whole group that the
    /// reader will probably not care about arrives quietly; one sent to them
    /// always counts.
    pub fn unread_since(&self, seen: u64) -> usize {
        let me = self.profile.as_ref().map(|p| p.user_id.as_str());
        self.posts
            .iter()
            .map(|post| {
                let quiet = post.affinity.is_some_and(|p| p < QUIET_BELOW)
                    && !me.is_some_and(|me| post.item.to.iter().any(|to| to == me));
                usize::from(post.created_at > seen && Some(post.author_id.as_str()) != me && !quiet)
                    + post
                        .replies
                        .iter()
                        .filter(|r| r.created_at > seen && Some(r.author_id.as_str()) != me)
                        .count()
            })
            .sum()
    }

    /// The shares Home leads with: of the newest few, the ones the reader
    /// will probably love come first, each group still newest first.
    pub fn picks(&self, count: usize) -> Vec<&Post> {
        let mut recent: Vec<&Post> = self.posts.iter().take(PICK_FROM).collect();
        recent
            .sort_by_key(|post| std::cmp::Reverse(post.affinity.is_some_and(|p| p >= LOVED_FROM)));
        recent.truncate(count);
        recent
    }

    /// The newest moment anything in the feed happened, in milliseconds.
    pub fn newest(&self) -> u64 {
        self.posts
            .iter()
            .flat_map(|p| {
                std::iter::once(p.created_at).chain(p.replies.iter().map(|r| r.created_at))
            })
            .max()
            .unwrap_or(0)
    }

    pub fn receive(&mut self, event: Event) {
        if event.generation != self.generation {
            return;
        }
        self.busy = false;
        self.reading = false;
        match event.result {
            Ok(update) => {
                self.failures = 0;
                self.optimistic = !self.queued.is_empty();
                match update {
                    Update::Opened(profile, feed) => {
                        if let Some(members) = feed.members {
                            self.members = members;
                        }
                        self.account = feed.account;
                        self.shared_playlists = feed.shared_playlists;
                        self.restored = true;
                        self.profile = profile;
                        self.posts = feed.posts;
                        self.next_cursor = feed.next_cursor;
                    }
                    Update::Joined(profile, secret, warning) => {
                        self.profile = Some(profile);
                        self.recovery_secret = Some(secret);
                        self.error = warning;
                        self.next_poll = Some(Instant::now());
                        return;
                    }
                    Update::Renamed(profile) => {
                        for post in &mut self.posts {
                            if post.author_id == profile.user_id {
                                post.author_name.clone_from(&profile.display_name);
                            }
                        }
                        if let Some(member) = self
                            .members
                            .iter_mut()
                            .find(|m| m.user_id == profile.user_id)
                        {
                            member.display_name.clone_from(&profile.display_name);
                        }
                        self.profile = Some(profile);
                    }
                    Update::Left => {}
                    Update::Connected(profile, feed) => {
                        self.restored = true;
                        self.profile = Some(profile);
                        self.account = feed.account;
                        self.members = feed.members.unwrap_or_default();
                        self.shared_playlists = feed.shared_playlists;
                        self.shared_songs = feed.shared_songs;
                        self.songs_used = feed.songs_used;
                        self.songs_capacity = feed.songs_capacity;
                        self.posts = feed.posts;
                        self.next_cursor = feed.next_cursor;
                    }
                    Update::Account(account) => {
                        let chosen = account::current_group();
                        self.members = account
                            .chosen_group(chosen.as_deref())
                            .map(account::Group::profiles)
                            .unwrap_or_default();
                        // A group this listener just lost must not stay on screen.
                        if chosen.is_some()
                            && account.chosen_group(chosen.as_deref()).map(|g| &g.id)
                                != chosen.as_ref()
                        {
                            self.posts.clear();
                            self.shared_playlists.clear();
                            self.shared_songs.clear();
                            self.next_cursor = None;
                            account::set_current_group(
                                account.groups.first().map(|group| group.id.clone()),
                            );
                        }
                        self.account = Some(*account);
                    }
                    Update::PlaylistShared(list) => {
                        self.shared_playlists.retain(|row| row.id != list.id);
                        self.shared_playlists.insert(0, list);
                    }
                    Update::SongUploaded(song) => {
                        self.songs_used += song.size;
                        self.shared_songs.retain(|row| row.id != song.id);
                        self.shared_songs.insert(0, song);
                    }
                    Update::SharedSongs {
                        name,
                        songs,
                        purpose,
                    } => self.songs_ready = Some((name, songs, purpose)),
                    Update::Feed(feed, append) => {
                        if let Some(members) = feed.members {
                            self.members = members;
                        }
                        if feed.account.is_some() {
                            self.account = feed.account;
                            self.shared_playlists = feed.shared_playlists;
                            self.shared_songs = feed.shared_songs;
                            self.songs_used = feed.songs_used;
                            self.songs_capacity = feed.songs_capacity;
                        }
                        if append {
                            for post in feed.posts {
                                if !self.posts.iter().any(|p| p.id == post.id) {
                                    self.posts.push(post);
                                }
                            }
                        } else {
                            self.posts = feed.posts;
                        }
                        self.next_cursor = feed.next_cursor;
                    }
                    Update::Shared(post) => {
                        if let Some(index) = self.posts.iter().position(|p| p.id == PENDING_SHARE) {
                            self.posts[index] = post.clone();
                        } else {
                            self.posts.insert(0, post.clone());
                        }
                        let mut seen = false;
                        self.posts.retain(|p| {
                            if p.id != post.id {
                                return true;
                            }
                            !std::mem::replace(&mut seen, true)
                        });
                    }
                    Update::Replied { post, reply } => {
                        let pending = self.in_flight_reply.take();
                        if let Some(target) = self.posts.iter_mut().find(|p| p.id == post) {
                            match target
                                .replies
                                .iter_mut()
                                .find(|r| Some(&r.id) == pending.as_ref())
                            {
                                Some(row) => *row = reply,
                                None => target.replies.push(reply),
                            }
                        }
                    }
                    Update::Changed => {}
                }
            }
            Err(error) => {
                self.failures = self.failures.saturating_add(1);
                // An optimistic share or reply that failed is taken back,
                // so nothing stays on screen that the group never got.
                if !self
                    .queued
                    .iter()
                    .any(|(action, _)| matches!(action, Action::Share(_)))
                {
                    self.posts.retain(|p| p.id != PENDING_SHARE);
                }
                if let Some(pending) = self.in_flight_reply.take() {
                    for post in &mut self.posts {
                        post.replies.retain(|r| r.id != pending);
                    }
                }
                self.error = Some(error);
                // What was shown at once and did not happen comes back with the
                // next read, soon. A read that failed just waits longer.
                if std::mem::take(&mut self.optimistic) && self.queued.is_empty() {
                    self.next_poll = Some(Instant::now());
                    return;
                }
            }
        }
        self.next_poll =
            Some(Instant::now() + Duration::from_secs(60 * 2u64.pow(self.failures.min(4))));
    }
}

/// The saved sign-in, for a song's stream, which plays outside this runtime.
pub async fn saved_identity(credentials: &crate::credentials::Store) -> Option<Identity> {
    let bytes = credentials
        .lease(Slot::Friends)
        .load_secret()
        .await
        .ok()??;
    serde_json::from_slice(&bytes).ok()
}

/// One actor on the existing backend runtime. A newer request (only sign-out
/// actions are allowed while busy) cancels pending network work immediately.
/// Credential leases prevent a cancelled join from saving after sign-out.
#[derive(Default)]
pub struct Runtime {
    identity: Option<Identity>,
    loaded: bool,
    cursor: Option<String>,
    members_read: Option<Instant>,
}

impl Runtime {
    pub fn spawn(
        credentials: CredentialStore,
        emit: impl Fn(Event) + Send + 'static,
    ) -> tokio::sync::mpsc::UnboundedSender<(Request, Http)> {
        Self::spawn_with_client(credentials, emit, Client::configured)
    }

    pub(super) fn spawn_with_client(
        credentials: CredentialStore,
        emit: impl Fn(Event) + Send + 'static,
        client: impl Fn(Http) -> Result<Client, String> + Send + 'static,
    ) -> tokio::sync::mpsc::UnboundedSender<(Request, Http)> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(Request, Http)>();
        tokio::spawn(async move {
            let mut runtime = Self::default();
            let mut pending = None;
            loop {
                let Some((request, http)) = (match pending.take() {
                    Some(request) => Some(request),
                    None => rx.recv().await,
                }) else {
                    break;
                };
                let generation = request.generation;
                tokio::select! {
                    result = runtime.execute(request.action, client(http), &credentials) => {
                        emit(Event { generation, result });
                    }
                    newer = rx.recv() => {
                        let Some(newer) = newer else { break; };
                        pending = Some(newer);
                    }
                }
            }
        });
        tx
    }

    async fn execute(
        &mut self,
        action: Action,
        client: Result<Client, String>,
        credentials: &CredentialStore,
    ) -> Result<Update, String> {
        if matches!(action, Action::Leave) {
            self.identity = None;
            self.cursor = None;
            self.loaded = true;
            credentials.invalidate(Slot::Friends);
            credentials
                .lease(Slot::Friends)
                .clear_secret(false)
                .await
                .map_err(|_| "Friends sign-out could not be saved. Try Leave again.".to_owned())?;
            return Ok(Update::Left);
        }
        let client = client?;
        if matches!(action, Action::Join { .. }) {
            let Action::Join {
                invite_code,
                display_name,
            } = action
            else {
                unreachable!()
            };
            // Join is a new identity boundary, even if its request fails.
            // Revoke any prior local identity before issuing the request.
            self.identity = None;
            self.cursor = None;
            self.loaded = true;
            credentials.invalidate(Slot::Friends);
            let lease = credentials.lease(Slot::Friends);
            let cleared = lease.clear_secret(true).await;
            if matches!(
                cleared,
                Err(crate::credentials::Error::Filesystem
                    | crate::credentials::Error::Invalid
                    | crate::credentials::Error::Stale)
            ) {
                return Err(
                    "The previous Friends identity could not be forgotten. Check the app's state folder and retry."
                        .into(),
                );
            }
            let identity = client.join(&invite_code, &display_name).await?;
            let bytes = serde_json::to_vec(&identity)
                .map_err(|_| "Friends identity could not be encoded.")?;
            // A failed removal may still have safely written the revocation
            // marker. Never remember a new identity over an uncertain removal.
            let saved = match cleared {
                Ok(()) => lease.save_secret(bytes).await,
                Err(error) => Err(error),
            };
            let profile = Profile {
                user_id: identity.user_id.clone(),
                display_name: display_name.trim().to_owned(),
            };
            let secret = identity.secret.clone();
            self.identity = Some(identity);
            let warning = saved.err().map(|_| "Joined for this session. The protected credential store could not remember it. Keep your secret before leaving.".into());
            return Ok(Update::Joined(profile, secret, warning));
        }
        if !self.loaded {
            self.identity = credentials
                .lease(Slot::Friends)
                .load_secret()
                .await
                .map_err(|_| "Unlock the credential store to restore Friends.".to_owned())?
                .map(|bytes| {
                    serde_json::from_slice(&bytes)
                        .map_err(|_| "Stored Friends identity is invalid.".to_owned())
                })
                .transpose()?;
            self.loaded = true;
        }
        if let Action::ConnectWithToken(token) = action {
            let (identity, display_name) = client
                .sign_in(&token, &device_name(), self.identity.as_ref())
                .await?;
            // A failed save still signs this session in; the sign-in is cheap to repeat.
            let bytes = serde_json::to_vec(&identity)
                .map_err(|_| "Friends identity could not be encoded.")?;
            let lease = credentials.lease(Slot::Friends);
            let saved = lease.save_secret(bytes).await;
            let profile = Profile {
                user_id: identity.user_id.clone(),
                display_name,
            };
            self.identity = Some(identity);
            let identity = self.identity.clone().expect("just set");
            let feed = self.read_group(&client, &identity, None, true).await?;
            self.cursor.clone_from(&feed.next_cursor);
            if saved.is_err() {
                log::warn!("the group sign-in could not be remembered");
            }
            return Ok(Update::Connected(profile, Box::new(feed)));
        }
        let Some(identity) = self.identity.clone() else {
            return Ok(Update::Opened(None, Feed::default()));
        };
        let identity = &identity;
        let update = match action {
            Action::Open => {
                let profile = client.profile(identity).await?;
                let feed = self.read_group(&client, identity, None, true).await?;
                self.cursor.clone_from(&feed.next_cursor);
                Update::Opened(Some(profile), feed)
            }
            Action::Refresh | Action::SelectGroup(_) => {
                let feed = self.read_group(&client, identity, None, false).await?;
                self.cursor.clone_from(&feed.next_cursor);
                Update::Feed(feed, false)
            }
            Action::More => {
                let cursor = self.cursor.clone();
                let feed = client.feed(identity, cursor.as_deref()).await?;
                self.cursor.clone_from(&feed.next_cursor);
                Update::Feed(feed, true)
            }
            Action::Account(op) => {
                client.apply(identity, op).await?;
                Update::Account(Box::new(client.account(identity).await?))
            }
            Action::SharePlaylist { name, items, to } => {
                Update::PlaylistShared(client.share_playlist(identity, &name, &items, &to).await?)
            }
            Action::DeleteSharedPlaylist(id) => {
                client.delete_shared_playlist(identity, &id).await?;
                Update::Changed
            }
            Action::UploadSong(path) => {
                Update::SongUploaded(client.upload_song(identity, path).await?)
            }
            Action::DeleteSong(id) => {
                client.delete_song(identity, &id).await?;
                Update::Changed
            }
            Action::OpenSharedPlaylist { id, purpose } => {
                let (name, songs) = client.shared_playlist_songs(identity, &id).await?;
                Update::SharedSongs {
                    name,
                    songs,
                    purpose,
                }
            }
            Action::Rename(name) => Update::Renamed(client.rename(identity, &name).await?),
            Action::Reply { post, text } => {
                let reply = client.reply(identity, &post, &text).await?;
                Update::Replied { post, reply }
            }
            Action::DeleteReply { post, reply } => {
                client.delete_reply(identity, &post, &reply).await?;
                Update::Changed
            }
            Action::Share(post) => Update::Shared(client.share(identity, &post).await?),
            Action::Like { id, liked } => {
                client.like(identity, &id, liked).await?;
                Update::Changed
            }
            Action::Delete(id) => {
                client.delete(identity, &id).await?;
                Update::Changed
            }
            Action::LeaveGroup => {
                client.leave(identity).await.map_err(|_| {
                    "Leaving the group could not be confirmed. Retry your saved identity, or forget this device.".to_owned()
                })?;
                self.identity = None;
                self.cursor = None;
                credentials.invalidate(Slot::Friends);
                credentials
                    .lease(Slot::Friends)
                    .clear_secret(false)
                    .await
                    .map_err(|_| {
                        "Left the group, but credential removal failed. Forget this device again."
                            .to_owned()
                    })?;
                Update::Left
            }
            Action::Leave
            | Action::Join { .. }
            | Action::DismissSecret
            | Action::ConnectSpotify
            | Action::ConnectWithToken(_) => unreachable!(),
        };
        Ok(update)
    }

    /// The account, the group to look at within it, and that group's feed.
    /// The account comes first: someone just removed from a group must not ask
    /// it for a feed. A service from before accounts answers none, and the one
    /// group it has is read as it always was.
    async fn read_group(
        &mut self,
        client: &Client,
        identity: &Identity,
        cursor: Option<&str>,
        always_members: bool,
    ) -> Result<Feed, String> {
        let account = client.account(identity).await.ok();
        let chosen = account::current_group();
        let group = account
            .as_ref()
            .and_then(|account| account.chosen_group(chosen.as_deref()));
        if let Some(account) = &account {
            account::set_current_group(group.map(|group| group.id.clone()));
            if account.groups.is_empty() {
                return Ok(Feed {
                    members: Some(Vec::new()),
                    account: Some(account.clone()),
                    ..Feed::default()
                });
            }
        }
        let mut feed = client.feed(identity, cursor).await?;
        // Members only decide who a share can be sent to; a feed without them
        // still opens. An account already says who they are.
        if let Some(group) = group {
            feed.members = Some(group.profiles());
        } else if always_members
            || self
                .members_read
                .is_none_or(|at| at.elapsed() > Duration::from_secs(5 * 60))
        {
            feed.members = client.members(identity).await.ok();
            self.members_read = Some(Instant::now());
        }
        if account.is_some() {
            feed.shared_playlists = client.shared_playlists(identity).await.unwrap_or_default();
            // A service with no file store just has none to list.
            if let Ok((songs, used, capacity)) = client.shared_songs(identity).await {
                feed.shared_songs = songs;
                feed.songs_used = used;
                feed.songs_capacity = capacity;
            }
        }
        feed.account = account;
        Ok(feed)
    }
}

/// How a signed-in computer shows up in the list of them.
fn device_name() -> String {
    let name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect::<String>()
        .trim()
        .to_owned();
    if clean.is_empty() {
        "This computer".to_owned()
    } else {
        clean
    }
}
