# Phone web app tests

Browser tests for `assets/web/` (the phone app), run by Playwright with its own
WebKit and Chromium. They never reach the network, never play real audio and
never use a real account.

```sh
cd services/friends
npm run test:phone
```

`npm test` is the service's own fast Node test run and does not touch any of
this. On a fresh checkout, install the browsers once with
`npx playwright install webkit chromium`.

## What is here

- `playwright.config.ts` — three phone projects: iPhone 13 and iPhone SE on
  WebKit, Pixel 7 on Chromium.
- `server.ts` — one local mock service per test. It serves `assets/web` with the
  same headers as `src/web.ts` and answers `/api/*`: account, posts, members,
  playlists, live presence, and the `/api/live/remote/*` pair (state, send,
  reply, phone, events as server-sent events) that a computer would normally
  answer. The seed has a 200-song mixed playlist plus home sections, liked
  songs and search results.
- `browser-fake.js` — installed with `addInitScript`. `window.Audio` becomes a
  fake element (`window.__audio.advance/fail/play/pause`), and
  `navigator.mediaSession`/`MediaMetadata` record handlers, metadata and
  position state (`window.__mediaSession`).
- `fixtures.ts` — the per-test server, the stored sign-in, fake audio, the
  navigation guard, and the teardown check that every test passes only with no
  console errors, no page errors, no unhandled rejections and no `window.open`.
- `*.spec.ts` — the tests, grouped by area.

## Driving failures

`server` is a `PhoneServer` shared by the test:

```ts
server.failNext('search', { status: 500, error: 'Search is down.' });
server.slowNext('home', 3_000);
server.setComputerOffline(true);
server.dropEventStream(1);
server.endEventStreams();
server.setComputerState({ song: server.songAt(121), playing: false });
```

A test that deliberately causes a browser-level log line (Chromium notes an
injected HTTP 500 in the console) declares it with
`observed.allowConsole(/pattern/)`. JavaScript console errors are never allowed.

## The navigation guard

Any main-frame navigation off the app's origin fails the test, as does
`window.open` or an app scheme. The one exception is a test exercising Spotify's
own sign-in: call `phone.guard.allowNext(/^https:\/\/accounts\.spotify\.com\/authorize\?/)`
first. External requests are served locally (artwork as a 1x1 PNG, Spotify's
token endpoint as JSON), so no traffic leaves the machine.
