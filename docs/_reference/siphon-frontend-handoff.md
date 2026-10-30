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

## Status (2026-10-30)

- Phase 1, app direction: **approved.** The owner chose direction **D
  (Hybrid)**: A's top bar and a floating sidebar, B's content, player island
  and now-playing view, C's download options, and the reactive motion
  catalogue. `DESIGN.md` at the repository root is the reference. The
  mockups stay outside the repository as review evidence.
- Phase 2, native implementation: started with shared tokens and reusable
  controls under `src/ui/`. Views are rebuilt in the order of the brief
  (shell, Home, lists, player, Group, downloads, settings).
- `siphon-integration-contract.md` had not arrived on any branch when this
  was written (`main` at `ac4bd07`; `siphon/cleanup`, `credits-isrc`,
  `friends-feed`, `loudness`, `soundcloud`, `youtube` inspected).

## Decisions carried into D

- **Source colour is on the title, never a chip.** Dark theme: Spotify
  `#1ED760`, SoundCloud `#FF8A1F`, YouTube `#FF4848`. Light theme: Spotify
  `#0A8A3F`, SoundCloud `#C25100`, YouTube `#CC1F26`. Orange is pushed toward
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

## Files the frontend expects to touch

New modules under `src/ui/` (tokens, source title, row tools, shell pieces)
and the existing views. `app.rs`, `model.rs`, `backend.rs`, `settings.rs`,
`widgets.rs` and `Cargo.toml` get only small, separate wiring commits,
listed in each milestone report.

## Checks

None run for this milestone: it changes documentation only.
