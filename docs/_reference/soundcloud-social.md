---
title: SoundCloud Social and Track Data
description: Reposts, optional track metadata and the secondary following feed.
---

SoundCloud track and set menus offer **Repost on SoundCloud** and **Remove
SoundCloud repost**. A session connection is required; a public profile is
read-only. Controls wait for membership to load, update immediately when
used, and hold that change over lagging library reads. A failed write shows
an error and restores the previous value. No like, repost or comment counts
are added to the interface.

The secondary SoundCloud following feed is available through the application
API for the upcoming interface redesign. It contains tracks and sets posted
or reposted by followed people, with the posting person and time. It has no
new navigation page. Siphon's own friends feed remains the primary social
feed. Public-profile connections report a **Connect SoundCloud** state.

## Requests and storage

The SoundCloud web application's endpoint table, fetched from
`https://soundcloud.com/` and its
`https://a-v2.sndcdn.com/assets/55-c7bcef92.js` bundle on 2026-10-30, defines
`stream` as GET `stream` on api-v2. This is the following feed. The table also
defines `stream/posts`, `stream/notifications`, `activities` and
`activities/:filter`; those are not requested here.
The same site's `https://a-v2.sndcdn.com/assets/56-72ebd57a.js` bundle
contains the repost-ID collection parser: it reads `collection` and
`next_href` and follows pages until the cursor is absent. The endpoint-specific
collections override its request limit to 200.

Reposts use PUT or DELETE `me/track_reposts/:id` and
`me/playlist_reposts/:id`. Library refresh reads both corresponding `/ids`
endpoints with `limit=200`, following their cursors. Feed reads use GET
`stream?limit=50&linked_partitioning=1`, then its `next_href`. Only cursors on
the same API origin and the expected endpoint are accepted. Returned social
cursors are validated before caching, normalized to absolute URLs and stripped
of credential parameters and fragments. A refresh counts distinct posts when
catching up to cached pages, so overlapping pages cannot truncate the feed.
These requests
use the existing session authorization and configured proxy. Writes use the
existing no-redirect, no-retry client. No session credentials go to artwork
hosts, and expired sessions do not fall back to anonymous social requests,
including requests already waiting for a transport slot.

The existing `soundcloud-library/<user-id>-session.json` cache now contains
repost IDs and loaded feed pages as optional, backward-compatible fields.
Public-profile caches remain separate. Files use the existing private,
atomic background writer. Pending repost writes are not saved. Account
changes clear in-memory social state and reject late responses. A failed
feed page keeps the existing items and cursor; a refresh replaces a cached
prefix only after reading enough pages to catch up. Feed reads happen only
when requested, without background polling.

## Interface integration

- `Action::ToggleSoundCloudRepost(uri)` accepts `soundcloud:track:<id>` and
  `soundcloud:playlist:<id>`. Use `App::can_repost_soundcloud(uri)` for enabled
  state and `App::soundcloud_reposted(uri)` for the current value.
- `Action::LoadSoundCloudFollowing { more: false }` refreshes the feed;
  `more: true` requests its next page. Read
  `App::soundcloud_library.snapshot.following` (`items`, `next`, `loaded`)
  and `App::soundcloud_library.following.status` (`ConnectSoundCloud`, `Idle`,
  `Loading`, `Ready`, `Error`). Cached items remain usable in loading/error
  states. A feed item has `kind` (`Posted`/`Reposted`), `by`, `created_at`, and
  `content` (`Track`/`Playlist` using common application models).
- Runtime requests travel through `Command::SoundCloudLibrary` with
  `Request::Repost` or `Request::Following`. Results travel through
  `Event::SoundCloudLibrary` with `Response::Reposts`, `Response::Reposted`
  or `Response::Following`. Connection generations reject previous-account
  results; write/feed serials reject superseded completions.
- The snapshot's `reposts` contains `tracks`, `playlists` and `loaded`.
  Use the action rather than directly mutating these sets.

## Track metadata

The common `api::models::Track` has optional `release_date: Option<String>`
and `play_count: Option<u64>` fields. Dates retain the source's ISO date or
timestamp. SoundCloud fills them from `release_date`, then `display_date`,
then `created_at`, and from `playback_count`. Missing values stay absent,
including in older caches. Other source adapters can fill the same fields
when their data is available; no popularity-to-play-count conversion is used.

The shared `Track` also retains optional `label_name` and `publisher_metadata`:
`isrc`, `artist`, `album_title`, `release_title`, `writer_composer`,
`publisher`, `p_line`, and `upc_or_ean`, all optional strings. The wire and shared
models use the same `PublisherMetadata` type. ISRC is also copied to the common
`Track.external_ids.isrc` field; no catalogue match is implied by that metadata.
These fields survive mixed-playlist and account-cache round trips. Artist conversion
continues using the publisher artist when available. The existing artwork
conversion supplies the `-t500x500` variant to the common image selector,
including large cover views.

Endpoint names and the repost-ID pagination envelope were verified against the
public web bundles. Authenticated reads and writes are tested with local
fixtures, not an owner's session. The following-feed item shape still needs a
local authenticated check. These private web endpoints can change independently
of the public SoundCloud API.
