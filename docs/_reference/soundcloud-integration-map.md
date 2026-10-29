---
title: SoundCloud Integration Map
description: Working integration notes for the personal fork's mixed-source playback.
nav_exclude: true
---

# SoundCloud integration map

Research baseline: `4d60f63c79ac3ec591e168024505b245cab0e948`, 2026-10-28.
All `file:line` references below refer to that tree, before integration. Search
by the accompanying function or variant name after other agents' changes land.
This is a code integration guide, not a claim about SoundCloud endpoint support.
Only `src/soundcloud/mod.rs` exists at this baseline. APIs of the six planned
modules are not assumed here. Code sketches describe proposed adapters, not
existing callable functions.

The user's personal-fork request authorizes this work despite the upstream
Spotify-only product boundary in `AGENTS.md` and `CONTRIBUTING.md`. Spotify
audio must still come from Spotify through librespot. No change to the
Spotify-only Connect path is intended. No source code was changed for this map.

Read first: `AGENTS.md`, `CONTRIBUTING.md`,
`docs/_reference/queue.md`, `docs/_reference/how-it-connects.md`, and
`docs/_reference/what-spotify-allows.md`. The queue contract applies to both
playback owners: manual rows precede context rows, occurrences are distinct,
starting a new context keeps manual rows, Clear removes only manual rows, and
every action changes the display immediately.

## 1. Playback flow and the routing boundary

### Existing command and event spine

1. A view pushes `model::Action` into `app.actions`. `Action` is defined at
   `src/model.rs:1111`. Views must continue to emit actions without doing IO.
2. `App::apply_actions` (`src/app.rs:10847`) clears `frame_now`, drains the
   actions, calls `apply`, and also drains actions emitted by an earlier action.
   `App::apply` (`src/app.rs:10875`) first handles common bookkeeping, including
   leaving Repeat One when manually choosing another song (`10893`) and
   superseding pending play-search (`10898`), then dispatches at `10904`.
3. Starts converge through `play_request` (`src/app.rs:9084`) into `start_play`
   (`9107`). Reconnection replay calls `start_play` directly (`9101`).
   `start_play` chooses a start offset, caches the expected row, calls
   `expect_track`, records recent context, sets `assumed_context` and
   `queue_start_pending`, then selects `Target` at `9182`.
4. `App::target` (`src/app.rs:1684`) prefers active local librespot, then an
   explicitly selected remote device, then a fresh remote snapshot, and finally
   local-ready or remote-none. It has no concept of a hybrid owner.
5. Local commands use `Backend::player` (`src/backend.rs:1603`), which sends
   `Command::Player(PlayerCommand)` (`758`). Runtime dispatch is at `2339`.
   `Engine::command` (`src/player.rs:716`) handles audio interruption and calls
   `send_command` (`735`), which calls Spirc. `Load` constructs a context/list
   request at `762`, activates Spirc, and loads it at `785`.
6. Remote commands use `Backend::api` (`src/backend.rs:1380`) and
   `Command::Api`, dispatched at `2371`. `ApiRequest::Remote` is executed at
   `5789`; restricted receivers use the session Connect path at `5867`.
   `App::remote` (`src/app.rs:8837`) is only the simple remote-control helper,
   not a common routing point for local playback.
7. Librespot `PlayerEvent`s enter `run_events` (`src/player.rs:920`). Stale
   play-request IDs are rejected at `935`. `apply_event` (`974`) updates
   `LocalState` and emits `EngineEvent::State`. `Runtime::engine_notify`
   (`src/backend.rs:3445`) forwards it as `Event::Local(Box<LocalState>)`
   (`3455`). `App::handle_backend_events` (`src/app.rs:2459`, Local arm `2545`)
   calls `handle_local` (`3162`). `Event::Playback` (`2504`) is engine
   readiness/auth status, handled by `handle_playback` (`2808`), not the
   currently audible song.
8. Spotify reads return through `Event::Api` to `handle_api`
   (`src/app.rs:6759`): `PlaybackState` at `6911`, `Queue` at `6995`.
   Request sequence checks reject superseded reads. These paths also update
   shown state and must be guarded while hybrid playback owns it.

### Every playback/control entry point

| Surface | Emission or adapter | Existing application route |
| --- | --- | --- |
| Song row double-click, Enter, accessibility activation, cover/number Play | `track_row_contents`, `src/ui/widgets.rs:2064`, `2087`; keyboard row navigation `src/ui/collection.rs:1180` | `Action::PlayFromRow`, `src/app.rs:10995` |
| Original playlist/album/Liked Songs header Play | `actions_row`, `src/ui/collection.rs:248`, `306`, `322` | `TogglePlay` when already playing; otherwise `PlayContext` |
| Sorted, filtered or hidden-song view Play | `src/ui/collection.rs:310`; `prepare_table_view:610`, `view_context:669` | `PlayFromRow` with `RowContext::View`, handled at `src/app.rs:11036` |
| Row context forms | `RowContext::Context`, `Uris`, `Queue`, `View` | `src/app.rs:11000`, `11019`, `11035`, `11036`; Queue uniquely calls `play_queue_item:6028` |
| Player bar, including fullscreen lyrics' shared bar | `transport`, `src/ui/player_bar.rs:687` | Shuffle `748`, Previous `762`, toggle `798`, Next `813`, Repeat `844`, seek `897` |
| Player bar volume, mute and queue drop | `extras`, `src/ui/player_bar.rs:911` | PreviewVolume `940`, SetVolume `946`, ToggleMute `970`, QueueMany `1028` |
| Collection Shuffle | `src/ui/collection.rs:354` | `SetShuffle`, without starting the page's context |
| Context menu Play / Shuffle play / Add to queue | `context_menu_items`, `src/ui/widgets.rs:1201` | PlayContext `1214`, ShufflePlay `1228`, AddToQueue `1238` |
| Single/multiple row menu queue operations | `item_menu_at:956`, `picked_menu:675`, `src/ui/widgets.rs` | PlayNext `718`/`979`, QueueMany `729`, AddToQueue `990`, RemoveFromQueue `1006` |
| Queue page, side panel, fullscreen column | `src/ui/queue.rs:15`, `37`, `166`; `contents:341`, `queue_row:732` | Queue row PlayFromRow, ClearQueue `321`, drops `348`/`444`/`565`, move `561` |
| Search song / top result / cards | `src/ui/search.rs:435`, `412`, `496`, `564`, `628` | One-song Uris, PlayUris, PlayContext or TogglePlay |
| Sidebar/list/grid/double-click/menu | `src/ui/sidebar.rs:132`, `217`, `289`, `1991` | TogglePlay or PlayContext |
| Home cards, recent songs, episodes | `src/ui/home.rs:282`, `433`, `479`, `568`, `720`, `796` | TogglePlay, PlayUris, PlayContext, PlayEpisode |
| Artist Play/popular songs, radio pages, new releases | `src/ui/artist.rs:161`, `239`, `399`; radio uses collection widgets; `src/ui/new_releases.rs:123`, `151` | Shared actions; releases use PlayUris and SetShuffle |
| Podcast/episode and prerelease preview rows | `src/ui/show.rs:231`, `394`, `477`; `src/ui/prerelease.rs:423` | PlayEpisode or PlayFromRow |
| Lyrics line click | `src/ui/lyrics.rs:276`, `824` | Seek |
| Main keyboard shortcuts | `src/ui/keys.rs:102`, `105`, `121`, `124` | Space toggle; command arrows previous/next; shift arrows seek; S shuffle; R repeat |
| OS media keys, Linux MPRIS, Windows SMTC, macOS Now Playing | `handle_media_commands`, `src/app.rs:4500` | Play/Pause/Stop are conditional TogglePlay; Next, Previous, SeekBy, Seek, volume, SetShuffle, SetRepeat, OpenUri-to-PlayContext |
| Windows taskbar thumbnail toolbar | `src/thumbbar/win.rs:287`; consumed in `src/entrypoint.rs:1396`; conversion `src/thumbbar.rs:45` | ThumbCommand becomes the shared transport Action |
| Tray menu | `handle_tray`, `src/app.rs:4405`, menu cases `4420` | TogglePlay, Next, Previous |
| macOS app menu / Dock menu | App menu adapter `src/entrypoint.rs:1338`; `handle_dock_menu`, `src/app.rs:4432`; native producers `src/mac_menu.rs:157` | Same transport, seek, shuffle, repeat and volume actions |
| macOS notch / Touch Bar | `src/notch.rs:18`; `handle_notch_commands`, `src/app.rs:4446` | NotchCommand converted to shared Action |
| Winamp mini player and its playlist | `src/ui/winamp/mod.rs:576`, `1409`, `1470`, `1547`; `src/ui/winamp/playlist.rs:104`, `444`, `781` | Shared transport actions, queue-row PlayFromRow, PlayContext |
| MilkDrop child controls | `milkdrop_command`, `src/app.rs:4087` | Previous, Next, TogglePlay, mute, saved, shuffle |
| CLI / Stream Deck / single-instance control socket | `handle_control_commands`, `src/app.rs:4452` | Shared actions including PlayUri, PlaySearch, Transfer, explicit shuffle/repeat and seeks |
| Play-search completion | `ApiResponse::PlaySearch`, `src/app.rs:8083` | PlaySearchMatch -> play_request, guarded by serial `10970` |
| Resume and reconnect replay | `resume_last`, `src/app.rs:9561`; `replay_queued_play:9101` | play_request or start_play, not necessarily a new UI Action |
| Natural end, load failure, deferred album enqueue | `src/player.rs:1083`; `src/app.rs:10062`; `handle_local:3244` | No existing common action; hybrid needs explicit end/error events and an album completion adapter |

OS command handling, tray, listening/history and ticking also run with the
window closed through `background_frame` (`src/app.rs:12623`). Do not make
hybrid advancement depend on drawing a UI view.

### Recommended insertion points: one owner decision, no UI-specific routes

There is **no existing function that receives every playback operation**.
Branching only in `start_play`, `target`, `Backend::player` or a row widget
misses some of the table above. The smallest comprehensive seam for UI and OS
commands is `App::apply`, immediately before its `match action` at
`src/app.rs:10904`, after the common repeat and play-search precedence rules.
Add one hybrid action adapter there, covering starts, transport, queue edits,
refresh and transfer rejection. Keep all existing Spotify arms as fallthrough.

Use one normalized dispatcher inside that adapter. Route internal resumes,
reconnection replays and delayed queue completions into the same dispatcher,
rather than duplicating source detection in their callbacks:

```rust
// Proposed structure, not an API supplied by an unwired module.
// In apply, after common bookkeeping, before the existing match:
if let Some(intent) = self.normalize_playback_action(&action) {
    if self.dispatch_hybrid_if_owned(&intent) {
        return; // fully applied and projected; no legacy queue mutation follows
    }
}
// Existing Spotify action match follows unchanged.

// Shared decision inside dispatch_hybrid_if_owned:
// Start => classify resolved context/list, including overlays and manual rows.
// Other controls => use the active playback owner, not current track's source.
// A known mixed context stays hybrid even when its current row is Spotify.
```

Normalization must not cap lists, issue IO, mutate the old queue, or start a
Spotify endpoint while deciding. Reuse the semantics of the four RowContext
arms at `10995`, preserving clicked occurrence and display order. For
`PlaySearchMatch`, validate its serial before routing. `SeekBy` clamps against
the displayed track; `Previous` uses the existing three-second restart rule
(`src/app.rs:11081`). Preserve the `leave_repeat_one` behavior at `9853`.
Its call to `set_repeat` happens before this adapter: make that helper
owner-aware and send a hybrid repeat intent through the same dispatcher.
Otherwise the common preamble can still issue a Spotify repeat command while
hybrid is active. Apply the same rule to internal calls to seek/volume/queue
helpers; an action adapter alone cannot intercept direct method calls.

For internal `PlayRequest`s, put an early call to that **same dispatcher** in
`start_play`, before Spotify offset selection at `src/app.rs:9118`. This is
the safety net for `resume_last` and `replay_queued_play`; the UI adapter must
return after it handles a hybrid start, so it does not also reach this path.
Resolve mixed plans asynchronously on the backend when rows are not loaded,
retaining the request generation and the clicked occurrence. Never infer a
Spotify playlist is pure merely because its loaded prefix has no SoundCloud
row. Check `playlist_overlay` membership before choosing Connect. Do not let
the existing 500-URI cap (`src/app.rs:13621`, called at `10979`, `11031`,
`11045`) silently truncate a hybrid context before classification.

The owner can be a new `PlaybackOwner::{SpotifyConnect, Hybrid}` separate
from `Target`. Set it when accepting a start, retain it across Spotify rows in
the hybrid queue, and relinquish it only on an explicit replacement/handoff.
`target()` should return local for an active hybrid session before checking
`local.is_active()`. SoundCloud-only contexts and standalone SoundCloud rows
also require the hybrid owner. A local mixed playlist currently containing
only Spotify tracks can use Connect as an explicit ordered URI list, while
retaining its local context identity. Its URI must never be sent as a Spotify
context. Decide this case deliberately in the plan adapter.

Adding SoundCloud to an active pure Spotify local queue needs an explicit
promotion plan: capture current track/position, remaining context order and
manual occurrences before handing ownership to hybrid. The Web API queue is
only a window, not a guaranteed full context. Resolve the full order first or
refuse with a clear explanation; do not silently drop the unseen tail.
SoundCloud additions on another Connect device must refuse, not send them to
Spotify or silently pull playback away. Queueing Spotify rows while hybrid
is active remains hybrid. Starting a pure Spotify collection after a mixed
one must also account for retained SoundCloud manual rows before demoting.

These legacy boundaries must be bypassed when the adapter handles the action:

| Operation | Existing boundary that otherwise talks to Spotify |
| --- | --- |
| Start | `start_play`, `src/app.rs:9107`; local load `9193`, remote branch `9199` |
| Pause/resume, seek, volume | `toggle_play:9574`, `seek:9634`, `set_volume:9685` |
| Next / Previous | `apply`, `src/app.rs:11069`, `11091`; must intercept before `pop_queue_head:6295` |
| Shuffle / repeat | `set_shuffle:9782`, `set_repeat:9864`; hybrid owns order/repeat even on Spotify rows |
| Queue skip | `play_queue_item:6028`; replace the loop of Spirc Next commands with one `skip_to` |
| Add / batch / album | `queue_one:10173`, `queue_many:10112`, `queue_album_tracks:10062`, `write_queue_adds:10211` |
| Play next / insert / move / remove / clear | `apply:11159`, `11166`; positional insert follows; `remove_from_queue:6138`, `clear_queue:6200`, `resync_local_queue:10152` |
| Queue read | `refresh_queue:6001`; project the hybrid queue without an API call |
| Transfer or receiver activation | `transfer:9897`, `apply:11520`, `11526`; reject remote mixed playback before any side effect |

The backend should receive engine work, not own a second logical queue.
Add a separate command family beside `Command::Player` (`src/backend.rs:758`)
for SoundCloud load/control and source handoff, dispatched beside `2339`.
Do not pass a `soundcloud:` URI to `PlayerCommand::Load` or `AddToQueue`.
`SpotifyUri::from_uri` at `src/player.rs:742` will reject it.

For a hybrid Spotify row, bypass `local_load` (`src/app.rs:13566`): its
single-song optimization turns the row into a Spotify context, allowing
Spotify's continuation. Dispatch an explicit **one-element tracks load**,
with no context, shuffle false, repeat Off and autoplay false. Hybrid repeat
and shuffle belong solely to `hybrid_queue`.

```rust
LoadSpec {
    context_uri: None,
    uris: vec![spotify_uri],
    offset_index: Some(0),
    position_ms,
    play: true,
    shuffle: Some(false),
    repeat: Some(RepeatMode::Off),
    autoplay: false,
    ..Default::default()
}
```

Verify this suppresses fork-level continuation. App autoplay also needs a
hybrid guard at `handle_local` (`src/app.rs:3244`, helper `13599`).
`PlayerEvent::EndOfTrack` is currently discarded at `src/player.rs:1083`.
Add a tagged end/failure signal before that discard, routed backend -> app,
with playback generation and occurrence ID. Exactly one matching completion
advances hybrid. A Stopped snapshot alone is ambiguous: replacement, explicit
stop, disconnect and natural end can all stop an engine.

`PlayerCommand` (`src/player.rs:287`) has no Stop variant. The handoff adapter
needs an explicit, acknowledged stop/cancel path, and a way to prevent late
librespot output/events or a kept reconnect player from reviving. Do not
implement source handoff using Toggle. SoundCloud and librespot must have one
audible owner even if they share the same sink implementation.

## 2. Shown state, optimism and the queue panel

`NowPlaying` lives at `src/app.rs:175` (fields follow there). The main read
surface is `now_playing` (`1928`): frame snapshot, requested-track preview,
live state, then a paused resume preview. `refresh_frame_now` (`1937`) mirrors
that selection. `current_track_uri` (`1736`) and `row_is_played_copy` (`1749`)
drive row highlighting. `played_row` differentiates duplicate copies.

The live local branch (`src/app.rs:1972`) reads `local.track` (`LocalTrack`,
`src/player.rs:149`): uri/title/artists/album/art URLs/duration/episode flag.
`LocalState` (`172`) contributes playback, loading, position/position_at,
volume, shuffle, repeat, seek_sequence and track_sequence. `position_now`
(`src/player.rs:227`) interpolates only while playing. The remote branch
(`src/app.rs:2041`) reads `RemoteSnapshot.state.item`, context/device,
is_playing, progress_ms, shuffle/repeat/volume, and received_at.

`player_bar::show` (`src/ui/player_bar.rs:46`) takes this snapshot.
`now_playing_block:415` draws artwork, title, artists, album/show links and
heart. `transport:687` reads playing/loading/shuffle/repeat/can_control and
position_ms/duration_ms. `extras:911` reads volume/can_set_volume and device
identity. The bar itself does not need a SoundCloud-specific transport.

Queue rendering (`src/ui/queue.rs:341`) reads `app.queue: Loadable<Queue>`.
`Queue` (`src/api/models.rs:790`) contains currently_playing and queue. At
`src/ui/queue.rs:371`, the renderer prefers the now-playing URI, then cached
metadata, over a lagging queue current row. At `385`, `playing_from` supplies
the linked context name. `queued_rows_len` (`src/app.rs:6101`) divides manual
Playing next from context Next up. `queue_row` (`src/ui/queue.rs:732`) uses
the same TrackRow and RowContext::Queue as other lists. Local edit capability
currently tests `local.is_active()` in `queue_locally_reorderable` (`6119`),
so merely returning Target::Local is insufficient for SoundCloud controls.

Existing optimistic protections to preserve:

| Mechanism | Code and behavior |
| --- | --- |
| Expected track | `expect_track`, `src/app.rs:1838`; requested preview `1946`; pending keys `2223` show title/row/loading before confirmation |
| Ordered local confirmation | `reconcile_local_track_intent:1856`, called from `handle_local:3194`; a new local track report settles the intention |
| Lagging remote answers | `reconcile_remote_track_intent:1870` uses poll sequence and mismatch allowance; old polls do not undo a click |
| Context hold | `playing_context_uri:1710`, `assumed_context` set at `9176`; sorted/filtered list keeps its source context even though loaded as URIs |
| Play and seek | `optimistic_playing` read at `1896`/`1990`; local/remote pending positions `2015`/`2060`; seek sequence confirmation `3203` |
| Next and queue changes | `pop_queue_head:6295`, `show_queued_song:10193`, positional mutations and `queue_start_pending` update display before IO |
| Stale queue reads | `queue_fetch_is_stale:6349`, `ApiResponse::Queue:6995` retain pending additions, clear/remove/reorder/shuffle, then recheck |
| Confirmed transitions | `on_now_playing_changed:3569` consumes occurrences and saves resume state, distinguishing track_sequence from URI equality |

Recommended hybrid projection: add a separate active hybrid snapshot in App,
then adapt it to these same presentation fields. Do not overwrite the raw
librespot `local` with synthetic SoundCloud state and later feed it through
`handle_local`, which also runs Spotify recovery/autoplay logic.

```rust
// First live branch in now_playing_live, before local/remote:
if let Some(hybrid) = self.active_hybrid_snapshot() {
    return Some(hybrid.to_now_playing());
}
// After each pure queue mutation, synchronously:
self.queue = Loadable::Loaded(hybrid.to_api_queue());
self.manual_queue = hybrid.manual_uris(); // presentation compatibility only
self.session_dirty = true;
```

Project full Track metadata, local=true, controllability, position, loading,
volume and the **hybrid** shuffle/repeat, even while its engine is librespot.
The queue module remains authoritative; `manual_queue` is a projection, not a
second mutating queue. Extend `now_playing_item`, `cached_track_preview:2145`
and the track cache path to find SoundCloud metadata by full URI. The current
cache is keyed by bare Spotify ID (`now_playing_item:1915`, local branch
`1977`), and `cached_track_preview` only accepts `spotify:track:`.

Give each accepted command/engine load a generation and occurrence ID.
Retain optimistic expected track/seek until the matching engine confirms or
fails; a late event from the previous source must never clear it. Update
`refresh_frame_now` and invalidate `frame_now` on projection, preserving the
one-snapshot-per-frame behavior. Repeated identical URIs still need distinct
sequences, including Repeat One, for markers/history/media controls.

While hybrid owns playback, raw `Event::Local` can update Spotify readiness
and feed a matching Spotify child load, but must not run its unrelated queue
consumption, autoplay, repeat/shuffle adoption or resume overwrite. Guard
`handle_local:3162`, the `PlaybackState` arm (`6911`) and `Queue` arm (`6995`).
At hybrid entry, invalidate outstanding remote/queue sequences and clear
Spotify-specific pending holds/batches. Keeping a timeout hold is not enough:
Spotify is never authoritative for the hybrid queue. Continue device/catalogue
reads as needed. An incoming external Connect takeover requires an explicit
ownership policy and clean audio handoff, not accidental last-event-wins.

`playing_context_uri` (`1710`) and `playing_from` (`3345`) need a hybrid-first
branch. Supply context URI/name/page from the hybrid plan for mixed playlists,
SoundCloud sets/users/likes and Spotify playlists with overlays. Avoid asking
the one-track Spotify child load for context identity. Recent ordering should
record the original context, not that child track.

## 3. Local-file inventory and source capability guards

Reproduce the lexical audit with:

```sh
rg -n 'is_local|spotify:local|local_files' src --glob '*.rs'
rg -n 'LOCAL_URI_PREFIX|engine_queues|local_file_key|local_file\(|local_art\(' src
```

The first search alone misses named-prefix guards. Local files are not a
complete permission model: several Spotify-only actions are currently offered
to any `PlayableItem::Track`, including local files. Do not simply replace
`is_local` with `is_local || soundcloud`. **Keep SoundCloud Track.is_local
false**, use the URI scheme to identify source, and gate capabilities explicitly.
`Source::of` at `src/soundcloud/mod.rs:80` also treats every non-SoundCloud URI
as Spotify; it is not a validator for Spotify API calls.

### Production sites containing the local-file special cases

| File and anchors | Existing responsibility | SoundCloud treatment |
| --- | --- | --- |
| `src/api/models.rs:331`, `667` | Track and PlaylistItem is_local flags | Preserve Spotify-local meaning; SoundCloud availability uses streamability, not a folder scan |
| `src/app.rs:663`, `1255`, `1327`; `2486`, `2490` | local_files map, initialization, folder scan/result | Separate SoundCloud metadata/availability store; never add streams to this map |
| `src/app.rs:1810`, `1813`, `1829`, `1832` | Exact duplicate-row start and positional URI lookup reject missing/local rows | Hybrid uses stable occurrence mapping, not Spotify index heuristics |
| `src/app.rs:3595` | Resume manual local row through engine instead of Web API | Restore hybrid snapshot separately; never submit SoundCloud queue to either Spotify route |
| `src/app.rs:6612` | request_contains skips Spotify locals | Also exclude SoundCloud and siphon URIs from Spotify membership checks |
| `src/app.rs:9065`, `9068` | playlist_start finds first available catalogue row | Use resolved hybrid order for overlay contexts before this selection |
| `src/app.rs:10066` | Album queue excludes local/unavailable tracks | Source-aware playability for hybrid album/playlist batches |
| `src/app.rs:10437`, `10452`, `10462`, `10474`, `10481`, `10489` | Folder setting/restart, can_queue, local_file, local_art, item_image, row_playable | Separate source eligibility; image URL path is reusable; file matching remains Spotify-local only |
| `src/app.rs:10672` | Optimistic Spotify PlaylistItem sets is_local false | Never splice SoundCloud into raw Spotify page items here |
| `src/app.rs:12267`, `12268`, `12270`; `13372`, `13376`, `13384`; `13428` | Local-folder actions, prefix, engine_queues, key normalization, EngineConfig | Do not add soundcloud prefix to engine_queues or SpotifyUri parser |
| `src/backend.rs:48`, `52`; `732`, `738`; `958`, `962`, `970`; `1368`; `2240`, `2249` | Filesystem discovery/picker, LyricsRequest.local_file, result events | No SoundCloud use of disk scanner or embedded-tag path |
| `src/player.rs:65`, `66`, `525` | EngineConfig.local_files -> librespot local_file_directories | Leave for Spotify-local playback |
| `src/session_reads.rs:1577`, `1614`, `1624` | Session playlist local-row mapping and percent-decoded local metadata normalization | Keep mapping Spotify-only; merge overlay after raw rows are mapped |
| `src/ui/collection.rs:512`, `604`, `632`, `658` | Table cache changes when local scan size changes | Add SoundCloud/overlay availability/revision invalidation independently |
| `src/ui/collection.rs:1436` | Carry PlaylistItem.is_local into Track during row conversion | SoundCloud adapter must not set it |
| `src/ui/widgets.rs:1314` | Base row_playable excludes local, empty, explicit unplayable | SoundCloud can pass when streamable; App adds source-aware eligibility |
| `src/settings.rs:285`, `286`, `491`, `579` | Local folder persistence/default/getter | SoundCloud account belongs in its own storage, not this setting |
| `src/ui/settings.rs:342`, `717`, `1005` | Local folder detail and picker | Add account UI separately; no SoundCloud folder control |
| `src/local_tags.rs:24`, `29`, `93`, `99`; `src/images.rs:105`, `459`; `src/backend.rs:4573` | File artwork URI, embedded picture/lyrics; lexical search does not catch all of these | Not a template for SoundCloud network lookup |
| `src/app.rs:1695`; `src/ui/devices.rs:524`, `526`, `566` | Variables named is_local describe device identity | Not local-track filters; keep them distinct from catalogue source |
| `src/soundcloud/mod.rs:5`, `104` | Analogy and URI helper tests | Existing convention and negative local-URI test |

### Spotify-only actions requiring guards beyond that grep

| Capability | UI and application boundaries |
| --- | --- |
| Spotify heart/save/contains | `src/ui/widgets.rs:737`, `1010`, `1973`; `src/ui/player_bar.rs:596`, `631`, `662`; `src/app.rs:1591`, `6606`, `11247`, `11262`; set_saved helper and backend ApiRequest::SetSaved (`src/backend.rs:290`, `5612`) |
| Add to Spotify playlist | `add_to_playlist_menu`, `src/ui/widgets.rs:772`; picked menu `769`, row menu `1023`; App request/add at `10597`, `10629`; duplicate-check request at `10619` must also exclude foreign URIs |
| Queue / Play next / queue playlist export | `src/app.rs:10452`, `10173`, `10211`, `10152`, `6251`, `6281`; export mixed queue to a local mixed playlist, not CreatePlaylist with foreign URIs |
| Radio, hidden songs, autoplay | `src/ui/widgets.rs:1095`, `1115`, `1292`; `src/app.rs:5427`, `5458`; `src/app/radio.rs`; `src/app.rs:3244` |
| Share/open links | `src/ui/widgets.rs:1188`, `1196`, `1300`, `1308`; `src/app.rs:11551`, `11557`; `util::open_spotify_url`, `src/util.rs:206`, does not validate scheme and currently fabricates Spotify links for SoundCloud URIs |
| Artist/album navigation and metadata/credits | `src/ui/widgets.rs:1126`, `1155`, `1379`, `1904`; `src/ui/player_bar.rs:481`, `530`, `561`; `src/app.rs:3541`, `3639`, `6557`; do not turn SoundCloud uploader/set IDs into Spotify artist/album requests |
| Lyrics | `App::request_lyrics`, `src/app.rs:3702`; `fetch_lyrics`, `src/backend.rs:4563`, calls Spotify first then LRCLIB. SoundCloud should show unavailable for this integration and issue neither Spotify nor inferred lyrics requests |
| Clipboard | `paste_songs:10713`, `known_song:10762`, `CopySongs:11557`; parser `src/link.rs`, plus cut/paste shortcuts `src/ui/collection.rs:1073` |
| Current-track cache enrichment | `src/app.rs:3639` calls ApiRequest::Track for a local now-playing ID; add a Spotify scheme guard before this block, regardless of cache contents |

There is no standalone song-credits dialog/API in this tree. Artist credits
come from `LocalTrack.artists` and Spotify Track metadata; album copyright
credits are drawn at `src/ui/collection.rs:2009`. Do not invent a credits
endpoint or populate a fake Spotify album to gain artwork.

For SoundCloud, the plan supports queue and local playlist/overlay actions.
It does not authorize Spotify Like, native Spotify playlist membership,
radio, Connect queue, Spotify lyrics/credits or Spotify share links. If
SoundCloud Like or permalink sharing is later wired, use explicitly labeled
SoundCloud actions and its real returned permalink. `ExternalUrls` only has
a `spotify` field (`src/api/models.rs:189`); do not store a SoundCloud URL there.
Mixed multi-selection must filter per capability while retaining order and
duplicate occurrences, including keyboard and drag paths, and show an honest
count. Guard App/backend boundaries as well as menus.

### Fixtures and tests found by the local-file search

Keep these existing regression groups intact and add parallel source tests:

- `src/backend.rs:6856` local playlist JSON normalization fixture.
- `src/app.rs:17601`, `18368`, `19390` to `19494`: queue restore, repeated
  row metadata, local queue eligibility and no Web API leakage.
- `src/app.rs:27498` to `27584`: local playability, artwork, folder restart.
- `src/app.rs:29042` to `29088`, `29527`: duplicate start-position and cached
  availability fixtures.
- `src/player.rs:1854`: default EngineConfig fixture.
- `src/session_reads.rs:2422` to `2514`, `2783`: local mapping, URI encoding,
  invalid URI handling and cache serialization fixtures.
- `src/ui/collection.rs:3151`, `3377`: table test data and scan cache key.
- `src/link.rs:254`: rejected spotify:local link test.
- `src/demo.rs:303`, `305`, `309`, `318`, `322`, `389`, `423`, `1314`
  to `1327`: found/missing local row fixtures and flags.

## 4. Search with minimum churn

`SearchFilter` (`src/model.rs:778`) is the **entity kind** filter, with All,
Songs, Artists, Albums, Playlists, Podcasts, Episodes. Keep it. `SearchState`
(`815`) owns query/committed, serial, results/results_serial, split playlist
result, pending bits, error, filter and typing-run/debounce state.

`run_search` (`src/app.rs:6430`) bumps the serial, resets results with the
committed query, and submits `ApiRequest::Search`, including empty query to
cancel Spotify work. `run_typed_search:6460` and `submit_typed_search:6469`
preserve history typing-run semantics. Backend `search` (`src/backend.rs:4745`)
cancels preceding tasks, splits playlists to shared grant when available,
emits SearchStarted, and dispatches catalogue/full search. Execution is at
`5625`, `5633`, `5670`. Responses `SearchStarted`, `SearchPlaylists`, `Search`
are handled at `src/app.rs:8073`, `8114`, `8132`, with serial **and query**
guards. Preserve this split and its independent rate-limit behavior.

`src/ui/search.rs:15` shows entity chips at `22`, pending/error at `34`, then
has Spotify-results early returns at `41` to `70`. `all:128` builds Spotify
shelves; `songs:435` draws ordinary TrackRows. Each existing Spotify search
song plays a one-song Uris context (`455`), not the whole results list.

Minimum extension:

```rust
enum SearchSource { All, Spotify, SoundCloud } // separate from SearchFilter
// SearchState additions, with independent result/error/pending ownership:
source: SearchSource,
soundcloud: Loadable<SoundCloudSearchResults>,
soundcloud_serial: u64,
soundcloud_pending: bool,
// Action::SetSearchSource(SearchSource)
// Command::SoundCloudSearch { query, serial, account_generation }
// Event::SoundCloudSearch { query, serial, account_generation, result }
```

1. Insert All / Spotify / SoundCloud chips before entity chips at
   `src/ui/search.rs:22`. Use `widgets::chips` (`src/ui/widgets.rs:3114`) and
   emit SetSearchSource. It is transient view state, not a new Settings field.
2. Add SoundCloud search alongside the Spotify send at `src/app.rs:6453`.
   With source All, requests run independently. With Spotify selected, cancel
   or skip SoundCloud; with SoundCloud selected, cancel/skip Spotify using its
   empty-query cancellation path. A source change must invalidate query
   deduplication at `6431`, even when committed text is unchanged. Either bump
   serial and reload selected sources, or explicitly load the missing side
   without tripping that early return. Reject old-source and old-account results.
3. Keep SoundCloud Command/Event outside ApiRequest and the Spotify gateway
   authorization/budget. The runtime calls the unwired async client through
   the existing configured HTTP transport. Cancel on query/account changes.
4. Extract the Spotify results portion of `show` into a small helper, or move
   its early returns into a source-local block. Otherwise Spotify Loading,
   Failed or empty will prevent a successful SoundCloud section from drawing.
   The page-level empty state must wait for all visible sources to settle.
5. Append a SoundCloud section for All/Songs with the returned Tracks mapped
   to PlayableItem::Track. Reuse `track_row`; use one-track Uris to preserve
   search playback behavior. Sets and users use source-aware cards/pages,
   never `Page::Playlist` or `Page::Artist` with a bare SoundCloud ID.
   Entity chips for unsupported SoundCloud kinds show a source-local empty
   state or are disabled, without changing Spotify filters.
6. Add source-independent pending/error display. Spotify's `search_failed`
   (`src/app.rs:6525`) must not set the entire combined view Failed when
   SoundCloud succeeds. Remember one successful query using the existing
   typing-run logic (`6486`), including SoundCloud-only success.

## 5. Sidebar, library, playlist pages and overlays

### Existing data and edit flow

`Library` (`src/model.rs:566`) has the Spotify playlists Loadable plus
pagination/generation/incoming-whole-list and optimistic edit bookkeeping,
then liked/albums/artists/shows/episodes and filter. `App::load_playlists`
(`src/app.rs:4810`) and `read_rootlist:4762` populate Spotify library/order.
`apply_playlist_edits:4860` prevents stale server listings from undoing
creation/removal. Account cache writes at `4898` persist that Spotify list.

Sidebar `playlist_rows` (`src/ui/sidebar.rs:718`) chooses flat or folder rows,
`folder_rows:758` joins rootlist URIs to library metadata, `playlist_entry:896`
makes `Page::Playlist(id)`, and `order_entries:440` applies pins/recent/custom
order/name ordering. `finish_entry_interaction:228`, drop arm `258`/`266`,
distinguishes Liked Songs saves from AddToPlaylist. Sidebar create is at
`1047`. `editable_playlists` (`src/app.rs:12603`) currently enumerates only
Spotify destinations. The add-menu uses that list at `src/ui/widgets.rs:772`.

`Page::Playlist` is loaded by `ensure_loaded` (`src/app.rs:5180`): create a
generation, request metadata at `5196`, request rows/cache separately.
`load_more:5658`, `load_window:5788`, `load_playlist_items_at:5860` handle
paged/windowed reads. Metadata/row acceptance checks generations and snapshots
(`handle_api`, `src/app.rs:7392`, `7497`); edits must not be overwritten by
old page/cache responses. `PlaylistPage` (`src/model.rs:841`) keeps raw
PlaylistItems, filter, contributors, cache progress, local additions,
optimistic_snapshot, pending_writes and refresh-after-write state.

`collection::playlist` (`src/ui/collection.rs:1669`) temporarily removes the
page from the map, converts raw items using `playlist_cached_table_items`
(`1500`, call at `1682`), draws hero/actions, then a Table at `1795`.
`playlist_rows:1423` drops null entries from displayed rows but records each
raw row position in `positions:1453`; it returns `(PlayableItem, added_at,
added_by)` tuples. `Table` (`472`) separates those items from RowContext,
pagination and page identity. `playlist_position:1272` maps a displayed item
back to **server** position using positions and row_offset. `view_indices`
(`1357`) handles text filter/sort; `prepare_table_view:610` caches display
order plus playable URIs/positions. `view_context:669` retains edit rights
while replacing context playback with displayed ordered URIs.

Drag payload is `model::DragTrack`; row creation is in
`src/ui/widgets.rs:2176`. Table drops at `src/ui/collection.rs:1015`/`1022`
emit MoveInPlaylist for same-list rows or InsertInPlaylist otherwise.
Selection menus are `src/ui/widgets.rs:675`; list cut/copy/paste/delete at
`src/ui/collection.rs:1073` emits the same actions. Sorted views retain remove
rights but suppress positional moves (`src/ui/widgets.rs:1032`).

| Edit | App and backend send |
| --- | --- |
| Add/insert | `request_playlist_add`, `src/app.rs:10597`, checks duplicates locally or via API; `add_to_playlist_now:10629` mutates shown raw rows and totals, then sends ApiRequest::AddToPlaylist at `10702` |
| Remove | `Action::RemoveFromPlaylist`, `src/app.rs:11303`, uses URI removal unless repeated occurrence needs `repeated_rows_to_remove:10501` and `remove_playlist_rows:10538` -> Command::RemovePlaylistRows `10573` |
| Move | `src/app.rs:11346` captures snapshot, changes raw PagedList, sends ApiRequest::ReorderPlaylist `11365` with range_start/insert_before |
| Create/rename/delete/cover | Actions start at `src/model.rs:1247`, `1254`, `1260`, `1252`; existing dialogs are Spotify-specific |
| Write protection | `prepare_playlist_mutation`, `src/app.rs:10576`, invalidates old generations/windows/cache; PlaylistChanged response around `7750` settles writes/snapshot |
| Runtime execution | `src/backend.rs:5543` add, `5553` remove, `5566` reorder; duplicate occurrence session removal `4381` reads revision then writes exact indices once |

### Mixed local playlists and SoundCloud pages

Use separate App/Library stores for local mixed playlists, SoundCloud sets and
SoundCloud likes. Append their source-aware sidebar entries after Spotify
folder construction and before common sort/pin filtering. Do not add them to
the Spotify rootlist or account library-cache payload. Use full URI keys for
sidebar pins/recent order, drag targets and page caches. Bare numeric IDs can
collide across services.

Add dedicated Page variants for local mixed playlist, SoundCloud playlist,
SoundCloud user and SoundCloud likes. Their loaders must branch before
`ensure_loaded`'s Spotify connection gate (`src/app.rs:5137`), so local stores
can load without a Spotify API call. The current shell also gates everything
on Spotify sign-in (`src/ui/mod.rs:44`); an entirely Spotify-signed-out
SoundCloud mode is a separate explicit product decision, not something this
page branch alone enables. Optional SoundCloud account connection should not
be confused with this shell gate.

Reuse collection hero/actions/table with finite loaded rows and a source-aware
edit target. Preserve Spotify-specific PlaylistPage raw paging/cache fields;
do not fake a Spotify snapshot or owner permission for a local list. A
read-only SoundCloud set/user/likes page can reuse Table with no editable
Spotify target; add-to-local and playback still work. The planned client does
not imply SoundCloud playlist mutation support, so do not expose its native
rename/add/remove/reorder without an implemented API contract.

Local mixed playlist edits should emit typed local actions or carry a typed
destination, update the store and displayed revision immediately, then hand
encoded bytes to `app.files`. Extend destination menus and sidebar DnD through
the same typed destination adapter. Do not put local IDs through
`request_playlist_add`, `can_edit_playlist`, Spotify duplicate checks or cover
upload endpoints. Clipboard links likewise need provider-specific parsing and
resolution before the local edit.

### Overlay insertion: preserve two coordinate systems

**Never splice overlay rows into `PlaylistPage.items.items`.** It is the raw
Spotify coordinate system used by paging, snapshot caches, duplicate removal,
offset starts and reordering. Keep the overlay store separate, scoped by
Spotify account plus playlist URI and local overlay revision.

The rendering merge belongs in a derived view after raw row conversion at
`src/ui/collection.rs:1689`, before hidden/filter/sort/view preparation. The
playback merge belongs in the shared context-plan resolver, not only here:
sidebar Play must see the overlay even if the page was never opened.

```rust
enum RowOrigin {
    Spotify { absolute_position: u32, occurrence_key: String },
    Overlay { entry_id: String },
}
struct DisplayRow {
    item: PlayableItem,
    origin: RowOrigin,
    // added_at / added_by, stable row key as required
}
// raw Spotify page -> rows + raw positions -> overlay merge -> sort/filter
// retain origins through every transformation
```

The existing `Vec<usize>` pagination positions and `Vec<u32>` picked positions
cannot represent "this row has no Spotify position" safely. Introduce a
parallel origin map or typed row identity. Never use a sentinel index or the
merged row index for a Spotify write. `TrackRow.position`
(`src/ui/widgets.rs:1322` struct) is already optional for single rows, but
multi-selection and drag payloads must also retain origins. `RowContext::View`
preserves context/edit rights, not enough origin information by itself.

- Removing Spotify rows maps to original absolute indices; removing overlay
  rows edits only overlay entry IDs. A mixed selection splits into operations
  without losing duplicate identity. Spotify URI-only removal still removes
  all copies, so use the existing revision-based path for repeated copies.
- A Spotify row reordered relative to another Spotify row uses raw Spotify
  from/to boundaries. Moving an overlay row changes local anchors only.
  Specify cross-origin drop semantics; if unmapped, refuse that drop instead
  of sending the visual index. Count only preceding Spotify entries when
  translating an insertion boundary.
- Keep Spotify page total/next_offset/base_offset/cache checkpoint unchanged.
  Derived merged row count/duration are separate display values. The virtual
  finite table at `src/ui/collection.rs:1801` assumes Spotify total/positions;
  it needs an origin-aware extent/missing-window mapping before overlays can
  occupy those slots. A safe first integration fully loads the relevant
  context or uses a separate finite derived table rather than pretending
  remote windows include overlay positions.
- Overlay anchors must survive Spotify edits by stable occurrence identity,
  with a documented fallback when an anchor disappears. URI alone cannot
  distinguish repeated tracks. Recompute merged view on server snapshot or
  local overlay revision changes. Include overlay revision in table cache
  keys (`src/ui/collection.rs:506`, `610`, `1701`) and playback-plan generation.
- In an overlaid Spotify page, adding a SoundCloud row means "add a local
  overlay", not a successful Spotify library edit. Make that destination
  behavior visible. Never submit the foreign row to Spotify duplicate checks
  or mutation requests. A Spotify failure must not roll back a successful
  independent overlay edit, or vice versa.

## 6. Rows, artwork and source identity

The shared row implementation is `TrackRow` / `track_row` /
`track_row_contents` (`src/ui/widgets.rs:1322`, `1452`, `1482`). Column sizing
is at `1411`. Artwork is drawn at `1635` using
`app.item_image(row.item, app.art_px(size))`. Title/subtitle layout starts at
`1688`; artist links use the helper near `1350`. Album is at `1883`, added by
`1911`, dates `1930` vicinity, plays `1958`, heart `1973`, duration `2008`,
then more/context menus. `collection::table` (`src/ui/collection.rs:687`)
supplies row flags, index, selection, context and original position.

Put a small source badge inside the existing title/subtitle area starting at
`src/ui/widgets.rs:1688`, reserving width before ellipsis. Apply in compact,
thin and normal title paths, plus the player now-playing block if desired.
Use URI source, not Track.is_local or NowPlaying.local: the latter means
"on this computer". Add accessible source text and a tooltip. Do not hide the
title at narrow widths or change the row height unnecessarily. This is a
visible change and needs matched theme/size evidence when implemented.

`Track::image` (`src/api/models.rs:351`) only reads album.images.
`PlayableItem::image` (`642`) delegates to it. The SoundCloud Track adapter
therefore needs an image-bearing Album-shaped display container with empty
Spotify ID/URI, or a backwards-compatible independent artwork field/helper.
An empty album ID must not become a clickable Spotify album; artist/uploader
links similarly need service-aware targets. Keep the real source metadata in
the SoundCloud cache rather than fabricating Spotify entity IDs.

**Yes, the image pipeline already accepts arbitrary HTTPS artwork URLs,
including `https://i1.sndcdn.com/...`.** There is no Spotify hostname allowlist
in `is_art` (`src/images.rs:102`) or `Inner::fetch` (`447`). Fetch uses the
configured `Http` client at `472`. The full URL hashes to a SHA-1 cache filename
(`437`), disk reads run off-thread (`462`), downloads check status and size
(`479`, `484`, `490`), then atomically write through a `.part` rename on a
worker (`501`). `ArtLoader::prefetch:283`, `cached_file:270`, scaled URI `72`,
`CoverScaler:1098` and texture eviction are provider-independent. No new image
client or domain exception is needed. This is a code-path finding, not a live
network test of that host. Do not attach SoundCloud session tokens to artwork
requests or persist expiring authenticated stream URLs as artwork.

## 7. Persistence, resume, history and recently played

`AppDirs::session_file` and `history_file` are in `src/paths.rs`.
`App::new` loads History (`src/app.rs:969`) and SessionState (`1002`), restores
resume fields at `1287`, cached queue rows and table sorts from that session.
`SessionState` (`src/settings.rs:1837`) is serde-defaulted for compatibility.
It saves last page/context/track/position, **manual URI queue**, a display-only
30-row queue preview, shuffle/repeat, recent_contexts, sorts and window state.
`App::save_session` (`src/app.rs:13113`) encodes and hands bytes to
`self.files.write` (`13163`); `shutdown:13180` saves and flushes.

`request_resume_track` (`src/app.rs:3296`) only fetches Spotify tracks.
`cached_track_preview:2145` is also Spotify-only. `ensure_resume_context_loaded`
(`3326`), `context_page:3447`, `step_resume:3459`, `context_track_uris:8960`
and `resume_last:9561` all need source-aware alternatives. Existing
`on_now_playing_changed:3587` re-enqueues saved manual URIs to Spotify when
the remembered local track resumes; hybrid must bypass that block entirely.

Add an optional, serde-defaulted versioned hybrid snapshot to SessionState,
or an atomically written sidecar referenced by it. It must retain:

- Owner and logical context URI/name/provider, plus local playlist or overlay
  revision and account scope. Do not infer owner from the current row alone.
- Full ordered context entry identities and current occurrence/cursor,
  shuffle permutation/RNG state as required by the queue API, repeat mode,
  manual entries including duplicate IDs, and previous/history cursor needed
  by hybrid Previous. A 30-row display preview is not a playback snapshot.
- Metadata for the current row and enough queued rows for an immediate paused
  preview, current position and safe paused-on-restart behavior, without
  persisting Instant or live decoder handles. Restore the logical queue
  first, then start playback only on explicit resume as today.
- Stable SoundCloud IDs/URIs, not resolved stream URLs or session tokens.
  Resolve stream URLs afresh, and gracefully skip/refuse unavailable tracks
  without replacing the saved context with a stale Spotify poll.

Keep old SessionState files readable and bound snapshot size. Restore new Page
encode/decode values (`src/model.rs:133`, `154`) and table-sort keys. Local
mixed/overlay files belong to their planned modules, loaded off-thread and
written through fastframe_file::Writer. SoundCloud account secrets belong in
the account module's protected store, with generations/cancellation on
disconnect. Do not put them in session/history/settings JSON or logs.

History is local, not Spotify reporting: `History` (`src/history.rs:35`)
stores up to 500 PlayHistory records, atomically through Writer (`71`).
`note_listening` (`src/app.rs:12826`) observes **live** playback, records once
after min(30 seconds, half-duration), with a one-second lower bound
(`src/history.rs:26`), and excludes paused time/seek jumps. Repeated local
track_sequence changes reset listening at `src/app.rs:3169`.
`history::played_track` (`src/history.rs:121`) reconstructs Track metadata;
`History::record:98` currently records no context. `merged:155` interleaves
local plays with Spotify recently played using URI/time identity, and
`App::rebuild_recents` (`src/app.rs:8908`) updates `recents_view`.

Feed hybrid's confirmed audible state through the live read surface so the
same threshold works for both sources. Reset listening for each occurrence,
including the same URI replay. Preserve SoundCloud artwork/source metadata
in played_track and avoid routing its ID to Spotify enrichment/contains.
If context-bearing local history is desired, add it compatibly; it is absent
today. `home.recently_played` comes separately from Spotify
(`src/app.rs:7035`) and is not automatically the merged Recent tab. Deliberately
extend Home's recent view if required rather than claiming current history
already makes SoundCloud appear there. `note_recent_context:8860` can order
new sidebar URIs once their entry types are added.

## 8. Demo fixtures and deterministic verification

`src/demo.rs` is compiled for tests or the demo feature (`src/lib.rs:14`).
`populate` (`src/demo.rs:340`) sets backend offline and App offline, marks the
fake Spotify session connected, populates all page/library models, seeds
search (`694`), queue (`705`), remote playback (`753`) and track_cache (`770`).
`play_here:964` copies the shown remote metadata into LocalTrack for local
scenarios. `apply_flags:1064` selects pages and extra surfaces; local found/
missing rows (`303`, `309`) are a useful pattern for source eligibility.

The CLI wiring is `src/entrypoint.rs:35` to `99`: --demo, --demo-data,
--demo-page, --demo-show, --demo-shot, --demo-shot-delay, --demo-size,
--demo-drag and --demo-click. Population/flags run at `550`; isolated demo
directories are set at `439`. Existing headless egui tests live throughout
demo.rs and exercise emitted actions/accessible nodes.

The demo is deterministic in fixture shape, not completely network-free:
`image:22` uses seeded picsum URLs, and populate uses wall-clock-derived dates.
For new comparison tests, use fixed SoundCloud fixture IDs/titles, fixed
positions and no advancing clock, a fixed hybrid shuffle seed/order, and
embedded/generated in-memory cover bytes or a seeded disk cache. Do not call
the actual SoundCloud API/account/engine. Extend the offline command-recording
seam (`Backend::send:1312`, `player:1603`) to capture SoundCloud commands and
allow injected events, as existing Spotify tests do.

Add opt-in flags rather than changing every existing fixture: SoundCloud
search, user/likes/set, mixed playlist, overlaid Spotify playlist, and mixed
now-playing. Include Spotify A / SoundCloud B / Spotify A, duplicate manual
rows, unavailable/preview rows, no artwork, long/RTL text, paused/loading,
seek, source handoff and remote-device refusal states. Keep Spotify-only
demo output and assertions unchanged unless the UI scope requires it.

## 9. Model variants and exhaustiveness checklist

Prefer shared playback actions. SoundCloud does not need separate Next,
Seek, TogglePlay, Shuffle or Repeat variants.

| Current model | Needed extension or sibling |
| --- | --- |
| Page::Playlist/Artist/LikedSongs, `src/model.rs:110` | MixedPlaylist(local ID), SoundCloudPlaylist(u64), SoundCloudUser(u64), SoundCloudLikes(account key as needed). Add encode/decode and exact scheme-aware from_uri |
| Page::from_uri, `src/model.rs:182` | Currently discards scheme, so soundcloud:playlist:5 incorrectly becomes Spotify Page::Playlist(5). Dispatch on scheme first, including spotifast:playlist |
| Action::SetSearchFilter, `src/model.rs:1305` | SetSearchSource(SearchSource), keeping entity filter separate |
| Action::Create/Update/DeletePlaylist, Add/Insert/Remove/MoveInPlaylist, `src/model.rs:1215`, `1247` | Typed playlist destination/origin, or mixed-playlist and overlay siblings; do not reuse a bare ID with implicit Spotify meaning |
| Action::CopyLink/OpenInSpotify, `src/model.rs:1284`, `1299` | Source-aware CopyLink or explicit SoundCloud permalink/OpenUrl path, only when real permalink is available |
| Action::ToggleSaved/SetSavedMany, `src/model.rs:1180`, `1211` | Leave Spotify semantics; a later SoundCloud like action must be explicit and backed by client capability |
| Action::SignIn/SignOut, `src/model.rs:1324` | ConnectSoundCloud / DisconnectSoundCloud and optional account submission, independent of Spotify credential teardown |
| Dialog::CreatePlaylist/EditPlaylist/ConfirmDeletePlaylist, `src/model.rs:1063`, `1068`, `1077` | Local mixed versions or a typed destination; no Spotify public/cover-upload controls for local data |
| Dialog::ConfirmPlaylistDuplicates, `src/model.rs:1082` | Local/overlay duplicate confirmation with entry identity, or typed target; do not submit SoundCloud rows to Spotify check |
| Dialog, `src/model.rs:1054` | SoundCloud account connection with profile URL or session-token input as supported by account.rs; no token in debug formatting or persistence outside protected store |
| RowContext / DragTrack / TrackRow position | Typed edit destination and RowOrigin for overlay/local rows; preserve shared playback contexts and duplicate identity |

Exhaustive matches include `src/ui/mod.rs:278` central routing (playlist arm
`341`), minimum-width/title handling nearby (`232`), `App::ensure_loaded:5136`,
load_more/reload/load_window, page retention/eviction (`src/app.rs:3116`),
navigation/history/open_link, UI topbar/library/sidebar matches, demo flag
decoding, and persisted page/sort keys. Search all `Page::`, `Action::` and
`Dialog::` matches after adding variants and compile all platform cfgs.

## 10. Highest-risk seams and integration checks

| Risk | Why it matters and required check |
| --- | --- |
| Spotify echo overwrites hybrid | Guard `handle_local:3162`, PlaybackState `6911`, Queue `6995`, and context/repeat adoption. Inject stale answers before and after source handoff and beyond existing hold timeouts; display must not flicker |
| Two queues advance | Hybrid Next/skip_to mutates once, then loads once. Bypass pop_queue_head, consume_manual_queue_head, Spirc Next loops, resync_local_queue and Web API polling for hybrid. Duplicate manual/context URI copies remain separate |
| No natural completion signal | EndOfTrack is discarded at `src/player.rs:1083`. Carry occurrence/load generation, reject old completions, and distinguish stop/cancel/failure/end so an A -> B -> A sequence cannot skip twice |
| Spotify continuation or repeat | The ordinary single-song helper loads a context. Use one-track explicit list, suppress child repeat/shuffle/autoplay, and guard app autoplay. Test final Spotify row, repeat-one, repeat-all and a SoundCloud failure |
| Shared audio ownership | `sink_builder`, `src/player.rs:843`, wraps RodioSink in Tapped at `880`. Same sink type is not proof of a shared physical output. Stop old decoder, clear replacement/seek buffered audio and EQ/limiter history, retain pause semantics, and keep visualisers post-EQ/pre-volume. Inspect sink.rs AudioControl and Tapped before wiring |
| Reconnect revives old Spotify song | Backend kept-player/resume logic (`src/backend.rs:2339`, `3472`; `PlaybackResume`, `src/player.rs:266`) needs owner/generation guards. Reconnect must not overwrite a SoundCloud row, and a SoundCloud stream must not require a Spotify reconnect to pause |
| Playing from / row markers | Hybrid child Spotify load reports no logical mixed context. Use plan context + row occurrence, and retain original source on sorted/filtered playback; test sidebar and queue headings |
| URI ID collisions / fake links | util::uri_kind and uri_id do not validate scheme; Page::from_uri ignores it. Cache and navigate by provider/full URI; no SoundCloud ID in Spotify Track/Artist/Album/Contains requests or fabricated Spotify links |
| Overlay index corruption | Preserve raw PlaylistPage and cache offsets; origin mapping for selection/removal/reorder/DnD. Test nulls, missing locals, duplicate Spotify tracks, window base > 0, sort/filter, delayed snapshot and anchor deletion |
| History / reporting | There is local listening history, no application scrobbler found. Do not claim SoundCloud plays are reported to Spotify. Feed only confirmed audible state, avoid counting loading time, reset same-URI occurrences, preserve source artwork and Recent behavior |
| OS media controls | `sync_media_controls`, `src/app.rs:4610`, already maps NowPlaying metadata, duration, position, volume, shuffle/repeat. Hybrid seek confirmation must call controls.seeked as `handle_local:3206` does. Test SetPosition track-ID checks (`handle_media_commands:4513`), repeated same URI, pause and hidden-window controls on each native platform |
| Sonos / Cast / Connect remote | Reject mixed owner/context before `Action::Transfer:11520`, `ActivateReceiver:11526`, and `transfer:9897`; also backend defense at activate_receiver (`src/backend.rs:4063`), remote API execution `5789`, restricted Connect `5867`. Receiver activation can log in/start Spotify (`cast::start` at `4122`), so UI-disable alone is too late. Local SetAudioDevice remains valid |
| Account / request lifetime | Keep SoundCloud and Spotify credentials independent. Profile-only read connection is not authenticated write permission. Use account/request generations, configured proxy and cancellation. Never log session-token inputs, headers, stream URLs containing tokens or authorization responses |
| Background audio and sleep | `sync_power`, `src/app.rs:12733`, currently asks playback_needs_activity about raw local librespot state at `12749`. Include audible/loading hybrid state so SoundCloud receives the same activity/sleep protection and releases it on pause/stop, without making background playback depend on a visible window |
| Restore loses context tail | Persist actual hybrid snapshot, not 30 preview rows or bare current track. Restart on a SoundCloud manual occurrence with Spotify duplicates and exact shuffle order; restore paused, then resume at saved position |

Suggested integration sequence: typed source/origin model and metadata cache;
pure hybrid snapshot adapter; shared action router and tagged engine events;
audio ownership/handoff; queue projection and stale-response guards; local
playlist/overlay pages; search/sidebar/account UI; persistence/demo regression
coverage. Keep Spotify-only baseline tests green throughout.

This research change needs documentation validation and reference checks. It
does not establish audio, API, UI or platform test coverage. Integrators must
run `CONTRIBUTING.md`'s full check matrix for source changes, add focused
regressions above, and use demo captures at narrow/normal sizes in light/dark
themes. Store review images/comparison files outside the repository.

Research validation on Windows: the dependency-source, review-evidence and
new-settings policy scripts passed. Source anchors were checked for existing files
and valid line numbers, all ten requested sections are present, code fences
balance, and the guide contains no em dashes. `cargo fmt --all --check` finds
an existing module-order difference in `src/lib.rs` (sonos/soundcloud); it was
left unchanged. `packaging/test-launchers.py` cannot pass in this environment:
Unix `true` and Ruby are missing, and its Bash calls reject Windows paths.
The Jekyll build cannot start because `bundle` is unavailable. A default
Clippy run was stopped during cold dependency compilation; no Rust build,
Clippy success, test-matrix completion or native playback validation is claimed.

## Appendix: complete lexical hit index at the research baseline

This includes comments, fixtures and device-name false positives. The production
meaning and additional named-helper paths are classified in section 3. Every
line matched by the first audit command is listed here, grouped by file.

- src/api/models.rs:331
- src/api/models.rs:667
- src/app.rs:663
- src/app.rs:665
- src/app.rs:1255
- src/app.rs:1327
- src/app.rs:1695
- src/app.rs:1698
- src/app.rs:1810
- src/app.rs:1813
- src/app.rs:1829
- src/app.rs:1832
- src/app.rs:2487
- src/app.rs:2492
- src/app.rs:2493
- src/app.rs:6612
- src/app.rs:9065
- src/app.rs:9068
- src/app.rs:10066
- src/app.rs:10435
- src/app.rs:10437
- src/app.rs:10438
- src/app.rs:10441
- src/app.rs:10449
- src/app.rs:10456
- src/app.rs:10466
- src/app.rs:10492
- src/app.rs:10672
- src/app.rs:12267
- src/app.rs:12268
- src/app.rs:12270
- src/app.rs:13372
- src/app.rs:13382
- src/app.rs:13428
- src/app.rs:17601
- src/app.rs:17604
- src/app.rs:17619
- src/app.rs:18368
- src/app.rs:19390
- src/app.rs:19396
- src/app.rs:19397
- src/app.rs:19398
- src/app.rs:19399
- src/app.rs:19403
- src/app.rs:19404
- src/app.rs:19494
- src/app.rs:27498
- src/app.rs:27506
- src/app.rs:27509
- src/app.rs:27510
- src/app.rs:27511
- src/app.rs:27514
- src/app.rs:27516
- src/app.rs:27524
- src/app.rs:27542
- src/app.rs:27547
- src/app.rs:27548
- src/app.rs:27556
- src/app.rs:27572
- src/app.rs:27576
- src/app.rs:27580
- src/app.rs:27583
- src/app.rs:27584
- src/app.rs:29042
- src/app.rs:29048
- src/app.rs:29086
- src/app.rs:29088
- src/app.rs:29527
- src/app.rs:29528
- src/backend.rs:48
- src/backend.rs:962
- src/backend.rs:970
- src/backend.rs:1368
- src/backend.rs:6856
- src/backend.rs:6859
- src/backend.rs:6860
- src/demo.rs:303
- src/demo.rs:305
- src/demo.rs:318
- src/demo.rs:322
- src/demo.rs:389
- src/demo.rs:423
- src/demo.rs:1314
- src/demo.rs:1318
- src/demo.rs:1327
- src/link.rs:254
- src/player.rs:65
- src/player.rs:66
- src/player.rs:525
- src/player.rs:1854
- src/session_reads.rs:1577
- src/session_reads.rs:1614
- src/session_reads.rs:1624
- src/session_reads.rs:2422
- src/session_reads.rs:2432
- src/session_reads.rs:2435
- src/session_reads.rs:2440
- src/session_reads.rs:2448
- src/session_reads.rs:2453
- src/session_reads.rs:2477
- src/session_reads.rs:2501
- src/session_reads.rs:2514
- src/session_reads.rs:2783
- src/settings.rs:285
- src/settings.rs:286
- src/settings.rs:491
- src/settings.rs:579
- src/settings.rs:580
- src/soundcloud/mod.rs:5
- src/soundcloud/mod.rs:104
- src/ui/collection.rs:512
- src/ui/collection.rs:604
- src/ui/collection.rs:605
- src/ui/collection.rs:632
- src/ui/collection.rs:658
- src/ui/collection.rs:1436
- src/ui/collection.rs:3151
- src/ui/collection.rs:3377
- src/ui/devices.rs:524
- src/ui/devices.rs:526
- src/ui/devices.rs:566
- src/ui/settings.rs:342
- src/ui/settings.rs:344
- src/ui/settings.rs:352
- src/ui/settings.rs:717
- src/ui/settings.rs:1005
- src/ui/widgets.rs:1316
- src/ui/widgets.rs:1318
