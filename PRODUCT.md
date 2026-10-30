# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

The owner and a small group of friends. Every one of them is both a music producer and a music enthusiast. Everyone is on Windows and has Spotify Premium. They listen across Spotify and SoundCloud every day. They use YouTube mainly to find beats and share them with each other; none of them uses YouTube Music.

The app serves both sides of the same person: the simple listener who just wants to press play, and the producer who wants BPM, key, credits and a download for the DAW. Producer tools are always within reach but never crowd plain listening.

## Product Purpose

Siphon Player is a private desktop music app for that group. One library, one queue and one search hold Spotify songs, SoundCloud uploads (private links included) and YouTube beats. The group's own shared songs come first; Spotify friend activity and SoundCloud's people-you-follow sit underneath as secondary feeds. Every track carries the Siphon helper's download and BPM/key tools.

Success: it replaces the Spotify app and the SoundCloud tab for the group, sharing a song with friends takes one action, and opening the app on a normal day lands on new releases and a mix built from what Spotify and SoundCloud both recommend.

## Positioning

The only player where a Spotify song, a private SoundCloud upload and a YouTube beat sit in the same playlist and queue, shared inside a closed friend group, with download and BPM/key on every track.

## Operating Context

- A Windows desktop app open for long listening sessions, often beside music-making work; downloads and BPM/key lookups go through the installed Siphon helper.
- Songs arrive from friends as links in chat apps. Private SoundCloud links and unlisted YouTube links should open in the player instead of the browser (planned).
- Home: new releases and a blended Spotify and SoundCloud mix. Library: a sidebar or an equivalent that is always reachable. Now playing: always present, but not necessarily a classic bottom taskbar.

## Capabilities and Constraints

- Native Windows app in Rust with egui/eframe on OpenGL (glow). The interface is custom-drawn; there is no browser engine. Custom GL shaders are possible through paint callbacks. Low memory and instant start are part of the product.
- Fork of Spotifast (MIT); upstream attribution stays.
- Spotify playback uses librespot and needs Premium. Web API access goes through each listener's own Spotify developer app (Development Mode admits at most five accounts per app), so friends need a short setup tutorial in the same plain style as the extension's install guide and the SoundCloud token guide.
- SoundCloud browsing needs no account; a session-token connection enables likes, reposts and private libraries.
- YouTube playback is planned through the helper's yt-dlp. YouTube likes stay inside Siphon (no Google developer app). YouTube search is later work.
- Removed from scope: visualizers (MilkDrop, player-bar bars), Winamp mode and skins, podcasts and audiobooks, macOS and Linux code, translations (English only), Canvas and other video, waveforms, timed comments, tags, repost and comment counts.
- Planned: source-coloured titles instead of source chips; like/save for every source; repost for SoundCloud songs only; a Siphon friends feed (Vercel backend, invite code, generated id plus a chosen name); play counts in playlists, albums and artist pages where a source provides them; release dates (upload date for YouTube); lyrics where available; loudness matched across every source; BPM and key; square per-row buttons for download, BPM/key, credits and add-to-last-playlist; the Siphon extension's helper features inside the app; SoundCloud-to-Spotify matching by ISRC.

## Brand Commitments

- The name Siphon and the nested-U logo, shared with the Siphon browser extension.
- Source colours identify the source: Spotify green, SoundCloud orange, YouTube red. They must read as clearly green, orange and red and may be nudged apart for distinction.
- Owner's binding references: T3 Code, shadcn/ui, coss.com/ui, and Aceternity's liquid glass, gooey input and noise background.
- Minimal and low-colour. Motion and liquid glass are welcome; many colours together reads as childish rather than premium.

## Evidence on Hand

- The existing app, with a demo mode that fills every page with sample data and writes screenshots (`--demo`, `--demo-shot`, `--demo-size`; requires the `demo` feature).
- The sister product's design language: `C:\Users\senni\siphon\DESIGN.md` (Siphon browser extension).
- No testimonials, usage figures or public claims exist; none may be invented.

## Product Principles

1. Listening first: a feature that does not serve listening, sharing or making music does not ship.
2. One library, three sources: a song's source changes its colour, never its layout or controls.
3. Uniform or not at all: a piece of data appears only where every relevant source can provide it.
4. Friends first: the group's own shares rank above any platform's social feed.
5. Light by design: no browser engine, no video, and effects cheap enough to leave on.

## Accessibility & Inclusion

A private group app; the owner waived colour-vision accommodation for source colours. Text and controls must still be readable at normal desktop distance.
