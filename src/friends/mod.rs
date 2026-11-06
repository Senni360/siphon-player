//! The private Siphon group, independent of Spotify friend activity.
//!
//! `Action` is the redesign-facing API. `Model::begin` applies optimistic
//! edits; `Model::receive` consumes runtime events. Nothing here logs secrets.

pub mod account;
mod client;
pub mod live;
pub mod pick;
mod state;

pub use client::{
    Client, DEFAULT_BASE_URL, EventLines, Feed, Identity, NewPost, PhonePlaying, PhoneSong, Post,
    Profile, RemoteWord, Reply, Source,
};
pub use state::{
    Action, Event, Model, PENDING_REPLY, PENDING_SHARE, Request, Runtime, SongsFor, saved_identity,
};

#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod tests;
