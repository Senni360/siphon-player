---
title: Loudness Matching
description: One loudness reference for Spotify and external PCM sources.
---

The existing **Normalisation** setting controls all local sources. Off leaves
decoded levels unchanged (EQ, volume and the final peak limiter still apply).
There is no additional setting or source-specific volume slider.

Spotify keeps librespot's supplied track gain, with explicit zero dB pregain,
track mode instead of automatic album mode, and dynamic normalisation. The
pinned librespot revision `3ba977f1f918582edce566cfc16617f8eccdfb1e`
applies `10^((track_gain_db + normalisation_pregain_db) / 20)` in
[`NormalisationData::get_factor`](https://github.com/crmne/librespot/blob/3ba977f1f918582edce566cfc16617f8eccdfb1e/playback/src/player.rs).
Its [`PlayerConfig`](https://github.com/crmne/librespot/blob/3ba977f1f918582edce566cfc16617f8eccdfb1e/playback/src/config.rs)
defaults supply a -2 dBFS dynamic-limiter threshold, 5 ms attack, 100 ms release
and 5 dB knee; Siphon retains those values. No LUFS target is hardcoded in that
gain calculation: Spotify supplies the measured gain. Spotify's
[published track reference is -14 LUFS](https://support.spotify.com/us/artists/article/loudness-normalization/).
Keeping the supplied measurement avoids decoding whole Spotify songs before
playback or estimating them from a short intro. Spotify metadata is trusted;
it is not remeasured, so exact agreement with every historical master is not
guaranteed. Missing gain metadata falls back to zero dB, not a fresh LUFS
measurement. Track mode deliberately matches individual tracks across sources
rather than preserving relative album levels.

All external PCM uses the same pure Rust `ebur128` gated integrated R128
measurement at stereo 44.1 kHz, before EQ, volume or limiting. Gain is
`-14 LUFS + 0 dB pregain - measured LUFS`. This uses R128's measurement method,
not its broadcast playback target of -23 LUFS. Boost is capped at 12 dB and
attenuation at 30 dB; silence never produces a gain estimate. The existing
final limiter remains after gain and EQ and protects boosted peaks. Spotify
also has its upstream dynamic limiter, while external sources use Siphon's
existing final limiter (a -1 dBFS threshold and 2 dB knee). Their peak handling
therefore differs. Strong EQ, limiting, different masters and exceptionally quiet recordings can keep
the audible result from exactly reaching the reference.

On the first play, SoundCloud decodes up to three seconds ahead and measures
that buffered PCM before writing it. It spends up to 150 ms between packets;
a blocking network read still has the engine's normal 15-second deadline and
can be cancelled by transport commands. PCM is buffered in memory and replayed
once, without counting it twice. This can add startup latency on slow streams.
If lookahead yields too little audio, the starting gain is -14 dB. The running
integrated estimate updates once per second after three seconds of audio.
Uncached estimates cannot boost above unity in the first 30 seconds. Corrections
slew continuously at 1 dB/second upwards and 3 dB/second downwards, with a
0.5 dB deadband. This is track loudness correction, not momentary compression.
A quiet intro cannot predict a loud chorus; the slow correction and limiter
bound that limitation without trying to flatten each musical phrase.

Measurements are private readable JSON records in `state/volume/loudness/`,
named by a hash of the stable source URI. The format is versioned and stores
LUFS, measured frames and whether the decoder reached EOF. Checkpoints every
ten decoded seconds and on Stop/replacement save contiguous prefixes from
zero. Writes use `fastframe_file::Writer` for atomic replacement off the UI
thread. Invalid records are ignored. Seeks and URL refreshes cannot replace a
whole-track value with a fragment; shorter prefixes cannot overwrite longer
ones. Later plays start immediately at the cached gain. A completed measurement
stays fixed during subsequent playback so quiet and loud passages retain their
dynamics. Delete the record to remeasure a source whose content has changed.
Seeks and decoder rebuilds of the current track retain its in-memory gain and
pending checkpoint, so a delayed atomic write cannot cause a sudden return to
the conservative starting level.

SoundCloud account/permission changes and Spotify sign-out call
`loudness::prepare_invalidation(directory)` before the backend accepts new
loads. This immediately disables cache reads/writes and advances a generation,
without waiting for disk or requiring an active sink. The returned
`Invalidation::finish()` job runs on `spawn_blocking`, removes the directory
under the atomic writer's barrier, and enables only its own generation. Old
queued writes cannot recreate cleared records; an older clear cannot enable or
delete a newer account's cache. Each processor drops old identity/measurement
on its next use, retaining audible gain for a continuing decoder. Failed or
abandoned clears stay disabled even after all sinks are dropped. The backend
reports deletion errors; another disconnect retries. Initial saved-account
restoration, same-account token/name changes and audio-engine restarts retain
measurements. These cache hooks do not authorize or cancel track requests;
the backend's account generations still reject stale requests.

## External source API

Every external source, including YouTube, shares `sink::ExternalPcm` and
automatically receives streaming measurement, gain, EQ and limiting. To get
per-track cache identity, call `ExternalOutput::begin_track(uri, from_start)`
after claiming the output and on each Load, seek or decoder rebuild. Pass a
stable source URI, never a signed stream URL. Only a contiguous decode from
position zero may set `from_start = true`.

When `needs_loudness_lookahead()` is true, a producer may buffer decoded PCM,
call `prime_loudness(&pcm)`, then write that exact PCM in order. Producers
without lookahead still start conservatively and converge. Call
`finish_track(true)` only on real decoder EOF. Stop/release and replacement
save a partial measurement; reset of EQ/limiter state must not reset track
measurement on pause. These methods run on playback workers, never the UI
thread. The redesign only needs to surface the existing normalisation toggle.

## Validation limits

Synthetic tests cover R128 reference tones, silence, capped gain, per-frame
ramps, a quiet intro followed by loud audio through the final limiter,
lookahead replay, cache generations (including no active sink, failed clears,
concurrent/superseded clears, and nonblocking PCM), seek continuity and exact normalisation
bypass. Decoder fixtures check buffered PCM is emitted once. These checks do
not listen to an output device or verify Spotify's gain metadata for a live
recording. Windows validation still needs source transitions, seeking,
pause/resume, normalisation on/off, boosted EQ/limiter behavior, and audible
first-play versus cached-play comparison using a separate test profile.
