# Product

<!-- impeccable:product-schema 1 -->

## Platform

desktop

A native Windows app drawn with egui, not a web page. The phone companion at `encore.senni.dev` is the only web part.

## Users

The owner and a small group of friends. Every one of them is both a music producer and a music enthusiast. Everyone is on Windows and has Spotify Premium. They listen across Spotify and SoundCloud every day. They use YouTube mainly to find beats and share them with each other; none of them uses YouTube Music.

The app serves both sides of the same person: the simple listener who just wants to press play, and the producer who wants BPM, key, credits and a download for the DAW. Producer tools are always within reach but never crowd plain listening.

## Product Purpose

Siphon: Encore, shown as Encore throughout the app, is a private desktop music app for that group. One library, one queue and one search hold Spotify songs, SoundCloud uploads (private links included) and YouTube beats. The group's own shared songs come first; Spotify friend activity and SoundCloud's people-you-follow sit underneath as secondary feeds. Every track carries the Siphon helper's download and BPM/key tools.

Success: it replaces the Spotify app and the SoundCloud tab for the group, sharing a song with friends takes one action, and opening the app on a normal day lands on new releases and a mix built from what Spotify and SoundCloud both recommend.

## Positioning

The only player where a Spotify song, a private SoundCloud upload and a YouTube beat sit in the same playlist and queue, shared inside a closed friend group, with download and BPM/key on every track.

## Operating Context

- A Windows desktop app open for long listening sessions, often beside music-making work; downloads and BPM/key lookups go through the installed Siphon helper.
- Songs arrive from friends as links in chat apps. Private SoundCloud links and unlisted YouTube links open in the player instead of the browser.
- Home: new releases and a blended Spotify and SoundCloud mix. Library: a sidebar or an equivalent that is always reachable. Now playing: always present, but not necessarily a classic bottom taskbar.

## Capabilities and Constraints

- Native Windows app in Rust with egui/eframe on OpenGL (glow). The interface is custom-drawn; there is no browser engine. Custom GL shaders are possible through paint callbacks. Low memory and instant start are part of the product.
- Fork of Spotifast (MIT); upstream attribution stays.
- Spotify playback uses librespot and needs Premium. Web API access goes through each listener's own Spotify developer app (Development Mode admits at most five accounts per app), so friends need a short setup tutorial in the same plain style as the extension's install guide and the SoundCloud token guide.
- SoundCloud browsing needs no account; a session-token connection enables likes, reposts and private libraries.
- YouTube plays through the helper's yt-dlp, which also serves search, channels, playlists and mixes. Likes and followed channels stay inside Siphon unless a listener connects their YouTube account through their own Google project, which syncs both ways. There is deliberately no page of recommendations: this is for finding beats.
- Hidden or out of scope: the player-bar visualizer, the Winamp mini player and skins, the language picker, and the Podcasts tab in the library. Podcast results still open from Search; podcasts and audiobooks are not on Home. Out of scope: Canvas and other video, waveforms, timed comments, tags, and comment counts. Windows x64 is the only supported target; other platforms' code is unverified.
- On a phone, `encore.senni.dev` is a public web app. A one-time code from Settings links it, and each phone or browser gets its own code and sign-in, so more than one can be linked. Linked, it is another device of the same account: library, search, artist pages, the group, and playback on the computer or on the phone itself. A browser can't play Spotify itself, so the page plays a Spotify song as the same song found on YouTube or SoundCloud, and it never opens another app to play. It installs to the home screen and keeps its shell for an offline open.
- Built since this list was written: source-coloured titles instead of source chips; like/save on every source; repost for SoundCloud songs only; the group feed on the Vercel friends service, joined with a Spotify account; play counts where a source provides them; release dates (upload date for YouTube); lyrics where available; loudness matched across every source; BPM and key; square per-row buttons for download, BPM/key, credits and add-to-last-playlist; the Siphon helper's downloads, drag tray and BPM/key analysis inside the app; SoundCloud-to-Spotify matching by ISRC.
- Still planned: a native party over the local network (`services/party-room` exists, the player does not use it yet) and a Windows ARM build (releases build x86_64 only).

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
