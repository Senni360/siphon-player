import assert from "node:assert/strict";
import test from "node:test";
import { mintToken } from "../src/tokens.mjs";
import { SECRET, request, startEdge } from "./helpers/context.mjs";
import { fakeFetch, isSpotifyToken, reply } from "./helpers/http.mjs";

const KEY = "44".repeat(32);
const SUB = "library-user";
const CLIENT_ID = "c".repeat(32);
const REFRESH = "library-refresh-token-0001";
const SC_CLIENT_ID = "abcdefghijklmnopqrstuvwxyz012345";
const ACCESS = "library-access-token-memory";
const HOME_HTML = '<script src="https://a-v2.sndcdn.com/assets/app.js"></script>';

function phoneToken(sub = SUB) {
  return mintToken({ sub, role: "phone", scope: "sync", ttlSeconds: 120 }, SECRET);
}

function computerToken(sub = SUB) {
  return mintToken({ sub, role: "computer", scope: "sync", ttlSeconds: 120 }, SECRET);
}

function track(id, overrides = {}) {
  return {
    id,
    uri: `spotify:track:${id}`,
    name: `Track ${id}`,
    artists: [{ id: "ar1", name: "Artist One" }],
    album: { id: "al1", name: "Album One", images: [{ url: `https://img.example/${id}.jpg` }] },
    duration_ms: 123_000,
    ...overrides,
  };
}

function album(id, overrides = {}) {
  return {
    id,
    uri: `spotify:album:${id}`,
    name: `Album ${id}`,
    images: [{ url: `https://img.example/${id}.jpg` }],
    artists: [{ id: "ar1", name: "Artist One" }],
    release_date: "2026-01-01",
    total_tracks: 10,
    album_type: "album",
    ...overrides,
  };
}

function artist(id, overrides = {}) {
  return {
    id,
    uri: `spotify:artist:${id}`,
    name: `Artist ${id}`,
    images: [{ url: `https://img.example/${id}.jpg` }],
    genres: ["pop"],
    followers: { total: 1000 },
    ...overrides,
  };
}

function playlist(id, overrides = {}) {
  return {
    id,
    uri: `spotify:playlist:${id}`,
    name: `Playlist ${id}`,
    images: [{ url: `https://img.example/${id}.jpg` }],
    owner: { id: "owner", display_name: "Owner" },
    tracks: { total: 2 },
    collaborative: false,
    ...overrides,
  };
}

function soundcloudTrack(id, overrides = {}) {
  return {
    id,
    title: `SoundCloud ${id}`,
    duration: 200_000,
    artwork_url: "https://i1.sndcdn.com/art.jpg",
    permalink_url: `https://soundcloud.com/artist/song-${id}`,
    user: { id: 9, username: "SC Artist" },
    ...overrides,
  };
}

/** A fetch handler for both providers, with every route used by the tests. */
function providerHandler(overrides = {}) {
  return async (call) => {
    if (isSpotifyToken(call)) {
      return overrides.token ? overrides.token(call) : reply(200, { access_token: ACCESS, expires_in: 3600 });
    }
    if (call.url === "https://soundcloud.com/") {
      return overrides.soundcloudHome ? overrides.soundcloudHome(call) : reply(200, HOME_HTML);
    }
    if (call.path.endsWith(".js")) {
      return reply(200, `window.__sc={client_id:"${SC_CLIENT_ID}"};`);
    }
    const key = `${call.method} ${call.path}`;
    if (overrides[key]) return overrides[key](call);
    return reply(404, {});
  };
}

async function startLibrary(t, { env = {}, overrides = {}, linkSpotify = true } = {}) {
  const fetchImpl = fakeFetch(providerHandler(overrides));
  const ctx = await startEdge({ env: { EDGE_VAULT_KEY: KEY, ...env }, fetch: fetchImpl });
  t.after(() => ctx.close());
  if (linkSpotify) {
    const saved = await request(ctx.base, "/v1/vault/spotify", {
      method: "PUT",
      token: computerToken(),
      body: { clientId: CLIENT_ID, refreshToken: REFRESH },
    });
    assert.equal(saved.status, 200);
  }
  return { ctx, fetchImpl };
}

test("library routes need a phone sync token and the vault key", async (t) => {
  const { ctx } = await startLibrary(t, {
    overrides: {
      "GET /v1/me/player/recently-played": () => reply(200, { items: [] }),
      "GET /v1/me/following": () => reply(200, {
        artists: { items: [], next: null, cursors: { after: null } },
      }),
    },
  });
  const media = mintToken({ sub: SUB, role: "phone", scope: "media", ttlSeconds: 120 }, SECRET);
  const live = mintToken({ sub: SUB, role: "phone", scope: "live", ttlSeconds: 120 }, SECRET);

  assert.equal((await request(ctx.base, "/v1/library/albums")).status, 401);
  assert.equal((await request(ctx.base, "/v1/library/albums", { token: media })).status, 401);
  assert.equal((await request(ctx.base, "/v1/library/albums", { token: live })).status, 401);
  assert.equal((await request(ctx.base, "/v1/library/albums", { token: computerToken() })).status, 401);
  assert.equal((await request(ctx.base, "/v1/home", { token: phoneToken() })).status, 200);
});

test("playlists paginate and append mixed playlists from the sync snapshot", async (t) => {
  const { ctx } = await startLibrary(t, {
    overrides: {
      "GET /v1/me/playlists": (call) => {
        const offset = new URL(call.url).searchParams.get("offset");
        if (offset === "0") {
          return reply(200, {
            items: [playlist("p1"), playlist("p2")],
            next: "https://api.spotify.com/v1/me/playlists?offset=2&limit=2",
          });
        }
        return reply(200, { items: [playlist("p3")], next: null });
      },
    },
  });

  const snapshot = {
    playlists: [
      { id: "sp1", name: "Spotify only", spotify: true, tracks: ["spotify:track:a"] },
      { id: "m1", name: "Local mix", mixed: true, tracks: ["youtube:video:abcdefghijk"] },
      { id: "m2", name: "Other", spotify: false, tracks: [{ uri: "soundcloud:artist/song" }] },
    ],
    mixed: [{ id: "m3", name: "Desktop library shape", count: 4 }],
  };
  const put = await request(ctx.base, "/v1/sync/playlists", {
    method: "PUT",
    token: phoneToken(),
    body: { data: snapshot },
  });
  assert.equal(put.status, 200);

  const first = await request(ctx.base, "/v1/library/playlists?limit=2", { token: phoneToken() });
  assert.equal(first.status, 200);
  const page = await first.json();
  assert.deepEqual(page.items.map((item) => item.id), ["p1", "p2", "m3", "m1", "m2"]);
  assert.deepEqual(page.items.slice(2).map((item) => item.source), ["mixed", "mixed", "mixed"]);
  assert.equal(page.items.find((item) => item.id === "m3").track_count, 4);
  assert.ok(page.next);
  assert.equal(JSON.stringify(page).includes(REFRESH), false);
  assert.equal(JSON.stringify(page).includes(CLIENT_ID), false);

  const second = await request(ctx.base, `/v1/library/playlists?limit=2&cursor=${encodeURIComponent(page.next)}`, {
    token: phoneToken(),
  });
  assert.equal(second.status, 200);
  const nextPage = await second.json();
  assert.deepEqual(nextPage.items.map((item) => item.id), ["p3"]);
  assert.equal(nextPage.next, null);
});

test("a mixed playlist carries a local file's upload key and its details", async (t) => {
  const { ctx } = await startLibrary(t, { linkSpotify: false });
  await request(ctx.base, "/v1/sync/playlists", {
    method: "PUT",
    token: phoneToken(),
    body: {
      data: {
        mixed: [{
          id: "m1local",
          uri: "spotifast:playlist:m1local",
          name: "Locals",
          image: null,
          tracks: [
            {
              id: "e1",
              upload: "local-abc",
              track: {
                uri: "spotify:local:Artist:Album:Song:180",
                name: "Song",
                artists: [{ name: "Artist" }],
                duration_ms: 180_000,
                album: { images: [{ url: "https://img.example/local.jpg" }] },
              },
            },
            { id: "e2", track: { uri: "soundcloud:artist/song", name: "Cloud" } },
            { id: "e3", track: { uri: "soundcloud:track:7", name: "Public", link: "https://soundcloud.com/artist/public-song" } },
            { id: "e4", track: { uri: "soundcloud:track:8", name: "Private", link: "https://soundcloud.com/artist/private-song/s-AbCdEf" } },
            { id: "e5", track: { uri: "soundcloud:track:9", name: "Query", link: "https://soundcloud.com/artist/song?secret_token=s-x" } },
          ],
        }],
      },
    },
  });
  const detail = await (await request(ctx.base, "/v1/library/playlists/m1local", { token: phoneToken() })).json();
  assert.equal(detail.playlist.name, "Locals");
  assert.equal(detail.items.length, 5);
  // Only a song's own public page travels on to the phone.
  assert.equal(detail.items[2].link, "https://soundcloud.com/artist/public-song");
  assert.equal(detail.items[3].link, undefined);
  assert.equal(detail.items[4].link, undefined);
  assert.equal(detail.items[0].upload, "local-abc");
  assert.equal(detail.items[0].name, "Song");
  assert.deepEqual(detail.items[0].artists, [{ id: null, name: "Artist" }]);
  assert.equal(detail.items[0].image, "https://img.example/local.jpg");
  assert.equal(detail.items[0].duration_ms, 180_000);
  assert.equal(detail.items[0].source, "mixed");
  assert.equal(detail.items[1].upload, undefined);
});

test("playlists still list mixed entries when Spotify is not linked", async (t) => {
  const { ctx } = await startLibrary(t, { linkSpotify: false });
  await request(ctx.base, "/v1/sync/playlists", {
    method: "PUT",
    token: phoneToken(),
    body: { data: { playlists: [{ id: "m1", name: "Mix", mixed: true, tracks: [] }] } },
  });
  const response = await request(ctx.base, "/v1/library/playlists", { token: phoneToken() });
  assert.equal(response.status, 200);
  const page = await response.json();
  assert.equal(page.error, "spotify_not_linked");
  assert.deepEqual(page.items.map((item) => item.id), ["m1"]);
});

test("liked songs, albums, followed artists and detail routes use the compact shape", async (t) => {
  const { ctx } = await startLibrary(t, {
    overrides: {
      "GET /v1/me/tracks": () => reply(200, { items: [{ track: track("t1") }], next: null }),
      "GET /v1/me/albums": () => reply(200, { items: [{ album: album("al1") }], next: null }),
      "GET /v1/me/following": () => reply(200, {
        artists: { items: [artist("ar1")], next: null, cursors: { after: null } },
      }),
      "GET /v1/artists/ar1": () => reply(200, artist("ar1")),
      "GET /v1/artists/ar1/top-tracks": () => reply(200, { tracks: [track("t2")] }),
      "GET /v1/playlists/p1": () => reply(200, playlist("p1")),
      "GET /v1/playlists/p1/tracks": () => reply(200, {
        items: [{ track: track("t1") }, { track: null }],
        next: null,
      }),
      "GET /v1/albums/al1": () => reply(200, album("al1")),
      "GET /v1/albums/al1/tracks": () => reply(200, {
        items: [track("t3", { album: undefined })],
        next: null,
      }),
    },
  });

  const liked = await (await request(ctx.base, "/v1/library/liked", { token: phoneToken() })).json();
  assert.equal(liked.items[0].uri, "spotify:track:t1");
  assert.deepEqual(liked.items[0].artists, [{ id: "ar1", name: "Artist One" }]);
  assert.deepEqual(liked.items[0].album, { id: "al1", name: "Album One" });
  assert.equal(liked.items[0].image, "https://img.example/t1.jpg");
  assert.equal(liked.items[0].duration_ms, 123_000);

  const albums = await (await request(ctx.base, "/v1/library/albums", { token: phoneToken() })).json();
  assert.equal(albums.items[0].release_date, "2026-01-01");
  assert.equal(albums.items[0].album_type, "album");

  const artists = await (await request(ctx.base, "/v1/library/artists", { token: phoneToken() })).json();
  assert.equal(artists.items[0].followers, 1000);
  assert.deepEqual(artists.items[0].genres, ["pop"]);

  const playlistDetail = await (await request(ctx.base, "/v1/library/playlists/p1", { token: phoneToken() })).json();
  assert.equal(playlistDetail.playlist.name, "Playlist p1");
  assert.deepEqual(playlistDetail.items.map((item) => item.uri), ["spotify:track:t1"]);

  const artistDetail = await (await request(ctx.base, "/v1/artist/ar1", { token: phoneToken() })).json();
  assert.equal(artistDetail.artist.id, "ar1");
  assert.deepEqual(artistDetail.top_tracks.map((item) => item.uri), ["spotify:track:t2"]);

  const albumDetail = await (await request(ctx.base, "/v1/album/al1", { token: phoneToken() })).json();
  assert.equal(albumDetail.album.id, "al1");
  assert.equal(albumDetail.items[0].image, "https://img.example/al1.jpg");
  assert.deepEqual(albumDetail.items[0].album, { id: "al1", name: "Album al1" });
});

test("SoundCloud likes and playlists need a stored token and stay private", async (t) => {
  const oauth = "2-000000-0000000000-soundcloud-token";
  const { ctx } = await startLibrary(t, {
    overrides: {
      "GET /me": () => reply(200, { id: 42, username: "me" }),
      "GET /users/42/track_likes": () => reply(200, { collection: [{ track: soundcloudTrack(1) }] }),
      "GET /users/42/playlists/liked_and_owned": () => reply(200, {
        collection: [{
          id: 202,
          title: "A Set",
          track_count: 3,
          permalink_url: "https://soundcloud.com/artist/sets/a-set",
          user: { id: 9, username: "SC Artist" },
        }],
      }),
    },
  });

  const missing = await request(ctx.base, "/v1/library/soundcloud/likes", { token: phoneToken() });
  assert.equal(missing.status, 409);
  assert.equal((await missing.json()).error, "soundcloud_not_linked");

  const saved = await request(ctx.base, "/v1/vault/soundcloud", {
    method: "PUT",
    token: computerToken(),
    body: { oauthToken: oauth },
  });
  assert.equal(saved.status, 200);

  const likes = await request(ctx.base, "/v1/library/soundcloud/likes", { token: phoneToken() });
  assert.equal(likes.status, 200);
  const liked = await likes.json();
  assert.equal(liked.items[0].uri, "soundcloud:artist/song-1");

  const playlists = await request(ctx.base, "/v1/library/soundcloud/playlists", { token: phoneToken() });
  assert.equal(playlists.status, 200);
  const sets = await playlists.json();
  assert.equal(sets.items[0].id, "202");
  assert.equal(sets.items[0].track_count, 3);

  const requestUrls = ctx.logs.join("\n") + JSON.stringify(liked) + JSON.stringify(sets);
  assert.equal(requestUrls.includes(oauth), false);
});

test("search answers with both sources and keeps one when the other fails", async (t) => {
  const spotifyDown = await startLibrary(t, {
    overrides: {
      "GET /v1/search": () => reply(503, {}),
      "GET /search/tracks": () => reply(200, { collection: [soundcloudTrack(1)] }),
      "GET /search/playlists": () => reply(200, { collection: [] }),
    },
  });
  const first = await request(spotifyDown.ctx.base, "/v1/search?q=hello", { token: phoneToken() });
  assert.equal(first.status, 200);
  const partial = await first.json();
  assert.equal(partial.spotify.error, "spotify_unavailable");
  assert.deepEqual(partial.spotify.tracks, []);
  assert.equal(partial.soundcloud.tracks[0].source, "soundcloud");
  assert.equal(partial.soundcloud.tracks[0].uri, "soundcloud:artist/song-1");

  const soundcloudDown = await startLibrary(t, {
    overrides: {
      soundcloudHome: () => reply(503, {}),
      "GET /v1/search": () => reply(200, {
        tracks: { items: [track("t1")] },
        artists: { items: [artist("ar1")] },
        albums: { items: [album("al1")] },
        playlists: { items: [playlist("p1")] },
      }),
    },
  });
  const second = await request(soundcloudDown.ctx.base, "/v1/search?q=hello", { token: phoneToken() });
  assert.equal(second.status, 200);
  const other = await second.json();
  assert.equal(other.soundcloud.error, "soundcloud_unavailable");
  assert.equal(other.spotify.tracks[0].uri, "spotify:track:t1");
  assert.equal(other.spotify.albums[0].id, "al1");
});

test("home merges recently played and new releases, then caches the result", async (t) => {
  let albumCalls = 0;
  const { ctx } = await startLibrary(t, {
    overrides: {
      "GET /v1/me/player/recently-played": () => reply(200, {
        items: [
          { track: track("t1"), played_at: "2026-11-09T10:00:00.000Z" },
          { track: track("t1"), played_at: "2026-11-09T09:00:00.000Z" },
          { track: track("t2"), played_at: "2026-11-09T08:00:00.000Z" },
        ],
      }),
      "GET /v1/me/following": () => reply(200, {
        artists: { items: [artist("ar1"), artist("ar2")], next: null, cursors: { after: null } },
      }),
      "GET /v1/artists/ar1/albums": () => {
        albumCalls += 1;
        return reply(200, { items: [album("al1", { release_date: "2026-01-01" })] });
      },
      "GET /v1/artists/ar2/albums": () => {
        albumCalls += 1;
        return reply(200, { items: [album("al2", { release_date: "2026-02-01" })] });
      },
    },
  });

  const response = await request(ctx.base, "/v1/home", { token: phoneToken() });
  assert.equal(response.status, 200);
  const home = await response.json();
  assert.deepEqual(home.recently_played.map((item) => item.uri), ["spotify:track:t1", "spotify:track:t2"]);
  assert.deepEqual(home.new_releases.map((item) => item.id), ["al2", "al1"]);
  assert.equal(home.errors, undefined);

  await request(ctx.base, "/v1/home", { token: phoneToken() });
  assert.equal(albumCalls, 2);
});

test("a 429 keeps Retry-After and backs off that listener", async (t) => {
  let calls = 0;
  const { ctx } = await startLibrary(t, {
    overrides: {
      "GET /v1/me/albums": () => {
        calls += 1;
        return reply(429, {}, { "retry-after": "7" });
      },
    },
  });
  const response = await request(ctx.base, "/v1/library/albums", { token: phoneToken() });
  assert.equal(response.status, 429);
  assert.equal(response.headers.get("retry-after"), "7");
  assert.equal((await response.json()).error, "spotify_rate_limited");
  const again = await request(ctx.base, "/v1/library/albums", { token: phoneToken() });
  assert.equal(again.status, 429);
  assert.equal(calls, 1);
});

test("repeated reads are served from the cache and no token reaches a response or log", async (t) => {
  let albumCalls = 0;
  let tokenCalls = 0;
  const { ctx } = await startLibrary(t, {
    overrides: {
      token: () => {
        tokenCalls += 1;
        return reply(200, { access_token: ACCESS, expires_in: 3600 });
      },
      "GET /v1/me/albums": () => {
        albumCalls += 1;
        return reply(200, { items: [{ album: album("al1") }], next: null });
      },
    },
  });

  const first = await request(ctx.base, "/v1/library/albums", { token: phoneToken() });
  const second = await request(ctx.base, "/v1/library/albums", { token: phoneToken() });
  assert.equal(first.status, 200);
  assert.equal(second.status, 200);
  assert.equal(albumCalls, 1);
  assert.equal(tokenCalls, 1);

  const bodies = `${await first.text()}\n${await second.text()}`;
  const logs = ctx.logs.join("\n");
  for (const secret of [ACCESS, REFRESH, CLIENT_ID]) {
    assert.equal(bodies.includes(secret), false);
    assert.equal(logs.includes(secret), false);
  }
});

test("invalid limits, cursors, ids and an empty search are refused", async (t) => {
  const { ctx } = await startLibrary(t);
  assert.equal((await request(ctx.base, "/v1/library/albums?limit=51", { token: phoneToken() })).status, 400);
  assert.equal((await request(ctx.base, "/v1/library/albums?cursor=bad!", { token: phoneToken() })).status, 400);
  assert.equal((await request(ctx.base, "/v1/library/playlists/not%2Fok", { token: phoneToken() })).status, 400);
  assert.equal((await request(ctx.base, "/v1/search?q=", { token: phoneToken() })).status, 400);
  assert.equal((await request(ctx.base, "/v1/artist/bad!id", { token: phoneToken() })).status, 400);
});
