---
title: Siphon Backend Integration Contract
description: Native model, action, event, fixture and ownership contracts for the Siphon frontend.
---

# Backend delivery contract

This contract belongs to the backend coordinator. The delivery branch is
`siphon/backend-delivery`, based on `ac4bd07` and the five preserved feature
snapshots. The local Windows coordinator owns the final integration and release.
Claude owns layout, styling, components, the three mockups and the approved
native frontend. This branch does not select a design or deploy the service.

The Rust source is authoritative for exact signatures. UI code draws borrowed
state and emits `model::Action`; `App` applies actions after drawing, and
`backend::Worker` performs network work on its runtime. Audio processing stays
in the player/external PCM modules. Encode state on the UI thread only when
cheap; hand writes to the existing atomic writer. Never start a helper, fetch
metadata or wait for disk while drawing a row.

## Track identity and metadata

All three sources use `api::models::Track` and `PlayableItem::Track`.

| Source | Canonical identity | Artist | Count/date |
| --- | --- | --- | --- |
| Spotify | `spotify:track:<id>` | supplied artist references | existing `app.play_counts` page map; `Track.album.release_date` |
| SoundCloud | `soundcloud:track:<numeric-id>` | uploader/artist references | playback count; `release_date`, then `display_date`, then `created_at` |
| YouTube | `youtube:video:<11-character-id>` | channel, then uploader | view count; upload date |

Keep the complete `Track` when passing a row to an action. The shared optional
fields are `release_date: Option<String>`, `play_count: Option<u64>`,
`publisher_metadata: Option<Box<PublisherMetadata>>` and `label_name: Option<String>`.
The new shared date/count fields currently populate for SoundCloud and YouTube;
Spotify retains the page/album locations above. They have serde defaults for
existing saved queues and playlists. Missing values
mean unknown, never zero. SoundCloud publisher metadata includes
`isrc: Option<String>` and also fills `external_ids.isrc`. Credits must come from
real metadata, never from an uploader name inferred to be a producer.

Artwork lives in `Track.album.images`. SoundCloud supplies its high resolution
artwork URL; YouTube retains image dimensions so the renderer can crop it.
`external_urls.spotify` is a legacy field name that carries the source's share
URL. Do not infer a Spotify source from that field name. A private SoundCloud
URL must preserve its secret path/query through the intended group share and
link-opening flow. Never log a track/share payload or put secret links into a
public diagnostic report. Signed playback URLs remain internal to the engine.

`is_playable` preserves available/unavailable/unknown, while `preview_only`
identifies SoundCloud previews. A row may be available for browsing while a
required local account/helper/output device is unavailable. Distinguish these
states in presentation. There is no YouTube-to-Spotify playback substitution.

## Actions and read states

Use `Loadable<T>` consistently: `NotLoaded`, `Loading`, `Loaded(T)`, and
`Failed(String)`. For optional results, `Loaded(None)` is a successful unavailable
answer. It is different from a failed request. Errors expose retry; an explicit
`RetryLyrics` can also recheck a previous unavailable result, and cancelling a
credits entry permits a fresh request.

| Operation | Native action/state | Behavior |
| --- | --- | --- |
| Open a link | `Action::OpenLink` | Use the common parser and dispatcher for track links. `OpenUri` navigates pages; direct tracks use `OpenLink` or `PlayUris`. |
| Like/save | `Action::ToggleSaved(uri)` | Routes to Spotify saves, session-authorized SoundCloud likes, or local YouTube likes. |
| SoundCloud like | `ToggleSoundCloudLike(uri)`; `App::can_like_soundcloud()` | Profile-only connections cannot write. |
| SoundCloud repost | `ToggleSoundCloudRepost(uri)`; `can_repost_soundcloud(uri)`, `soundcloud_reposted(uri)` | Tracks and sets only, session required; pending and acknowledged local writes overlay stale snapshots. |
| Following feed | `LoadSoundCloudFollowing { more }` | `soundcloud_library.following.status` and `snapshot.following`; secondary to the Siphon group feed. |
| Credits | `RequestCredits(uri)` / `CancelCredits(uri)`; `app.credits.get(uri)` | Per-track, independent of now playing, deduplicated while loading/loaded. Cancel the request when its consumer goes away. |
| Lyrics | `ToggleLyricsPanel`, `RetryLyrics`; `app.lyrics_uri`, `app.lyrics` | Results are bound to the current request as well as URI; stale requests cannot fill a newer view. |
| Add to playlist | `AddToPlaylist { playlist_id, playlist_name, items }` | A future add-to-last button uses this existing action after selecting its destination. No new persistent last-playlist preference is introduced here. |
| Download | `Download(items)`, `DownloadAll`, `CancelDownload(id)` | Existing `downloads::State`, `Progress` and `Event` carry queued/running/cancelling/finished/error state. Helper installation controls availability. |

Analysis, BPM/key and stem tools are owned by the separate Windows helper
integration branch, which was not supplied in this environment. Their future
action/event signatures are **not implemented by this delivery**. Do not bind a
button to a made-up successful response. Existing download availability is not
proof that a newer analysis protocol is installed. Spotify download matching
and independent-source analysis must retain their provenance; they do not
export or measure Spotify audio.

## Credits, exact matching and lyrics

Spotify credits use the existing same-account playback session/private-read
conventions. The operation/hash and official asset verification are documented
in [track details](track-details.md). ISRC matches stay in session-owned memory.
An authenticated read still requires local validation.
SoundCloud credits use publisher metadata. YouTube contributor credits return
unavailable rather than fabricated artist roles.

SoundCloud-to-Spotify mapping requires a normalized exact ISRC and duration
within 3 seconds. Only a verified mapping can reuse Spotify lyrics. The public
LRCLIB fallback for external sources requires normalized exact artist/title
and duration within 2 seconds. Unmatched results remain unavailable. No reverse
Spotify-to-SoundCloud lookup is introduced. Account/session/data changes reset
negative results and cancel obsolete jobs.

## Friends group

`Action::Friends(friends::Action)` is the UI entry point. Use
`app.friends_feed: friends::Model` for `profile`, `posts`, `next_cursor`, `busy`,
`error`, `restored` and the one-time `recovery_secret`. Service identity is
independent of Spotify/SoundCloud identity. Requests/events carry a generation;
prior-account results are ignored. Model actions own optimistic changes and
backoff; a widget must not add a second polling loop.

The action family includes Open, Join, Rename, Refresh, More, Share, Like,
Delete, DismissSecret, `LeaveGroup` (remote membership revocation) and `Leave`
(local forget, available offline). Remote leave retains shared posts as Former
member and removes that member's hearts. Requests use a fresh serial within the
account generation, so stale answers cannot replace newer actions. See
[the service README](../../services/friends/README.md) for exact leave semantics,
routes, deployment environment names and storage behavior. Deleting another
member's post is forbidden server-side. A private link is visible to the closed
group that receives it, so preserve the link but redact diagnostic output.

Poll only while the friends panel/window is visible, online and not already
busy. The model backs off after errors and stops polling during optimistic work.
`SIPHON_FRIENDS_URL` supplies the development/service URL, including `/api`.
There is no configured production endpoint in this repository. No deployment or
paid resources were created. Production requires Redis and a configured invite;
in-memory storage is only for development/tests.

## Playback, queue and loudness

Use the existing playback actions (`PlayFromRow`, `PlayContext`, `PlayUris`,
`TogglePlay`, `Next`, `Previous`, `Seek`) and manual-queue actions (`AddToQueue`,
`QueueMany`, `PlayNext`, `MoveInQueue`, `InsertInQueue`, `RemoveFromQueue`).
[Queue rules](queue.md) remain the behavioral contract. Track URI identifies
content; row/occurrence identity preserves duplicates and controls which copy
leaves the queue. Mixed contexts restore paused and stay on this computer.

SoundCloud and YouTube reuse the external progressive AAC/PCM engine and the
same output, EQ and limiter. URI/tag/generation checks reject old stream
resolutions and expired-URL refreshes after a skip. The YouTube catalogue caches
metadata instead of spawning a process per rendered row. The debug-only `SIPHON_YTDLP` override supports fixture tools; release builds
ignore it. The Windows helper's exact discovery/update-lock behavior remains a
local validation requirement.

Spotify uses metadata track gain at the explicitly configured zero pregain and
Dynamic normalization method. External audio uses R128 measurement referenced
to Spotify's published -14 LUFS convention, bounded lookahead and smooth gain
changes. These are different measurement mechanisms, not a promise of identical
perceptual loudness. Normalization off bypasses source gain while preserving EQ
and limiting. See [loudness](loudness.md) for the first-play limits and audible QA.

## Account and cancellation rules

Keep account generation, request serial and playback occurrence separate.
An answer is accepted only by the current owner/request. Clearing a panel does
not reset IDs so an old answer can reuse them. Stop pending helper/network work
when its consumer is replaced. A sign-out/account switch clears private in-memory
state before the next account uses it. Loudness writes reject invalidated
generations. SoundCloud library and YouTube likes writes may finish in their
original account-keyed files; new-account state never reads those files. Durable
local likes and user-saved mixed playlists are retained for their original owner.
Do not confuse retained user library data with a cache shared between accounts.

SoundCloud social state is scoped to user and connection type. A public-profile
connection never restores a session's private library. Credentials remain in the
protected store; invites, bearer secrets, tokens, private permalinks and signed
stream URLs must not appear in logs. Ordinary settings must not acquire a new
secret field.

## Deterministic frontend fixtures

`demo::siphon::fixture(Scenario)` returns the real shared model types with
explicitly invented data and no I/O. It covers Ready, Empty, Loading, Error,
Unavailable and LongTitles, with all three sources and unavailable YouTube
credits. Identities, dates, row IDs and text are fixed. Fixture counts and
contributor names are examples, never product or API claims.

With the `demo` feature, `--demo --demo-show siphon-ready` installs those models
into existing native surfaces and opens a three-source mixed playlist. Replace
the flag with `siphon-empty`, `siphon-loading`, `siphon-error`,
`siphon-unavailable` or `siphon-long-titles`. Read states also populate the credits,
lyrics, friends and following models for Claude's replacement views. Fixtures
do not imply a new credits/dialog layout is already implemented. Keep demo mode
and its isolated profile for visual work; never test with the owner's live data.
The demo backend stays offline, so network mutation controls remain disabled
even when the fixtures represent a connected account.

Normal/narrow widths, light/dark, font/DPI, OpenGL rendering and interaction need
native verification after Claude integrates the approved design. Browser
captures are not native evidence. Backend fixture tests are not audible-output,
authenticated-account, installer or Windows process tests.
