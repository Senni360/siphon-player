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

Cancel stops the remaining selection and asks the helper to cancel its current
job. Files already saved are kept. Closing the application cancels its jobs.
The extension's own jobs run in a separate helper process and are unaffected.
The player does not keep a download history or resume jobs after restarting.

## Sources and matching

SoundCloud tracks and sets use the catalogue's public permalink. Private or
restricted content can fail: the player does not export browser cookies or send
Spotify/SoundCloud account credentials to the helper.

For a Spotify track, the player searches YouTube for **artists - title** using
the helper's `probe` request, then probes the first five results. It requires a
known duration within three seconds, a matching title and primary artist, and
matching recording qualifiers such as live, acoustic, remix, and remastered.
The best qualifying result supplies the YouTube URL. A missing or uncertain
match produces a toast and skips that track. Matching uses public metadata and
can be conservative; it cannot establish recording identity with certainty.

This action saves an independently hosted YouTube recording. It never reads,
rips, decrypts, or exports Spotify audio, and never changes the player's Spotify
playback source. Use downloads only where you have permission to save the audio.

## Files and defaults

The installed helper owns yt-dlp, FFmpeg, extraction, leading-silence trimming,
metadata, cover art, BPM/key analysis, filename suffixes, and file writes. The
player does not bundle or duplicate that pipeline and adds no settings.

The player requests the helper's factory defaults, which match the extension's
defaults: **MP3 at 320 kbps**, metadata and cover art, leading-silence trimming,
BPM/key analysis and tags, and a filename suffix such as `[120 BPM Am]`. Files go to
the helper's default folder, normally `%USERPROFILE%\Music\Siphon`; SoundCloud
sets use the helper's playlist subfolder. Existing files are preserved.

The native protocol does not expose the extension's customized
`chrome.storage.sync` preferences or saved helper defaults, and does not accept
Spotify metadata overrides. Downloaded files therefore use the source's tags,
even when matched from Spotify. Completion waits for inline BPM/key processing
and its final filename. Analysis warnings do not undo a saved audio file.

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
are at most three active selections, with tracks processed sequentially within
each selection. The player sends `hello`, `probe`, `download`, and `cancel`;
the helper's compatibility mode keeps analysis inline before `done`.

Spotify collection metadata uses the existing same-account session/Web API
router. The helper contacts SoundCloud or YouTube, its media hosts, and the
services already used by the extension, including SponsorBlock for YouTube
music segments. It uses its own network configuration and tools, independently
of the player's proxy settings. No account grants are passed to the helper,
and native messages and helper logs are not copied into the player's logs.

The extension currently installs its helper on Windows only. Linux and macOS
keep their existing player behavior and show downloads as unavailable.
