---
title: Downloads
description: Save SoundCloud audio or matched YouTube recordings using the installed Siphon extension helper.
nav_order: 16
---

## Install and use

On Windows, install the owner's **Siphon browser extension helper** with its
installer first. The player finds `com.siphon.host` through the same current-user
native messaging registration as Helium. No browser window or running extension
is needed. If the helper is missing, Download is disabled with
**Install the Siphon extension helper to download**. After installing it, switch
back to the player to refresh detection.

Right-click a track or the playing song's artwork/title and choose **Download**.
Select several rows to download that selection. An album, Spotify playlist, or
SoundCloud set's menu offers **Download all**. Spotify collections load all their
pages before downloading; repeated tracks are saved once. Episodes and local
files are excluded. A small card above the player bar shows the current track,
stage, percentage when available, and **Cancel**. Completion and errors appear
as toasts. Playback continues throughout.

Download all reads the original Spotify collection. For locally mixed playlists
or locally inserted SoundCloud rows, select the rows and choose **Download**.

Cancel stops the remaining selection and asks the helper to cancel the jobs it
still has. Files already saved are kept. Up to four songs of a selection download
at once; a failed song is counted and the rest continue. Closing the application
cancels its jobs. The extension's own jobs run in a separate helper process and
are unaffected. The player does not keep a download history or resume jobs after
restarting.

## Sources and matching

SoundCloud tracks and sets use the catalogue's public permalink. Private or
restricted content can fail: the player does not export browser cookies or send
Spotify/SoundCloud account credentials to the helper.

For a Spotify track, the player searches YouTube and SoundCloud once for
**artists - title**, using the public listing's own titles, uploaders and lengths.
It requires a known duration within three seconds, a matching title and primary
artist, and matching recording qualifiers such as live, acoustic, remix, and
remastered. The best qualifying listing entry supplies a YouTube link or a public
SoundCloud permalink; no candidate is fetched separately. A missing or uncertain
match produces a toast and skips that track. Matching uses public metadata and
can be conservative; it cannot establish recording identity with certainty.

A found match is kept in `spotify-matches.json` with its date, so a later
download or phone play needs no search. If that source stops working, Encore
searches once more and replaces it. Removing the file only costs speed.

This action saves an independently hosted YouTube recording. It never reads,
rips, decrypts, or exports Spotify audio, and never changes the player's Spotify
playback source. Use downloads only where you have permission to save the audio.

## Files and defaults

The installed helper owns yt-dlp, FFmpeg, extraction, leading-silence trimming,
metadata, cover art, BPM/key analysis, filename suffixes, and file writes. The
player does not bundle or duplicate that pipeline.

Settings, under **Downloads & files**, offers a folder, a format (MP3, M4A, Opus,
FLAC or WAV) and a quality for lossy formats (320, 256, 192 or 128 kbps), plus
whether a finished file is offered to the helper's drag tray
(**Show downloads in the drag tray**). A choice left alone sends nothing, so the
helper's factory defaults apply: **MP3 at 320 kbps**, metadata and
cover art, leading-silence trimming, BPM/key analysis and tags, and a filename
suffix such as `[120 BPM Am]`. Files go to the helper's default folder, normally
`%USERPROFILE%\Music\Siphon`, unless a folder is chosen; SoundCloud sets use the
helper's playlist subfolder. Existing files are preserved.

The native protocol does not expose the extension's customized
`chrome.storage.sync` preferences or saved helper defaults, and does not accept
Spotify metadata overrides. Downloaded files therefore use the source's tags,
even when matched from Spotify. Completion waits for inline BPM/key processing
and its final filename, so the path shown, revealed and offered to a DAW is the
one on disk. Analysis warnings do not undo a saved audio file.

When the helper reports a drag tray, a finished file appears there with its title
so it can be dropped into a DAW. Turning the setting off, or an older helper
without a tray, sends nothing and shows no error.

## Transport and network access

On Windows, discovery reads the default `REG_SZ` value under
`HKCU\Software\Chromium\NativeMessagingHosts\com.siphon.host`, then the
equivalent Google Chrome, Microsoft Edge, and Brave keys if needed. It validates
the named `stdio` manifest and runs its registered executable or `.cmd`/`.bat`
launcher. The installer's launcher supplies FFmpeg and Node's PATH. The player
does not search arbitrary executable names on PATH for a helper.

Requests and replies use four-byte little-endian lengths followed by UTF-8 JSON
on piped stdin/stdout. Response frames are bounded to one MiB. Helper discovery,
process I/O, collection reads, and matching run on the backend runtime. There
are at most three active selections; each runs up to four songs at once through
one helper connection, which `hello` asks to run that many jobs. The player sends
`hello`, `download`, `cancel`, and — only when a tray is reported and enabled —
`showDrop`; the helper's compatibility mode keeps analysis inline before `done`.
Spotify matching reads YouTube's and SoundCloud's public listings directly, not
through the helper.

Spotify collection metadata uses the existing same-account session/Web API
router. The helper contacts SoundCloud or YouTube, its media hosts, and the
services already used by the extension, including SponsorBlock for YouTube
music segments. It uses its own network configuration and tools, independently
of the player's proxy settings. No account grants are passed to the helper,
and native messages and helper logs are not copied into the player's logs.

The extension currently installs its helper on Windows only. Linux and macOS
keep their existing player behavior and show downloads as unavailable.
