use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::credentials::{Slot, Store as CredentialStore};
use crate::http::Http;

use super::{Client, Feed, Identity, NewPost, Post, Profile, Reply};

/// The id an optimistic share carries until the service names it.
pub const PENDING_SHARE: &str = "pending-share";
/// The prefix of an optimistic reply's id until the service names it.
pub const PENDING_REPLY: &str = "pending-reply";

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
    Replied { post: String, reply: Reply },
    Changed,
}

#[derive(Default)]
pub struct Model {
    pub profile: Option<Profile>,
    pub posts: Vec<Post>,
    /// Everyone in the group, for sending a share to some of them.
    pub members: Vec<Profile>,
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
    /// else wrote, for the unread badge.
    pub fn unread_since(&self, seen: u64) -> usize {
        let me = self.profile.as_ref().map(|p| p.user_id.as_str());
        self.posts
            .iter()
            .map(|post| {
                usize::from(post.created_at > seen && Some(post.author_id.as_str()) != me)
                    + post
                        .replies
                        .iter()
                        .filter(|r| r.created_at > seen && Some(r.author_id.as_str()) != me)
                        .count()
            })
            .sum()
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
                    Update::Feed(feed, append) => {
                        if let Some(members) = feed.members {
                            self.members = members;
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
            }
        }
        self.next_poll =
            Some(Instant::now() + Duration::from_secs(60 * 2u64.pow(self.failures.min(4))));
    }
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
        let Some(identity) = &self.identity else {
            return Ok(Update::Opened(None, Feed::default()));
        };
        let update = match action {
            Action::Open => {
                let profile = client.profile(identity).await?;
                let mut feed = client.feed(identity, None).await?;
                // Members only decide who a share can be sent to; a feed
                // without them still opens.
                feed.members = client.members(identity).await.ok();
                self.members_read = Some(Instant::now());
                self.cursor.clone_from(&feed.next_cursor);
                Update::Opened(Some(profile), feed)
            }
            Action::Refresh | Action::More => {
                let more = matches!(action, Action::More);
                let mut feed = client
                    .feed(identity, if more { self.cursor.as_deref() } else { None })
                    .await?;
                // Someone may have joined; ask again now and then.
                if !more
                    && self
                        .members_read
                        .is_none_or(|at| at.elapsed() > Duration::from_secs(5 * 60))
                {
                    feed.members = client.members(identity).await.ok();
                    self.members_read = Some(Instant::now());
                }
                self.cursor.clone_from(&feed.next_cursor);
                Update::Feed(feed, more)
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
            Action::Leave | Action::Join { .. } | Action::DismissSecret => unreachable!(),
        };
        Ok(update)
    }
}
