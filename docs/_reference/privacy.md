---
title: Privacy
description: What Siphon stores on your computer, what it sends and to whom, and what it never collects.
nav_order: 4
---

Siphon is a desktop app that runs on your computer. It has no account of its
own, no telemetry, no analytics and no advertising. It talks to the music
services you use, and, only when you join a group or link a phone, to the
owner's two small services described below. Its author receives nothing else
about you or how you use it.

This page covers the Siphon app, version 0.8.0 and later. Earlier versions
kept sign-ins in files instead of the system credential store; update to a
current release.

## What stays on your computer



[Settings & Files](/settings-and-files/) lists every location and what is safe
to delete. **Sign out** in Settings removes the stored credentials and the
cached library metadata: playlists, Home and Liked Songs.

## What is sent, and to whom

Siphon connects only to the services below.

- **Spotify:** `accounts.spotify.com` and `api.spotify.com` for sign-in and your
  library, with the app you created; Spotify's playback and metadata endpoints
  through librespot on this computer.
- **SoundCloud:** its public API and site for browsing and playing, with no
  account unless you connect one.
- **YouTube:** through the installed Siphon helper's `yt-dlp`, which reads
  public and unlisted videos. A connected YouTube account also uses Google's
  API with your own project.
- **LRCLIB** (`lrclib.net`) for lyrics of SoundCloud and YouTube songs, sending
  only the artist, title and length.
- **GitHub** for update checks and downloads.
- **The group service** (`siphon-friends.vercel.app`) when you join a group: the
  shares, replies, hearts and listening state you choose to send.
- **The phone edge** (`edge.encore.senni.dev`) only when you link a phone: the
  account id, the phone's commands, what it plays, and the audio it fetches.
- **Spotify Soloist's server** (`soloist-builds.spotifycdn.com`) only if you
  enable Soloist, to download its player.

Links you open from the app, such as the Winamp Skin Museum or this website,
open in your browser.

## This website

The upstream documentation site, spotifast.rocks, counts page visits with [Plausible](https://plausible.io/data-policy),
which uses no cookies and collects no personal data. The app itself contains
no analytics.

## Questions

Ask on [GitHub](https://github.com/crmne/spotifast/issues).

## Optional Soloist

Soloist is separate Spotify software with its own network traffic and privacy
policy. It is downloaded only when enabled with a personal key. The key is
stored in the protected system credential store. Spotify currently requires
it as a process argument; Siphon does not log the command or child output.
Its control socket is loopback-only. See [Soloist](soloist.md).
