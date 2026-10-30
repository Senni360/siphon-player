//! The private Siphon group, independent of Spotify friend activity.
//!
//! `Action` is the redesign-facing API. `Model::begin` applies optimistic
//! edits; `Model::receive` consumes runtime events. Nothing here logs secrets.

mod client;
mod state;
pub mod ui;

pub use client::{Client, DEFAULT_BASE_URL, Feed, Identity, NewPost, Post, Profile, Source};
pub use state::{Action, Event, Model, Request, Runtime};

#[cfg(test)]
mod tests;
