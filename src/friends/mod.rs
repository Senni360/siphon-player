//! The private Siphon group, independent of Spotify friend activity.
//!
//! `Action` is the redesign-facing API. `Model::begin` applies optimistic
//! edits; `Model::receive` consumes runtime events. Nothing here logs secrets.

mod client;
pub mod pick;
mod state;

pub use client::{Client, DEFAULT_BASE_URL, Feed, Identity, NewPost, Post, Profile, Reply, Source};
pub use state::{Action, Event, Model, PENDING_REPLY, PENDING_SHARE, Request, Runtime};

#[cfg(test)]
mod tests;
