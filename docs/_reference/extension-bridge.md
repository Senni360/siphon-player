---
title: Extension Bridge
description: The local contract used by the owner's Siphon browser extension.
nav_order: 12
---

The Siphon browser extension can open the player's existing **Add to playlist**
picker. It does not start playback or choose a destination automatically.

```text
siphon.exe --add-to-playlist <url>
```

Pass the URL as one argument, without invoking a shell. Accepted items are
SoundCloud track and set URLs at `https://soundcloud.com/...` (including
desktop/mobile host aliases), and Spotify track, album and playlist URLs at
`https://open.spotify.com/...`, including `intl-xx` paths, or their `spotify:`
URIs. Spotify links become canonical URIs. SoundCloud links are resolved on
the backend through the existing SoundCloud `/resolve` API. Profile, artist,
podcast, search and arbitrary-host links are unsupported. Inputs are limited
to 4096 bytes and cannot contain embedded control characters.

On Windows, each player launch records the full executable path as a `REG_SZ`
value named `Exe` under `HKCU\Software\Siphon Player`. The write runs on a
worker before the window starts and failures are ignored. The extension reads
this value to find the player. Other platforms perform no registry write.
This key is separate from the downloader's own data.

A second launch forwards `add-to-playlist <canonical-item>` over the player's
existing private single-instance channel, receives `ok`, and exits with status
0. An unsupported URL exits nonzero with a short stderr message. A running
player that refuses or fails to acknowledge delivery also produces a nonzero
exit. With no player running, this process starts the normal app and accepts
the request for startup; it remains running as the player. Exit status 0 for
delivery means accepted, not that a playlist has already been changed.

The window comes forward. Spotify requests wait for sign-in when necessary.
The picker loads every page of a Spotify album or playlist before offering
the addition, preserving track order and duplicate occurrences. Unavailable
collection slots and non-track items are skipped. SoundCloud sets reuse the
existing whole-set copy path. Selecting a destination updates local rows
optimistically and shows a toast. SoundCloud tracks added to Spotify playlists
remain local overlays; mixed playlists remain on this computer. Spotify's
existing access and playlist-edit permissions still apply.

Resolution failures show a short toast. Closing the picker or opening a newer
request prevents late replies from replacing the current selection. The bridge
adds no listener, hosted service, credentials, settings or authentication flow.
