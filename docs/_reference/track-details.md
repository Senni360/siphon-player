---
title: Track Credits and Lyrics
description: Source-provided contributors, exact recording matches and cancellable lyrics reads.
---

# Track details

Credits are on-demand data for any track row, independent of playback. Spotify
contributors come from the signed-in playback session and its verified account.
SoundCloud contributors come only from publisher metadata: artist,
writer/composer, publisher, label and phonographic copyright. The uploader is
not inferred to be the performer, writer or producer. YouTube credits are
unavailable, represented by a successful empty result.

## Frontend contract

Send `Action::RequestCredits(uri)`. `App::credits.get(uri)` returns
`Loadable<Option<Credits>>`: loading, a source-provided result, `Loaded(None)`
for unavailable, or a retryable error. Each `Credit` carries a name, optional
role/group and a verified Spotify artist URI when the source supplies one.
`Credits.sources` retains the provider's attribution. No contributor is
invented to fill a missing role.

The runtime uses `Command::Credits { uri, request }` and
`Event::Credits { uri, request, result }`. Request IDs are monotonic, and a
late answer cannot fill a replaced or cleared entry. Repeated requests share
the loading/loaded cache; failed requests can be requested again. Send
`Action::CancelCredits(uri)` when a result is no longer wanted, or before an
explicit refresh of a loaded result. Cancellation removes that entry and
aborts only the matching runtime request. Spotify and SoundCloud account
changes clear all track-detail state and cancel in-flight reads.

Lyrics use `LyricsRequest { request, uri, query, local_file }` and
`Event::Lyrics { request, uri, result }`. A replacement request aborts the
previous read, including API cooldown waits. `Command::CancelLyrics` cancels
the current read. Both URI and request ID must match before displaying a
result. Metadata changes, an available playback session, a restored network
connection, account changes and explicit retry can recheck an unavailable
answer. Credits have a 15-second deadline; lyrics have a 30-second deadline,
including at most 10 seconds for an ISRC mapping.

## Exact external recording matches

The one-way SoundCloud-to-Spotify lookup requires a valid publisher ISRC,
identical normalized ISRC in the Spotify search result, and nonzero full-track
durations within 3 seconds. It searches through the existing catalogue API
gateway and never substitutes audio. Only positive mappings are cached, in
memory for the current account session, keyed by SoundCloud URI, normalized
ISRC and full duration. Account changes replace the cache; private track
identities are not written to a shared disk cache. No reverse lookup runs.
Backend startup removes the obsolete WIP `isrc-matches-v1` cache directory;
other cache directories are retained.

A verified mapping permits the SoundCloud recording to reuse the exact
Spotify track's lyrics. Otherwise SoundCloud and YouTube use LRCLIB only when
the full normalized artist and title match and duration differs by at most
2 seconds. Normalization folds case, accents and punctuation; it does not
remove remix, live, version or featured-artist words. Missing duration or
artist/title means no match. YouTube's channel name remains its artist; a
channel that differs from LRCLIB's performer may therefore have no lyrics.
YouTube never asks Spotify for lyrics or credits.

Existing Spotify and local-file lyrics lookup stays available. Positive
lyrics are cached for 30 days with atomic writes. Missing answers stay in
the current UI request only, and legacy negative disk entries are ignored on
fetch. The external strict cache has a separate namespace from the existing
Spotify/local metadata lookup. SoundCloud requests reuse the client's known
private-link token without putting that token in the track-detail model.

## Verified Spotify operation

On 2026-10-30 the official desktop page at `https://open.spotify.com/` loaded
`https://open.spotifycdn.com/cdn/build/web-player/web-player.f954508a.js`.
Its chunk map identified these official assets:

| Asset | SHA-256 of downloaded bytes |
| --- | --- |
| `xpui-root-dialogs.e544a0c2.js` | `5038cce15df8673e151e32543dd7de96df480d1137991845d3c62106fa26bd61` |
| `xpui-routes-track-v2.755ee45c.js` | `602eab08526ccea6f98bec36924c735b5720432dc6ad70c7e49f12aeee080005` |

Both assets, under that same `/cdn/build/web-player/` URL, contained
`queryTrackCreditsGroupedModal`, persisted-query hash
`f135fb9be58a72d041ab5d214d817021a272405d883860468e2627afb01a3ca9`, and
variables `trackUri`, `contributorsLimit: 100`, `contributorsOffset: 0`.
The official contributor renderer (module 78297 in the main bundle) uses
`name`, `role`, `roleGroup.name`, `uri`/`reference.uri`; the modal reads
`creditsTrait.contributors.items` and `creditsTrait.sources.items[].name`.

The implementation uses the existing Pathfinder transport, the verified
account's session, its normal background request budget and a deadline. It
paginates contributors and rejects GraphQL errors or missing schema instead
of caching them as an empty success. There is no equivalent documented Web
API contributor fallback, so an unavailable session or changed private
operation remains a visible, retryable error.

Public asset verification establishes the operation's provenance, not that
an authenticated response succeeded. Parser, strict-match, cache,
cancellation and account-change tests use deterministic fixtures. A real
Spotify session and a private SoundCloud track still require local account
validation. The future Credits control belongs to the native frontend work.
