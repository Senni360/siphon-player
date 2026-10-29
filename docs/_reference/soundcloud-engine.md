---
title: SoundCloud Engine Integration
description: Resolved stream inputs, URL refresh, and exclusive use of the shared audio output in this personal fork.
nav_order: 21
---

`soundcloud::engine` accepts already resolved audio URLs. This module does
not resolve tracks, change a queue or add interface controls. Adapt the API's
`StreamInfo` to `Stream { uri, url, protocol, mime, duration_ms, is_preview }`.
`duration_ms = 0` means unknown. Preview streams are played as supplied;
there is no attempt to obtain full audio for a preview.

## Application integration

`App::dispatch_hybrid_action` owns mixed transport and queue edits, with the
same source check in `start_play` for internal callers. Views keep emitting
ordinary playback actions. Search adapters should cache complete converted
tracks through `App::cache_soundcloud_tracks` before emitting `PlayUris` or
`PlayFromRow`. Use `OpenLink` for a SoundCloud permalink or short link.
Backend search adapters clone `Worker::soundcloud_client()` to share playback's
client ID, cooldown and optional session. Persist discoveries with
`backend::hybrid::persist_client_id` after a request, off the UI thread.
Local playlist changes keep using `save_playlist_sources`, including its
unreadable-file safeguards.

`Backend::HybridCommand` performs child loads, transport, stream resolution
and lookahead. A hybrid Spotify child is an explicit one-track load with
shuffle, repeat and autoplay disabled. Librespot and SoundCloud completions
carry both a load generation and a queue occurrence ID; only the current
pair can advance the authoritative queue. Spotify playback and queue reads
cannot overwrite it. Source switches acknowledge the old decoder stop or
SoundCloud's `OutputReleased` before loading the replacement.

The pinned Spirc fork resolves autoplay even for an explicit track list when
the Spotify session enables it. For hybrid loads the adapter inhibits Spotify
output first and detaches Connect only when its cluster marks this computer
active. It never activates an idle device just to disconnect it. Disconnect
clears Spirc's contexts; its network-dependent acknowledgement is best effort,
bounded to 500 ms. A missing acknowledgement does not abort playback while
output is inhibited. The decoder's separate ordered Stop barrier is required
before loading either source.
It validates the one-item list and loads its sole item directly on the
librespot decoder. Hybrid pause, play and seek also address that decoder.
The pinned Spirc also consumes EOF while inactive and sends a decoder Stop
when its cleared queue has no successor. The tagged observer holds a natural
completion until the matching track and play-request ID report Stopped. This
acknowledges Spirc's Stop before the application can load the next row. Old
requests, other tracks and duplicate EOF events cannot advance the queue.
Explicit handoffs cancel the observer before sending their own Stop.
Returning to Spotify-only playback uses the existing Spirc load path again.
Deferred Spotify-only commands carry an ownership generation: a newer Claim,
Load, Stop or Release discards the older handoff's commands.

Both sources need the local engine's shared audio output. If it is unavailable,
the app opens the device menu and points to **Set up playback here** or
**Try again**. A mixed queue keeps the chosen row paused rather than skipping
every row; press Play after setup to retry it. Normal Spotify playlists and
tracks continue through Connect, and an explicit Spotify load removes output
inhibition after a SoundCloud stop or a failed hybrid start.

Playback logs report routing, local engine readiness, handoffs, load generations
and resolved stream protocol/codec. They omit client IDs, account tokens,
track authorization values and signed stream URLs.

Mixed playback stays on this computer, including Spotify rows. A remote
transfer or receiver activation is refused in both the application and backend.
Adding SoundCloud to an existing local Spotify queue promotes it only when
the complete context and current occurrence are known. A remote queue, an
unknown tail or an ambiguous repeated occurrence is refused with an
explanation. Starting a local mixed playlist always establishes its own order.

The shared player bar, queue panel, media controls, local listening history
and sleep protection read the hybrid projection. The complete queue snapshot
is saved with the session and restores paused without contacting an engine.
If a context has not resolved yet, its pending request is saved separately;
Play resolves its full order instead of resuming a truncated preview.
SoundCloud account restoration is independent of Spotify and waits for the
configured proxy. `App::soundcloud_account` holds the restored account;
after a verified account change, send `Command::SoundCloudAccount` to update
the backend client and persist only the non-secret account state. Stream and
context resolutions capture a backend account generation. Changing accounts
rejects their old replies, clears the stream cache and cancels both active and
paused lookahead without interrupting the current decoder.

## Output ownership

Keep the Spotify `player::Engine` alive. Pause its transport and prevent
Connect from reactivating it, then obtain `player::Engine::external_output()`
and construct the SoundCloud engine with the backend runtime, the shared
`Http`, that output and a `Notify` callback. The output is available once
librespot constructs its sink. Construction never opens another audio device.

`Load` claims the existing RodioSink. Each external handle has its own claim;
a second owner is rejected. Spotify packets, start/stop calls, explicit audio
interrupts and late player events are gated while that claim is held. Claim,
release, writes and event handling are serialized, so an in-flight Spotify
write cannot mix with external PCM. This protects audio exclusivity; the
integrator must still suppress conflicting Connect and queue state updates.

A separate Spotify inhibition gate survives external release, including the
final SoundCloud row's Stop. Connect replacements are stopped and their packets
remain silent until an explicit validated Spotify load takes ownership. Gate
changes and rejected decoder commands are serialized, so an older rejection
cannot enqueue Stop after the replacement load.

Pause and end-of-track retain external ownership. To return to Spotify, send `Stop`
and wait for `Event::OutputReleased` before loading or resuming Spotify.
An end-of-track `Stopped` state alone does not release
external ownership. OutputReleased alone does not remove Spotify inhibition.
Dropping the engine requests asynchronous shutdown; use the
explicit Stop acknowledgement for a coordinated handoff.
Keep the Tokio runtime alive until that acknowledgement as well.

Both paths feed stereo 44.1 kHz PCM into the same RodioSink, rodio mixer and
fastframe-audio device. External samples pass through their own EQ filter
history, then the shared visualizer tap, then the limiter and existing output
volume. The tap is post-EQ and pre-volume for every visualizer, including at
zero volume. Source-rate conversion preserves state across packets and HLS
segments; the sink performs its existing conversion to the device rate.
Device selection, reopening, buffering and Spotify's processing stay intact.
Route volume through the existing Spotify mixer commands for either source.

## Commands and events

`Engine::command` queues `Load`, `Play`, `Pause`, `Toggle`, `Seek`, `Stop`,
`Preload`, `Refresh` or `Shutdown`. It returns false after the worker exits.
It never waits for HTTP, decoding or device work. `state()` clones the latest
snapshot. `Notify` runs on the feeder thread; forward events to the backend
or UI channel and return promptly. Do not perform blocking work in it.

`Event::State` reuses `player::LocalState`. Playback, loading, track URI and
duration, position anchors, errors, track sequence and seek sequence describe
this engine. Spotify remains the authority for volume, shuffle, repeat,
account and Connect fields. Signed URLs never enter snapshots or Debug output.

`Preload(next)` prepares a bounded decoder without advancing the queue.
Load the identical stream at zero to reuse a completed preload. At EOF with
a prepared successor, `EndOfTrack` is sent while the short output tail still
plays. Respond with that Load and autoplay to append the successor without
pausing or clearing the output. If no handoff arrives before the tail drains,
the device pauses. Other loads and seeks discard the old output. Pause cancels
preload work and HLS lookahead. Preload intent is retained, and requests made
while paused are deferred until Play. Explicit paused loads/seeks may
prepare their decoder, but a paused engine has no background decoding,
network lookahead, polling or device callbacks.

HTTP 403 or 404 on any range, playlist, initialization object or segment emits
`RefreshRequired { request_id, uri, position_ms }`. Resolve a fresh stream and
reply with `Refresh { request_id, stream }`. Matching replies resume at the
retained position and preserve play/pause intent. Replies for older requests
or another URI are ignored. Stop or a new Load invalidates the request.
Other failures emit `Error` and pause with a readable error in state.

## Network and format limits

All requests use the app's configured HTTP client and proxy on its Tokio
runtime, with a 15-second request deadline. A separate feeder performs
blocking demuxing and decoding; it never runs on the UI thread. Transport
commands cancel outstanding HTTP requests. Progressive audio uses 32 KiB
byte ranges rather than downloading a whole track. HLS follows up to four
master/media playlist levels and fetches one compressed segment ahead, with
a 16 MiB limit per object. Nothing is written to disk.

Symphonia decodes MP3, AAC ADTS and AAC fragmented MP4. HLS seeking starts at
the containing segment and discards decoded frames up to the requested
sample. Opus has no decoder in the existing dependency stack and returns
`Unsupported`; choose an MP3 or AAC transcoding. Encrypted HLS, MPEG-TS,
byte-range HLS and live playlists are unsupported. No DRM support is added.

Synthetic ffmpeg fixtures and a loopback HTTP server test transport,
resampling, segment seeking, EOF, preload handoff and URL refresh without
an account or audio hardware. Sink tests cover exclusivity and the tap's EQ
and volume ordering. Native device playback still needs manual validation.
