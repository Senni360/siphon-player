# Encore agent guide

Encore (Siphon: Encore) is a private Windows x64 music app for the owner and a
few friends. `PRODUCT.md` says what it is for and `DESIGN.md` how it looks. Keep
process light: ship what works and add a rule only after something has gone
wrong without it.

## Limits

- Never log or publish credentials, authorization responses, the signing key,
  the group invite code, or private SoundCloud and unlisted YouTube links.
- Do not circumvent DRM. Spotify songs play from Spotify, through librespot or
  the optional Soloist player, never from a substitute source. The one exception,
  the owner's call on 2026-11-05: the phone web app, which can't play Spotify,
  plays a Spotify song as the same song matched on YouTube or SoundCloud (the
  download matcher's strict rules), and the Spotify app only when there's no match.
- Each listener uses their own Spotify developer app. Never use someone else's
  Client ID.
- Windows x64 is the only target. Other platforms' code is unverified, so never
  claim it compiles.

## Code

- `src/ui/` draws views and emits `Action`s. Apply actions after drawing in
  `src/app.rs`; do not mutate application state from inside a borrowed view.
- Network and playback work goes on the runtime in `src/backend.rs` or in the
  player engine in `src/player.rs`, never as blocking work on the UI thread.
- File writes that wait for the disk (anything through `fastframe_file`) never
  run on the UI thread: encode there and hand the bytes to the app's
  `fastframe_file::Writer`. Its guard panics in debug builds.
- Settings and state files stay readable, backward compatible and atomically
  written, so an update never loses anyone's data.
- egui and winit come from pinned forks. Move all their crates to a new revision
  together. On Wayland, eframe paces frames itself, so add no vsync decision.
- The egui fork shapes right-to-left runs but leaves them in logical order. Pass
  logical text to `crate::bidi`; never reorder the string before layout.
- The interface is optimistic. A control shows its result the moment it is used,
  and the backend makes it true behind it. A late answer from before the user's
  action is stale: hold the shown state and ask again, so nothing the user did
  flickers away and comes back.

## Read before changing

- Authentication, Spotify requests, Connect, credentials or network behaviour:
  `docs/_reference/how-it-connects.md`. Keep it true when network behaviour
  changes.
- The queue: `docs/_reference/queue.md`. Its rules are the contract, and the
  queue tests in `src/app.rs` enforce them.
- A new Spotify-facing feature: `docs/_reference/what-spotify-allows.md`, which
  lists what is possible and why the rest is not.
- A state machine or API fallback: the nearby module tests.

## Working

- Work on `main`, one topic per commit. There is no CI, so before pushing code
  run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings` and
  `cargo test --all-targets`, adding `-j 2` to the last two if linking runs out
  of memory (`os error 1455`). Add a test when behaviour could quietly regress.
- Pushing to `main` redeploys the group service in `services/friends` on Vercel.
  Run its `npm test` when it changes.
- Discord loads the card's plain badge from `packaging/icons` on `main`, so keep
  those files at that path.

## Releasing

Write `packaging/release-notes/vVERSION.md`, commit everything else, then run:

```sh
packaging/release.sh VERSION
```

It sets the version, builds, zips, signs, pushes and publishes the GitHub
release. A version with a hyphen is a prerelease. Copies running a prerelease
update to the next prerelease by themselves, and stable copies only to stable.

The updater depends on the zip's `siphon-vVERSION-x86_64-pc-windows-msvc/` top
folder, `siphon-portable.txt` beside `siphon.exe`, `checksums.txt` signed with
the key in `~/.siphon-release/`, and `siphon.exe --version` printing
`siphon VERSION`. Change those only together with the updater.
