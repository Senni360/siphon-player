---
title: Siphon Frontend Handoff
description: Frontend status, shared design decisions and interface requirements for the Siphon redesign.
nav_exclude: true
---

# Siphon frontend handoff

Owner: Claude (frontend and design). Backend owns audio, APIs, auth,
persistence, caching and services. The local Windows thread owns platform
integration, merging and native verification. This file is the frontend's
report and its interface requirements. It does not replace the backend's
`docs/_reference/siphon-integration-contract.md`; where the two disagree, the
backend contract wins on behaviour and this file is updated to match.

## Status (2026-10-30, 18:20 Amsterdam)

- Direction **D (Hybrid)** is approved and recorded in `DESIGN.md`.
- **Frontend branch:** `claude/focused-rubin-pxfk32`. It stays append-only
  (no force pushes) and is based on `siphon/backend-delivery` (`8b94e5b`).
  Every commit compiles and passes clippy and the library tests.
- **For `siphon/evening-candidate`:** it already carries the frontend through
  `0d2943a`. Cherry-pick the frontend commits after `0d2943a`, in order:
  `git cherry-pick 0d2943a..origin/claude/focused-rubin-pxfk32`. Rehearsed
  at 18:10 on `1823bf6` with the frontend at `9fab9be`: two conflicts, both
  in `src/demo.rs`.
  1. At `f5341e4` (player island): four hunks in the SoundCloud source
     tests the evening branch already rewrote. Keep the evening side of
     every hunk, then make one change: the player-bar part of
     `soundcloud_source_indicators_follow_tracks_into_the_queue_and_player_bar`
     asserts that "SC" is **absent** (the island colours the title; the
     chip is gone everywhere).
  2. At `9fab9be` (account scoping): one hunk where the evening branch added
     the helper `source_title_rects` and the frontend added the test
     `the_island_title_takes_its_source_colour` at the same place. Keep
     both, the helper first.

  The rehearsed result: clippy (`--no-default-features --features demo
  --all-targets -D warnings`) clean, `cargo fmt --check` clean, 1,896
  library tests passed. One backend test,
  `cast::media::tests::errors_and_statuses_cannot_expose_signed_urls`,
  fails only when `RUST_BACKTRACE=1` is set: anyhow's `Debug` output then
  includes backtrace frames named `anyhow::__private`, which contain the
  word the test forbids. It passes with backtraces off. Backend owner:
  consider checking `Display` (`{error:#}`) instead.
- **Review fixes (PR 7 comment):** the last-used playlist and the island's
  song snapshots belong to the signed-in account, and sign-out forgets them
  and closes Now playing (`another_account_never_sees_the_last_playlist`).
  Row and Now playing downloads ask `app.download_source`. Rows and header
  share one column rule (the album narrows to 140 px, then gives way, then
  the tools do; the title keeps 220 px), tested at 560/561, 760/761 and
  920/921. The island
  title's source colour is tested in both themes.
- The final frontend head for tonight is published here by 20:30 Amsterdam,
  with this section updated.

## Decisions carried into D

- **Source colour is on the title, never a chip.** Dark theme: Spotify
  `#1ED760`, SoundCloud `#FF8A1F`, YouTube `#FF4848`. Light theme: Spotify
  `#097F3A`, SoundCloud `#B84D00`, YouTube `#CC1F26` (darker than the
  mockups so each title keeps 4.5:1 on the light window). Orange is pushed toward
  yellow and red toward crimson so the two never read as the same colour.
  Everything else is neutral (zinc greys). The brand accent is the text
  colour itself, never green, so the app is not a Spotify recolour.
- **Four square row tools, always in this order:** Download, BPM/key,
  Credits, Add to last-used playlist. 30 px squares with 16 px icons and a
  tooltip that states the result ("124 BPM · A minor", "Add to Night drive
  (last used)"). A disabled tool keeps its tooltip and says why ("Download
  needs the Siphon helper").
- **Unavailable is shown, never invented.** An unplayable song dims and its
  subtitle gives the reason. A missing BPM/key shows "Analysing…" or "–".
  Credits for uploads say they come from the upload.
- **Partial failure stays partial.** If Spotify stops answering, SoundCloud
  and YouTube rows stay and a banner offers Retry.
- **Typography:** Geist for UI and Geist Mono for numbers (BPM, key, times,
  ISRC). Both are OFL. Bundling two Geist weights plus Geist Mono replaces
  Inter; the size cost is measured before it lands.
- **Motion:** hover and panel transitions at the existing 0.12 s egui
  easing; anything longer is a one-shot open/close. Nothing animates while
  the window is idle, hidden or the system asks for reduced motion
  (`src/motion.rs`). Skeleton shimmer stops under reduced motion.

## Native feasibility notes

- Liquid glass (direction B) is not real backdrop blur. egui on glow cannot
  sample what is behind a panel cheaply every frame. The plan is a single
  pre-blurred, down-scaled copy of the current artwork (blurred once on the
  image worker when the song changes, cached with the art), painted behind
  the glass panels at low alpha, plus a 1 px top highlight and a hairline
  border. Static, so it costs nothing while idle.
- Noise (B's now-playing stage) is one tiny tiling texture uploaded once,
  painted at low alpha. No per-frame shader.
- Artwork tint already exists (`Palette::tint_from_art`). A, C use it only in
  the now-playing art area; B uses it for the stage and the playlist header.
- Drawers and the B now-playing stage animate with egui's `animate_bool`
  and request repaints only while the value moves.
- No waveform seek bar, visualisers, video, podcasts or translations in any
  direction.

## Interface requirements for the backend

What the UI needs to read and the actions it emits. Names are suggestions;
the backend contract decides. Each item lists what exists today.

1. **Source of an item.** `fn source(&PlayableItem) -> Source { Spotify,
   SoundCloud, YouTube }`. Today SoundCloud is detected by URI
   (`soundcloud::is_track_uri`); YouTube is on `siphon/youtube`. The UI
   colours titles from this one function.
2. **Availability.** `Option<Unavailable { reason: String }>` per item
   (region, removed, private link expired). Spotify gives `is_playable`.
3. **Helper status.** `Running | NotRunning | Starting`, plus whether stems
   are supported. Today `downloads::State::available` is a bool. Action:
   `StartHelper` if the backend can launch it, otherwise the UI only links
   to the install guide.
4. **Analysis (BPM/key).** Per item: `Pending | Ready { bpm: f32, key:
   String, camelot: Option<String> } | Failed(String) | Unavailable`.
   Actions: `Analyse(item)`, `CancelAnalysis(item)`. Cached by the backend;
   the UI never blocks on it.
5. **Download options.** `downloads::Request` needs `format: Wav | Mp3_320
   | Flac`, `stems: bool` and an optional DAW-ready file name. Progress
   already has `stage`, `fraction`, `cancelling`; `Summary.last_file` serves
   "Show folder".
6. **Credits.** `Loading | Ready(Credits { origin: Spotify | Upload, roles:
   Vec<(String, Vec<String>)>, label, released, isrc }) | Unavailable` (see
   `siphon/credits-isrc`).
7. **Last-used playlist.** The id and name of the playlist most recently
   added to, persisted in the state file (not a setting). Action:
   `AddToLastPlaylist(items)`; right-click opens the full picker.
8. **Home.** New releases (Spotify followed artists plus SoundCloud followed
   uploads, each with source and release date) and the blended For you mix
   (Spotify plus SoundCloud only, with source per item). Each list reports
   `Loading | Ready | Empty | Failed { source }`.
9. **Group feed.** `friends::Model` on `siphon/friends-feed` covers the
   primary surface: profile, posts, busy, error, and actions Join, Share,
   Like, Delete, More. The UI also needs an unread count since last view
   (can be derived locally from `created_at`).
10. **Secondary feeds.** Spotify friend activity exists. SoundCloud
    following activity needs a source: who, track, when.
11. **Per-service health.** `Spotify | SoundCloud | YouTube | Group` each
    `Ok | Degraded(String)` so a banner can name the service and offer
    Retry.

12. **Queue for the island.** The upcoming queue (at least the next three
    items with artwork) and actions `Next`, `Previous`, `SkipTo(index)`.
    The island animates from the queue it showed before the change, so the
    UI keeps its own snapshot; the backend only needs to report the new
    queue after each change, in order.
13. **Paste-to-play.** `Resolve(url)` for Spotify, SoundCloud (including
    private `s-` links) and YouTube links, returning `Resolving | Ready(item)
    | Failed(reason)`. Search shows the skeleton while it resolves.
14. **Share from anywhere.** `friends::Action::Share(NewPost)` from the
    island, a row or a playlist, with an optional note (the composer).
15. **Download completion.** `Summary.last_file` plus the folder, so the
    toast's Show opens it, and per-item stems output when stems were asked.

Until an item lands, its UI shows an honest unavailable or loading state in
shipping builds and uses a fixture only under the `demo` feature.

## Milestones

### 1. Design kit (`src/ui/kit/`)

The pieces every redesigned view shares. Nothing on screen changes yet:
the current views do not use the kit until the shell lands.

- `kit::source`: `Source::of(uri)` (Spotify, SoundCloud, YouTube by the
  helper's `youtube:video:` URIs), title colours per theme and over
  artwork, and `title()`, a one-line elided title in its source colour
  that goes through `crate::bidi`. Tests hold every title colour to 4.5:1
  on both built-in themes and keep orange and red at least 25 degrees of
  hue apart.
- `kit::motion`: the one curve (ease-out quint), the durations, staggering,
  `Transition` (repaints only while running, takes no time under reduced
  motion), and `Swipe` placements for the island's Replace, Next and
  Previous.
- `kit::tools`: the four square row tools with `Ready`, `Off(reason)`,
  `Running { fraction }` (a progress ring with a stop square, nothing
  spins) and `Done` states, tooltips on every state, and right-click
  reported separately.
- `theme::Icon` gains `Activity` (BPM/key) and `FileText` (credits), Lucide
  icons added under `assets/icons/`.

Not yet: the built-in palettes still carry the old colours (they switch to
the `DESIGN.md` ramp with the shell, so the app does not change look
halfway), and Geist is not bundled yet (Inter stays until its size is
measured). `kit::Source::of` defers to `crate::soundcloud::Source::of`.

### 2. Palette and source titles (`0d2943a` and before)

The built-in themes use the neutral ramp, the accent is the text colour,
and song titles take their source colour. The SC/YT chips are gone from
rows, search cards, collection headers and the player.

### 3. Player island (`f5341e4`)

The bottom bar is a floating card (`theme::PLAYER_BAR_HEIGHT` is now the
100 px strip that holds it), with every existing control. The next three
queue songs show as small covers that spread on hover and open the queue.
Song changes swipe: Next grows the first queue cover into the cover,
Previous shrinks it back, anything else pushes the old song and queue off.
The island keeps a snapshot of the outgoing song only while that runs.

### 4. Floating sidebar (`cee85fe`)

Docked, the sidebar is a rounded card inside the window. Hidden (the
existing `ToggleSidebar`, Ctrl B), it becomes an edge handle that grows as
the pointer nears, and the sidebar slides out over the page when the
pointer reaches the left edge. No new setting.

### 5. Row tools (`d58bbce`)

Download (helper), BPM/key (off, with its reason, until the helper's
analysis protocol exists), Credits (popover over `RequestCredits` /
`app.credits`, cancelled on close), Add to last-used playlist
(`AddToPlaylist`; the destination is learned from any add this session,
lives in egui memory for the signed-in account only, and resets on restart
or sign-out; storing it needs a backend state field).

### 6. Now playing (`f1d5d02`)

Clicking the island's cover grows Now playing out of the island into the
page area (the top bar stays). The ground is the cover's tint, darkened, in
both themes. Left: the cover, the title in its source colour, artists and
album, and pills for Download, BPM and key (off, with its reason), Credits
and Share with group (`friends::NewPost::from_item`). Right: Up next,
Lyrics (`app.lyrics`, the active line from `Lyrics::active_line`, or a Show
lyrics button over `ToggleLyricsPanel`) and Credits tabs. Clicking the
cover again, or Escape, shrinks it back. It is a view over existing state:
no new action, no setting.

### 7. From the group (`8e628a0`)

Home opens with the group's three newest shares from `app.friends_feed`:
cover, title in its source colour, who and when, their note, a heart
(`Friends(Like)`) and play (`OpenLink`). Not joined, failed (with Try
again over `Friends(Refresh)`), loading and empty each have their own line.
Spotify's own shelves follow unchanged.

### 8. Account scoping and column fit (`9fab9be`)

See the review fixes above. A playlist in a 1280 window keeps its album
column beside the tools, narrowed when needed; below about 870 px of list
width the album gives way to the tools rather than squeezing the title.

### Windows preview of the demo (`76d2805`)

`.github/workflows/preview-windows.yml` builds the offline demo with a
launcher per fixture state, so the redesign can be looked at without an
account. It runs by hand once on main, or from a push whose head commit
says `[preview]`. It complements the coordinator's `windows-candidate`
workflow, which builds the real app.

## Files the frontend expects to touch

New modules under `src/ui/` (tokens, source title, row tools, shell pieces)
and the existing views. `app.rs`, `model.rs`, `backend.rs`, `settings.rs`,
`widgets.rs` and `Cargo.toml` get only small, separate wiring commits,
listed in each milestone report.

## Checks

Each milestone, on Linux in the cloud container with software OpenGL (not
Windows): `cargo clippy --all-targets -- -D warnings` clean, the full
library test suite passing, `cargo fmt --all --check` clean, and native
`--demo-shot` captures of the backend fixtures inspected in dark and light at
1280 and 880 wide. Tests that encoded the old bar position or the SC chip
were updated to the new layout, not removed: drag tests start from the
island's cover, and source tests check the absence of the chip. Not run:
anything on Windows, real accounts, the installed helper.
