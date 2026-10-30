---
title: SoundCloud
description: Anonymous listening, optional account connections, and protected session storage.
nav_order: 11
---

SoundCloud is a separate source alongside Spotify in this personal fork.
Search and public playback do not need a SoundCloud connection. A connection
adds a profile's library. It does not replace Spotify sign-in.

The owner's browser extension can send a track or set to **Add to playlist**
using the [local extension bridge](/extension-bridge/).

## Connect and browse your library

Choose **Connect SoundCloud** in the sidebar or account menu. The default
field accepts a **Profile URL or username**. Choose **Connect public profile**
to verify the profile and see **Connected as [display name]**. Public likes,
playlists, reposts and followings need no sign-in. **Continue without
connecting** leaves anonymous search and playback available.

**Advanced: use a session token** opens a masked field. The dialog explains:
"This token can act as your SoundCloud account. Treat it like a password. It
is saved in your computer's protected credential store." **Connect session**
verifies `/me` before saving the session or showing a connected account. The
pasted token is cleared after every submission, including an invalid one.
Canceling the dialog or disconnecting prevents a late verification from
reconnecting the account.

Once connected, the sidebar's **SoundCloud** section shows **Likes**, uploads,
own and liked or collected playlists, and followed artists. Session connections
include private playlists and uploads that the account can access. Open
**Your uploads** for your own public and private tracks. Likes use the normal song table with
filtering, sorting, paged loading, Play and Shuffle. Playlist and artist rows
open their SoundCloud pages. Playback uses the shared local hybrid queue.
A public profile is read-only: SoundCloud hearts are hidden. A verified
session shows **Like on SoundCloud** or **Remove from SoundCloud Likes**.
Changes appear immediately; a failed write restores the previous heart and
row and shows an error. SoundCloud likes never change Spotify's Liked Songs.

Choose **Disconnect SoundCloud** from the account menu or the SoundCloud
section's context menu. Spotify remains connected independently.

## Search and browsing

Use the same search field for both services. **All**, **Spotify** and
**SoundCloud** chips beside the content filters select a source without changing
the query. Ctrl+Shift+S (Cmd+Shift+S on macOS) cycles the source while Search is
open. Songs, Artists and Playlists apply to SoundCloud; selecting SoundCloud
from an unsupported filter returns to All. Wide windows put the two services
beside each other, and narrow windows stack them.

SoundCloud songs use the ordinary track rows, menus and drag operations. A
small **SC** badge identifies them in search, the queue and the player bar.
It follows the light or dark palette and has a SoundCloud screen-reader label.
Spotify songs do not have a source badge. Playing a SoundCloud search result
uses the loaded SoundCloud songs as its context. Spotify search keeps its
existing single-song context.
When SoundCloud identifies a preview, **Preview only** appears beside the
source badge in rows, the queue and the player bar. Rows retain the full
catalogue duration; the player bar uses the available stream's duration once
playback loads. Preview availability is kept in local playlist snapshots.

Search runs on the backend runtime through the configured shared HTTP
transport. Public search stays anonymous; library and playback reads use the
current session when connected. Playback, search and library reads share discovery and the
persisted public client ID. Typing uses the existing search debounce. Changing the
query or leaving the source cancels pending SoundCloud work, and stale query
or request generations cannot replace the current results. Spotify and
SoundCloud have independent loading and error states. A failed section leaves
the successful sections visible. **Show more** follows each section's opaque
cursor, retaining existing rows and allowing failed pages to be retried.
Retry repeats the failed page. Submit the query again to start a fresh search
if a cursor expires.
Initial failures also offer **Retry** without restarting Spotify search.
Repeated failures from SoundCloud's sections share one short message, and an
empty result appears only after the request finishes. Error messages explain
how to recover instead of exposing HTTP statuses or network details.

Paste a desktop, mobile or short SoundCloud URL to resolve it. The linked
song, playlist or profile appears first. A pasted link reveals SoundCloud
results and resets the entity filter to All. Playlist and artist cards open native
collection pages with ordinary track tables; artist uploads have their own
**Show more** pagination. No embedded browser is used.

### Copy a whole SoundCloud playlist

Open a SoundCloud playlist or album and use **More > Add to playlist**, or
right-click its header. The picker lists editable Spotify
playlists and local mixed playlists. This copies every track in set order,
including duplicate occurrences. Adding the set again adds another copy,
matching single-track local additions. There is no add-undo command; remove
unwanted rows with the ordinary playlist row menu.

SoundCloud rows in a Spotify playlist are local insertions. Spotify and other
devices do not see them. The existing **SC** badge's tooltip explains this.
Local mixed playlists also stay on this computer. Both use hybrid playback,
including shuffle, across the complete combined list.
Create a local mixed playlist from the library's **+** menu. Enter a name to
enable **Save**. Open its **More** menu or right-click its sidebar row to edit
the name and description, duplicate it, or delete it from this computer.

Loaded sets appear immediately. Otherwise the runtime reads the set membership,
inserts placeholder rows and fills every missing track through batched metadata
requests. A failed metadata load removes only that addition's provisional rows
and reports the error. Removing or moving rows while loading is preserved.
Changing connections or quitting cancels pending copies. Only completed copies
are saved, so an interrupted app cannot restore rows stuck waiting for metadata.
Local files keep their existing version and atomic background-write path.
Playlist dragging in the sidebar reorders library entries; it does not copy
playlist contents.

Demo mode includes invented, deterministic SoundCloud results. Use
`--demo-page search --demo-show soundcloud,dark` for SoundCloud alone or
`--demo-show soundcloud-all,light` for both services. Save review captures
outside the repository. Library captures use `--demo-page soundcloud-likes`;
the default demo stays disconnected. Use `--demo-show soundcloud-profile`
to show a connected public library on another page, or
add `--demo-show soundcloud-session,soundcloud-playing,light` for a session
heart beside the source badge, or `soundcloud-connect` / `soundcloud-token`
for the connection dialog. Replace `light` with `dark` for the other palette.

## How requests reach SoundCloud

The client uses SoundCloud's web API at `api-v2.soundcloud.com`. It discovers
the anonymous `client_id` from SoundCloud's public web assets instead of
bundling a permanent key. This is an undocumented web interface, separate
from the official developer API at `api.soundcloud.com`. It can change or
stop working. An anonymous client ID does not grant access to private data.

A session connection adds `Authorization: OAuth <token>` to requests to
the SoundCloud API. The token must never go in a query string, a log, a
redirect request or a request to an audio or artwork host. Profile lookup
uses anonymous requests even when another account is connected.
Session hearts send a single `PUT` or `DELETE` to
`/users/<user-id>/track_likes/<track-id>`. Writes never follow redirects or
retry automatically, and a rate-limit response holds later writes until its
cooldown ends.

The web-client discovery and cookie-header approach also appear in
[yt-dlp's SoundCloud integration](https://github.com/yt-dlp/yt-dlp/blob/master/yt_dlp/extractor/soundcloud.py).
That is implementation evidence, not a supported third-party login contract.
No browser engine is embedded and no SoundCloud password is collected.

## Optional connections

| Mode | What it can read | What it can change |
| --- | --- | --- |
| Disconnected | Public search and playable public tracks | Nothing on an account |
| Profile | That profile's public likes, playlists, reposts and followings | Nothing on an account |
| Session | The signed-in user's public library and private playlists the session can access | Like and unlike tracks |

### Connect by profile

Paste a SoundCloud profile URL or its username. Mobile and desktop profile
URLs work. A short `on.soundcloud.com` link needs to be resolved first and
must lead to a SoundCloud profile. A track or playlist link is not a profile.

This connection follows public information. It does not prove ownership of
the profile, sign in to SoundCloud, or allow likes to be changed. Private
playlists stay private. SoundCloud's
[profile privacy guide](https://help.soundcloud.com/hc/en-us/articles/115003451767)
confirms that likes, reposts and followings are public, while uploads and
playlists can be private.

### Advanced: use a session token

In your usual browser, sign in at `https://soundcloud.com`. Open Developer
Tools, then Application (Chromium browsers) or Storage (Firefox), Cookies,
and `https://soundcloud.com`. Find `oauth_token` and copy only its value.
Paste that value into the masked session-token field. An `OAuth ` prefix
is accepted too. Do not paste your password or the entire cookie list.

This token can act as your SoundCloud account. Treat it like a password and
use this option only on a computer you trust. The app checks `/me` before
saving it and uses that response as the account identity. A pasted profile
name never determines the owner of a session.

There is no refresh grant in a pasted cookie. If SoundCloud expires or
revokes the session, Siphon shows **Reconnect SoundCloud**, forgets private
library views and continues reading public content anonymously. Choose that
prompt and paste a current cookie value. A forbidden track alone does not
disconnect a valid session: the client checks `/me` first. No fixed session
lifetime or access to every private item is promised.
Pending playback reads restart without account access, so a late private
response cannot restore expired access to the queue or stream cache.

Session library reads combine authorized `/users/<id>/playlists`,
`/users/<id>/playlists/liked_and_owned` and `/users/<id>/playlist_likes`,
following each feed's cursor and removing duplicate sets. Likes and uploads
use authorized `/users/<id>/track_likes` and `/users/<id>/tracks`.
Private `secret_token` values returned with catalogue items stay in memory
for playlist hydration, fresh track lookup and stream resolution. They are
scoped to the connection and never saved in library or session snapshots.
When you import a private set link, each saved row keeps that original link
with its set identity in the album metadata. After restart, playback can resolve
the link again and verify the selected song is still in that set. The track's
own permalink stays unchanged. Revoked links and removed members fail normally;
this does not create per-track secret links or save returned access tokens.

Disconnecting forgets the local account and token. It does not call remote
session sign-out, which could also sign out the browser using the same
session. SoundCloud documents this distinction in its
[disconnect and sign-out guidance](https://developers.soundcloud.com/docs/api/guide#disconnect-and-sign-out).

## Why there is no ordinary browser OAuth connection yet

As checked on 28 October 2026, SoundCloud documents OAuth 2.1 with PKCE, but
ordinary registered applications still need a client secret for token
exchange and refresh. PKCE does not remove that requirement. See the
[authentication guide](https://developers.soundcloud.com/docs/api/guide#authentication).

New app registration is available to Artist Pro subscribers through Your
Apps or SoundCloud's CLI. A user could register their own app and supply
both credentials and a registered redirect URI, but that adds a subscription
and setup work. See [Get an API key](https://developers.soundcloud.com/docs/api/register-app).

SoundCloud's own [credentials CLI](https://github.com/soundcloud/api/blob/master/scripts/sc-api-auth.mjs)
demonstrates a system-browser, loopback PKCE exchange without a secret. Its
bundled public client is explicitly scoped to that tool. It is not evidence
that a newly registered desktop app can use the same flow, and the app does
not reuse that client ID. A future OAuth option needs a supported public
client registration from SoundCloud. No shared secret belongs in a binary.

## Files and secrets

`soundcloud-account.json` in the application state directory holds the
connection mode, numeric user ID, display name, permalink and optional avatar
URL. It contains no token. The file is replaced atomically through the
app's background file writer. A missing file means disconnected.

The last loaded likes, playlist list and followed artists are cached under
`soundcloud-library/<user-id>-<mode>.json` in the application cache directory.
Snapshots are metadata, contain no session token, and are atomically written
by the background file writer. Public-profile and session snapshots are kept
separate, so session-only playlist metadata cannot appear in profile mode.
The saved library appears as soon as its account is restored, while fresh
requests run in the background. Failed refreshes keep the last good rows.
Disconnecting hides the library; these disposable cache files may remain
until the application cache is cleared.

Session tokens use the existing credential layer under
`com.siphon.player`, with a separate `soundcloud-session` entry for
each application state location. Linux uses Secret Service, macOS uses
Keychain, and Windows uses Credential Manager with the existing DPAPI-sealed
copy. See [Settings & Files]({{ '/settings-and-files/' | relative_url }})
for the platform directories and protection limits.

The credential worker verifies each save by reading it back. A locked or
unavailable store produces an error, never a new plaintext fallback file.
Disconnecting invalidates pending connection work and records a non-secret
revocation marker before deletion. Keep that marker if deletion failed:
it prevents an old token from restoring the connection at the next start.
Switching to profile mode also attempts to clear any previous session token.
A cleanup failure is reported, but the public profile remains usable without
an unlocked credential store. Its saved mode never restores a token. Signing
out of Spotify does not sign out of SoundCloud.

## Playback limits

The client does not request ads or make the web player's ad requests. It
requests the audio streams that SoundCloud makes available. It does not
replace Spotify tracks with SoundCloud recordings.

Go+ tracks served only as previews play as previews. A session token does
not promise full Go+ playback. Blocked tracks stay unavailable and encrypted
streams are not played. There is no paywall or DRM bypass. SoundCloud's
[streaming guide](https://developers.soundcloud.com/docs/api/guide#streaming-tracks)
describes playable, preview and blocked access.

## Source-specific actions

SoundCloud rows offer **Open on SoundCloud** and **Copy SoundCloud link** when
SoundCloud returned a public permalink. Artist links retain the full
`soundcloud:user:<id>` URI. A missing permalink is shown as unavailable;
Siphon never builds a Spotify link from a SoundCloud ID.

SoundCloud menus omit Spotify Liked Songs, radio, album navigation and credits.
Lyrics lookup is also unavailable for SoundCloud tracks. Disabled controls
explain why, and keyboard save commands cannot change Spotify's library for
those tracks. In a mixed selection, saving affects only eligible Spotify rows
in their original order. The gateway and Web API boundary reject foreign
URIs before authentication or network work, including queue and playlist
writes. Local mixed playlists and Spotify overlays retain their own
add-to-playlist routing.

SoundCloud playback stays on this computer. Spotify Connect, Sonos and Cast
selections are disabled with an explanation while a SoundCloud track is
playing or queued, or a local mixed context is active (including a Spotify
playlist with local additions).
The computer's own audio-output picker remains usable.
Artwork uses the existing URL-based image cache, including sndcdn.com URLs.

SoundCloud users and playlists use the canonical `Page::SoundCloud(full_uri)`;
Likes use `Page::SoundCloudLikes`. These keys round-trip through navigation
history without entering Spotify page loaders. Track metadata is cached by
full SoundCloud URI so numeric IDs cannot collide with Spotify entries.
