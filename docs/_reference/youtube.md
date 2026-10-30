---
title: YouTube audio
---

Siphon opens public and unlisted YouTube videos as audio in its local mixed
queue. Paste a watch, youtu.be, mobile/music watch, or Shorts link into Search,
or open it through the existing command-line/single-instance link command.
The internal identity is `youtube:video:<11-character-id>`; URL tracking and
playlist parameters do not become queue identities. Spotify tracks continue
to play from Spotify, never from a YouTube substitute.

The installed Windows Siphon helper owns yt-dlp. The player uses its existing
`%LOCALAPPDATA%\Siphon\bin\yt-dlp\yt-dlp.exe` without installing or updating
it. This narrow adapter expects the helper's shared running lock at
`%LOCALAPPDATA%\Siphon\bin\yt-dlp.running.lock`; the installer path and updater
locking still need verification against the owner's actual helper. Debug
builds may select an absolute fake/development executable with `SIPHON_YTDLP`.
Release builds ignore that override. Linux tests use fake tools; Linux has no
shipping helper discovery.

The command starts directly without a shell, ignores yt-dlp configuration,
accepts only a validated video ID, and requests format 140 (progressive AAC in
MP4). It never selects a video format or requests Google cookies. It inherits
the process environment for yt-dlp proxy handling; the app's custom proxy
preferences are not yet passed to this separate executable. Signed HTTPS
URLs must belong to `*.googlevideo.com`, contain no embedded credentials, and
remain in memory. The shared external PCM engine owns bounded range reads,
seek, URL refresh after expiry/refusal, cancellation, EQ/limiter and output
handoff. The shared loudness work applies at that same PCM seam.

At most two yt-dlp processes run concurrently. A 30-second deadline covers
process-slot waiting, update-lock waiting, execution and draining pipes.
Metadata output is limited to 4 MiB and diagnostics to 64 KiB. Timeout,
oversized output or cancelled work kills and reaps the directly launched
process before releasing its running lock. Windows launches hide the console.
Windows process-tree behavior when yt-dlp uses an external JavaScript runtime
still requires the local helper/system integration checks. Diagnostics become
fixed user-facing categories; neither helper stderr nor signed/unlisted URLs
are written to the player log.

Missing helper, unavailable/deleted/live/upcoming content, sign-in-required
content and unsupported AAC are explicit errors. No Google authentication or
YouTube search is implemented. Next-track preparation uses the same queue
preload path; completed loads are accepted only for the current playback tag.
Replacing or stopping playback cancels older resolver work. URL expiry is
checked before reusing a prepared stream; metadata is reused from a bounded,
private, account-scoped cache so browsing saved rows does not spawn processes.
Upload dates, views and thumbnails remain optional, and the channel supplies
the artist name. Thumbnail URLs are limited to YouTube's image hosts.

**YouTube Likes** uses the existing collection rows and sidebar navigation.
The row/player heart writes only Siphon's local library, with no Google API.
Likes live in private, atomically replaced `state/youtube-likes/<account-hash>.json`
files. They load on a blocking backend worker after the Spotify account is known
and after pending writes finish, so a quick account switch cannot restore an
older snapshot over recent likes. The app and restore worker share the existing
file writer and its suspend/exit flush. A flush timeout gives a retryable error.
Switching accounts clears visible state and pending metadata; generation/account
and save-revision checks reject old completions.

Failed saves keep current edits visible and report the failure. Opening YouTube
Likes again retries that snapshot. If the listener switched accounts before a
failed save completed, returning to the original account shows its last durable
likes with a warning. Unsaved track snapshots are not kept across sign-out.
On restart, only the last successfully replaced file is restored. Existing
unreadable, partial, invalid-track or future-version files are kept and cannot
be overwritten through a disabled like action. Reads and encoded saves share a
16 MiB limit. The earlier WIP
`Settings.youtube_likes` field was removed before release. No published-state
migration from that unshipped field is claimed.

Mixed playlists and Spotify playlist overlays retain YouTube tracks locally;
YouTube URIs are never sent in a Spotify playlist write. The common queue
preserves duplicate occurrences, transport intent and paused restoration.
Remote Spotify receivers cannot take external audio. The Download action
passes the selected YouTube URL directly to the existing helper protocol,
without Spotify matching or changing playback provenance.

Validation is account-free: deterministic video metadata, malformed links,
source provenance, cache/likes persistence, process argument/timeout/cancel/
lock/output-limit fixtures, delayed/failed atomic writes, account round trips,
duplicate load completions, and mixed queue/stale-account regressions. Existing
external-engine range/seek/URL-refresh tests cover the shared transport.
These fixtures do not prove live YouTube extraction, Windows helper updates,
real output-device playback, or audible cross-source transitions. Those are
required local checks before release.
