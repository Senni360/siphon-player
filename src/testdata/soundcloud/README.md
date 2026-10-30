`track.json`, `search_tracks.json`, `playlist.json`, `user.json`,
`track_likes.json`, and `playlist_likes.json` were
captured anonymously from api-v2.soundcloud.com on 2026-10-28 after discovering
the web client ID from soundcloud.com. Search used `q=lofi`. The playlist
contains 474 entries, of which 470 are id-only stubs. Client IDs and playback
authorization values were replaced with fixture values. No OAuth was used.

`client_asset.json`, `transcodings.json`, and `previews.json` are handwritten
fixtures covering malformed IDs, legacy formats, encrypted formats, and Go+
preview markers without copying a web JavaScript bundle.

`metadata.json`, `repost_ids.json`, and `following.json` are handwritten
fixtures. The repost and stream endpoints were verified in the SoundCloud
web application's `55-4f297086.js` endpoint table on 2026-10-30. No session
was available to capture their authenticated responses. The following feed
fixture distinguishes uploads, reposts, sets, unknown cards and deleted items.
