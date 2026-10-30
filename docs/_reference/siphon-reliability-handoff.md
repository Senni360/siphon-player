---
title: Siphon Reliability Handoff
description: Reproduced failures, fixes, performance evidence and delivery limits.
nav_exclude: true
---

# Reliability pass, 30 October 2026

`siphon/reliability-delivery` descends from the previous combined backend delivery
`8b94e5b`. Its complete tested code snapshot is `f908676`; subsequent changes at
the time of this report are documentation only. Apply the combined branch once,
not both it and the individual copies of its commits.

The owner requested a substantial parallel reliability/performance pass and a
combined runnable Windows application by 22:00–22:30 Europe/Amsterdam. Six
isolated workers investigated queues, streams, persistence, performance, friends
concurrency and independent review. Heavy Rust checks ran serially on the
coordinator's checkout. No extra AI provider, API key or real account credential
was used. The offered model called "Jeff" remains unidentified.

## Corrected behavior

- **YouTube actually enters the shared decoder.** Backend routing accepted
  YouTube, but the decoder's Load guard still accepted only SoundCloud URIs.
  A regression now decodes the same real fragmented-MP4 AAC fixture through both
  canonical identities and compares non-silent PCM.
- **Late seek events cannot skip a queue occurrence.** Settled decoders require
  the numbered seek acknowledgment. Initial loads may absorb a seek without a
  separate acknowledgment; reopening a decoder during a later seek cannot
  restore that exception. Tests cover two rapid seeks, nearby old snapshots,
  paused playback, repeated URIs and delayed EOF.
- **Superseded requests stop doing work.** SoundCloud foreground, preload and
  context tasks now have cancellation ownership. Stalled loopback requests prove
  sockets close when replaced. Audio Stop preserves an unresolved desired queue
  context; Release/account changes/shutdown cancel it. This distinction avoids
  leaving Next/Previous stuck while the original context is loading.
- **Broken HTTP responses cannot masquerade as the end of a song.** Ranges,
  lengths, bodies and 416 EOF evidence are validated. Buffered reads honor
  cancellation; overflowing offsets fail explicitly. An expired HLS init cancels
  a stalled media sibling. Seeking after EOF retains the selected position and
  correct loudness-measurement identity when Play resumes.
- **Local likes survive delayed writes and report failed saves.** Restore waits
  off-thread for the same atomic writer used by saves. Stale account/generation/
  revision callbacks are ignored. Active-account unsaved edits stay visible and
  retry on reopening Likes. After leaving and returning to an account, restore
  shows the last durable file with an explicit warning if a save failed. Unsaved
  edits are not promised to survive process termination. Partial, corrupt and
  oversized files are retained rather than overwritten as an empty library.
- **Private SoundCloud set members retain access on restart.** Existing album
  URL metadata preserves the original set link. A fresh client checks the set
  identity and member before learning access again. The fixture serializes a
  complete mixed-playlist Entry, then loads and refreshes it with a fresh client.
  No learned standalone token is persisted. Older rows that already lost their
  original set link need reimporting.
- **Friends concurrent writes no longer collide in lockstep.** The old immediate
  CAS retries rejected 28 of 48 simultaneous posts. Bounded jittered retries
  preserve all posts; ambiguous transport failures are never replayed. Requests
  propagate cancellation, Redis responses and documents are bounded, stored
  rows are validated, and feed responses explicitly select public fields.
  Authorization is still checked inside every attempted commit.
- **Malformed lyrics and metadata stay bounded.** Overflowing lyric timestamps
  are ignored without losing valid lines. Metadata reads cap actual bytes from
  the opened file, including growth after its initial size check.
- **Private builds do not offer upstream replacements.** The update repository
  and release-note identity now name `Senni360/siphon-player`. Signature
  verification remains mandatory; no private release channel or key was invented.
  See [private updates](private-updates.md).

## Measured large-library work

Liked Songs previously scanned every row separately for each retained optimistic
edit and shifted the array for each removal. Temporary borrowed indexes and a
stable filtering pass preserve ordering, duplicate behavior and count semantics.
The index stores only changed URIs, not an extra permanent library copy.

Identical deterministic workload, same Linux container, pinned Rust 1.98.0,
all-feature development test profile, median of three samples:

| Rows / retained edits | Rebuild before / after | Refresh before / after |
| --- | --- | --- |
| 10,000 / 0 | 5.410 / 6.015 ms | 4.199 / 4.785 ms |
| 10,000 / 100 | 20.589 / 14.200 ms | 12.906 / 9.143 ms |
| 50,000 / 1,000 | 768.609 / 60.825 ms | 472.196 / 48.182 ms |

The largest workload improved about 12.6× and 9.8× respectively. These are
synthetic development-build measurements, not Windows release/startup claims;
small no-edit timing differences are ordinary measurement noise. Timing is
reported, not asserted in a flaky test. Run explicitly with:

```sh
cargo test --locked --all-features liked::performance_tests::large_library_optimistic_edits -- --ignored --exact --nocapture
```

## Validation of the backend snapshot

- `cargo test --locked --all-targets --all-features`: **1,899 library tests
  passed, 7 ignored**, plus binary/integration/example targets passed.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- Rust `friends::tests::local_service_round_trip`: separately passed against a
  fresh local Node service with a dummy invite. The service was stopped afterward.
- Friends `npm test`: **25 passed**; `npm run check`: passed. Ten further bounded
  burst trials each preserved 48 posts and 18 heart operations across six service
  instances. These are local fixtures, not hosted Redis/Vercel measurements.
- Six fixed queue seeds exercise **1,152 app actions** against an independent
  occurrence/history ledger, including every directed source handoff, duplicate
  manual rows, shuffle/repeat, persistence roundtrips and stale engine events.
- Formatting, whitespace and repository policy checks passed on the scoped
  changes. Independent review found and corrected the Stop/context interaction.
  Integration fixture failures were corrected before the final green full run.

The seven ordinary-suite ignores include the new explicit timing workload and
the six existing environment/live-service checks. Only the timing workload and
local friends round trip were separately run in this pass. Windows native tests
run in the candidate workflow; do not treat this Linux result as a Windows pass.
Final combined-candidate checks are recorded separately after incorporating
cleanup/frontend changes, which remove old visualizer code and change totals.

## Delivery and remaining boundaries

See [evening delivery](siphon-evening-delivery.md) for the private executable
workflow, shared PR coordination and final integration ownership. An early
Windows build does not establish that the final frontend/helper work is included.
The artifact manifest is the source of truth for its commit.

Claude still owns the approved UI. The local agent must verify the installed
helper, actual audio device, native launch and real-account paths. No production
Friends URL or Redis credentials are configured here, and the service has not
been deployed. The new 8 MiB stored-document cap needs a migration check before
adopting any existing group; the service README documents that boundary.

Private-set member restoration does not silently grant a whole set to a Friends
recipient. Track sharing uses its own link; if access exists only through the
set, explicitly share the original set link when that is intended. See
[SoundCloud social behavior](soundcloud-social.md). No Windows/helper or hosted
service validation is claimed without actual evidence.
