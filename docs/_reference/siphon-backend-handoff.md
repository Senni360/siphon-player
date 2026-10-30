---
title: Siphon Backend Cloud Handoff
description: Delivered branches, validation evidence and the remaining local integration work.
---

# Backend cloud handoff, 30 October 2026

The private backend candidate is `siphon/backend-delivery`. Its tested code
commit is `8fe5edb3c4198f490fe22d1e93c2991ab06bf24a`; this handoff is a subsequent
documentation-only commit. Base `main` was
`ac4bd07ef371477fc90aa34d30a09546c844de73` and remains unchanged.

All five existing WIP snapshots were preserved and extended in isolated
worktrees. The delivery branch combines them with linear cherry-picks and
integration fixes. Integrate this candidate once, or select the feature branches;
do not apply both copies of the same work. Nothing was force-pushed, deployed,
tagged or released.

| Feature branch | Preserved WIP | Delivered head |
| --- | --- | --- |
| `siphon/loudness` | `633fb4fead54e192e27df6fcf3ce7c71d3d63437` | `48a7c7d7332eecd3b9edd89d569f1f119fe82b52` |
| `siphon/soundcloud` | `19f9388c50219dfee2b88be4a5e4c923229fe539` | `23fb39781f55280941d87f247f624b136db34e71` |
| `siphon/friends-feed` | `cb1a60945ba3c7dd37e406c10dcb54c73f9dcd4a` | `3a83f3b0b51c0c34ed017c19790bb301a1f5da16` |
| `siphon/credits-isrc` | `aff3562a998dd6acad5fd6dddba81936d544b9b2` | `a87629aae0ad83b0c8cb13184de29f012fb86667` |
| `siphon/youtube` | `9ca81051d6f7332a449d608e8a092c1e33d0fc60` | `c70bfa962513187a7f51e3646850a6f8fbcfedda` |

## Delivered behavior

- **Loudness:** retains the existing R128 implementation, verifies the pinned
  librespot configuration, shares external gain between SoundCloud and YouTube,
  preserves gain through seeks/refresh, and invalidates account-private
  measurements before new-account loads. EQ and limiting remain active when
  source normalization is off. Cache invalidation is tested without an active
  sink and against delayed/superseded disk work.
- **SoundCloud:** track/set repost state, session-only writes, following feed,
  pagination and stale-response overlays; source metadata/ISRC preservation;
  account cache isolation; private/short links retained in snapshots and used
  to restore stream access on a fresh client. The current public client assets
  confirm endpoint methods and repost pagination.
- **Friends:** TypeScript service and Rust client with invite enrollment,
  hashed bearer secrets, private group shares, pagination, hearts, own-post
  deletion, rename, rate limits and membership revocation. Leave group revokes
  remotely; Forget this device works offline. Redis CAS retries recheck
  authorization. Uncertain remote leave retains the credential for explicit
  retry. No production endpoint is configured.
- **Credits/lyrics:** verified Spotify operation/hash, source-based credits,
  exact ISRC matching within 3 seconds, strict external LRCLIB matching within
  2 seconds, cancellable per-track reads and stale-response protection.
  Session-owned matching replaces the old disk cache, which startup retires.
  Unsupported YouTube credits remain unavailable.
- **YouTube:** canonical links/CLI/playlist additions, bounded cancellable
  helper-managed yt-dlp adapter, metadata caching, shared external PCM,
  preload/URL refresh, account-scoped local likes and direct-source download
  provenance. There is no video UI or substitution for Spotify playback.

The source modules and exact frontend bindings are documented in
[the integration contract](siphon-integration-contract.md). Native fixture data
in `src/demo/siphon.rs` covers ready, empty, loading, error, unavailable and long
titles. These are explicitly invented, offline fixtures, not API responses.

## Shared files and integration seams

Feature modules contain most of the work. Shared changes are concentrated in
`src/app.rs`, `src/backend.rs`, `src/model.rs`, `src/api/models.rs`,
`src/backend/hybrid.rs`, `src/app/soundcloud_library.rs` and `src/ui/widgets.rs`.
`Cargo.toml`/`Cargo.lock` retain the loudness WIP's `ebur128` addition.

Track publisher metadata and `linked_from` use optional boxes to limit row
size; their JSON shapes remain unchanged. Following-content variants are boxed.
Lyrics requests/results now carry a request ID. Friends has per-request and
account generations. YouTube metadata and likes events include generation and
account ownership. Source share URLs still occupy the legacy
`external_urls.spotify` field; do not treat that field name as source identity.

The delivery branch retains both track-details and loudness invalidation hooks
at account transitions, plus YouTube cancellation on shutdown. Preserve all of
them when resolving local cleanup/helper/frontend conflicts. The cache memory
budget includes the new metadata allocations.

Minimal UI wiring adds YouTube Likes to the existing sidebar, existing-source
hearts/reposts/sharing actions, the friends panel and the existing lyrics control.
No visual direction was chosen. Claude still owns all mockups and the approved
native redesign. The local cleanup/helper work was not available here and was
not duplicated. Consequently this candidate still contains the old cleanup
targets such as visualizers, translations and platform packaging.

## Validation

Environment: Debian 13 Linux, repository-pinned Rust/Cargo 1.98.0, native audio,
windowing, CMake/Clang and OpenGL dependencies. Heavy builds ran serially with
two compile jobs, separate worktree targets, and uniform debug-info settings.
The complete application was validated on the combined delivery branch;
individual feature branches were not each subjected to duplicate full builds.
The old default MilkDrop/projectM feature compiled successfully in this cloud
environment. This is not only a `--no-default-features` result.

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` and `git diff --check` | Passed |
| Dependency-source, review-evidence and new-settings checks against `origin/main` | Passed |
| `.github/scripts/test-policy-checks.py` | 27 passed |
| `packaging/test-launchers.py` | 4 passed |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed |
| `cargo clippy --locked --all-targets --all-features -- -D warnings` | Passed |
| `cargo test --locked --all-targets` | 1,827 library tests passed, 6 ignored; binary/integration/example targets passed |
| `cargo test --locked --all-targets --all-features` | 1,869 library tests passed, 6 ignored; binary/integration/example targets passed |
| `cargo test --locked --all-features --doc` | Passed; no doctests defined |
| `RUSTDOCFLAGS='-D warnings' cargo doc --locked --all-features --no-deps` | Passed |
| Friends `npm ci`, `npm test`, `npm run check` | Passed; 12 tests |
| `cargo test --locked --all-features friends::tests::local_service_round_trip -- --ignored --exact` | 1 passed against a fresh local Node service; no app profile or real credentials |
| `cargo build --locked --features demo` | Passed |
| Native `--demo-shot` with isolated `--demo-data` | Rendered 1280x800 dark and 820x760 light/long-title states under Xvfb/software OpenGL; logs contained no warning, error or panic |

The local Rust-to-Node round trip ran at `f3869bd`; the later source change is
limited to SoundCloud private-link restoration. Friends source did not change.
The final full Rust suites include that SoundCloud correction. Initial WIP
compile errors and one demo lyrics regression were fixed before these passes;
the failed runs are not represented as successes.

The six ordinary-suite ignores cover desktop-portal access, native keyring,
local friends service, two live anonymous SoundCloud tests and an X11 window
state test. Only the friends-service test was separately run. Native screenshot
smoke coverage does not imply the ignored X11 state test passed.

Not run: Nix packaging (Nix unavailable), Jekyll site generation (Bundler
unavailable), Windows builds, native keyring round trips, real output-device
audio, authenticated Spotify/SoundCloud requests, live YouTube extraction, and
hosted Vercel/Upstash persistence. Public provider-bundle inspection is live
evidence of the cited client contracts; API behavior otherwise uses fixtures.
Redis REST tests use fixtures, not a live Upstash account.

## Next owner actions

The local OpenAI coordinator should fetch these private branches and reconcile
its newer cleanup/helper work before the final Windows build. Do not replace
the installed helper or overwrite uncommitted local work. Keep its constrained
build gate and separate demo profile. Native output-device checks still need
seek/pause/next, source transitions, normalization off and limiter behavior.

Verify the actual helper's discovery, hidden windows, process-tree cancellation,
yt-dlp update locks and configured proxy behavior. The direct yt-dlp adapter does
not yet translate the app's custom proxy settings; see [YouTube](youtube.md).
Check shared private/unlisted links, saved queue restoration and second-launch
handoff with real accounts locally. None of those claims can be established by
the Linux fixtures alone.

Claude should consume the integration contract and fixtures on this branch,
produce the three app mockups, then implement the owner's selected design.
Analysis/BPM/key/stems actions remain with the local helper integration; this
delivery does not invent their protocol or success states.

The friends service is prepared for a later reviewed deployment. Exact routes,
limits and environment variables are in
[`services/friends/README.md`](../../services/friends/README.md). The service needs
`SIPHON_FRIENDS_INVITE_CODE` and one matching Redis pair:
`KV_REST_API_URL`/`KV_REST_API_TOKEN` or
`UPSTASH_REDIS_REST_URL`/`UPSTASH_REDIS_REST_TOKEN`.
The player receives the real `/api` base via `SIPHON_FRIENDS_URL`. No cloud
resources or credentials were created or guessed here.

Other chats have not been contacted automatically. Relay this handoff and the
integration-contract link to Claude and the local coordinator.
