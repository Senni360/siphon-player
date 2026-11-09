import assert from "node:assert/strict";
import test from "node:test";
import { createLogger } from "../src/log.mjs";
import {
  SoundCloudClient,
  extractClientId,
  mapSoundcloudPlaylist,
  mapSoundcloudTrack,
  scriptAssets,
} from "../src/soundcloud.mjs";
import { fakeFetch, header, reply } from "./helpers/http.mjs";

const CLIENT_ID = "abcdefghijklmnopqrstuvwxyz012345";
const TRACK = {
  id: 101,
  title: "A Song",
  duration: 210_000,
  artwork_url: "https://i1.sndcdn.com/artwork.jpg",
  permalink_url: "https://soundcloud.com/some-artist/a-song",
  user: { id: 9, username: "Some Artist" },
};
const PLAYLIST = {
  id: 202,
  title: "A Set",
  artwork_url: null,
  track_count: 12,
  permalink_url: "https://soundcloud.com/some-artist/sets/a-set",
  user: { id: 9, username: "Some Artist" },
  is_album: false,
};

function discovery(handler) {
  const home = [
    '<html><body>',
    '<script src="https://a-v2.sndcdn.com/assets/with-id.js"></script>',
    '<script src="https://a-v2.sndcdn.com/assets/empty.js"></script>',
    "</body></html>",
  ].join("");
  return fakeFetch(async (call) => {
    if (call.url === "https://soundcloud.com/") return reply(200, home);
    if (call.path.endsWith("/empty.js")) return reply(200, "nothing here");
    if (call.path.endsWith("/with-id.js")) {
      return reply(200, `window.__sc = {client_id:"${CLIENT_ID}"};`);
    }
    return handler(call);
  });
}

test("extracts only a valid public client ID and safe script assets", () => {
  assert.equal(extractClientId('client_id:"abcdefghijklmnopqrstuvwxyz012345"'), CLIENT_ID);
  assert.equal(extractClientId('my_client_id:"abcdefghijklmnopqrstuvwxyz012345"'), null);
  assert.equal(extractClientId('client_id:"short"'), null);
  assert.equal(extractClientId("client_id:42"), null);

  const assets = scriptAssets(
    '<script src="/assets/a.js"></script><script src="https://evil.example/b.js"></script>'
    + '<script src="https://i1.sndcdn.com/c.js"></script>',
  );
  assert.equal(assets.length, 2);
  assert.equal(assets[0], "https://soundcloud.com/assets/a.js");
  assert.equal(assets[1], "https://i1.sndcdn.com/c.js");
});

test("discovers the client ID once and searches public tracks", async () => {
  const fetchImpl = discovery((call) => {
    if (call.path === "/search/tracks") {
      return reply(200, { collection: [TRACK] });
    }
    return reply(404, {});
  });
  const client = new SoundCloudClient({ fetch: fetchImpl, now: () => 1_000 });

  assert.equal(await client.clientId(), CLIENT_ID);
  assert.equal(await client.clientId(), CLIENT_ID);
  assert.equal(fetchImpl.count((call) => call.url === "https://soundcloud.com/"), 1);

  const tracks = await client.searchTracks("a song", { limit: 10, offset: 20, scope: "search-user" });
  assert.equal(tracks.length, 1);
  assert.deepEqual(tracks[0], {
    source: "soundcloud",
    id: "101",
    uri: "soundcloud:some-artist/a-song",
    name: "A Song",
    artists: [{ id: "9", name: "Some Artist" }],
    album: null,
    image: "https://i1.sndcdn.com/artwork.jpg",
    duration_ms: 210_000,
    permalink: "https://soundcloud.com/some-artist/a-song",
  });

  const searchCall = fetchImpl.calls.find((call) => call.path === "/search/tracks");
  const url = new URL(searchCall.url);
  assert.equal(url.searchParams.get("q"), "a song");
  assert.equal(url.searchParams.get("limit"), "10");
  assert.equal(url.searchParams.get("offset"), "20");
  assert.equal(url.searchParams.get("linked_partitioning"), "1");
  assert.equal(url.searchParams.get("client_id"), CLIENT_ID);
  assert.equal(header(searchCall, "authorization"), undefined);
});

test("likes and playlists use the stored OAuth token and never put it in a URL or a log", async () => {
  const token = "2-000000-0000000000-oauth-token";
  const lines = [];
  const logger = createLogger({ level: "debug", sink: { log: (line) => lines.push(line) } });
  const fetchImpl = discovery((call) => {
    if (call.path === "/me") return reply(200, { id: 42, username: "me" });
    if (call.path === "/users/42/track_likes") return reply(200, { collection: [{ track: TRACK }] });
    if (call.path === "/users/42/playlists/liked_and_owned") {
      return reply(200, { collection: [PLAYLIST] });
    }
    return reply(404, {});
  });
  const client = new SoundCloudClient({ fetch: fetchImpl, now: () => 5_000, logger });

  const likes = await client.likes(token, { limit: 25, offset: 0, scope: "listener" });
  assert.equal(likes.length, 1);
  assert.equal(likes[0].uri, "soundcloud:some-artist/a-song");

  const playlists = await client.playlists(token, { limit: 25, offset: 0, scope: "listener" });
  assert.deepEqual(playlists[0], {
    source: "soundcloud",
    id: "202",
    uri: "soundcloud:playlist:202",
    name: "A Set",
    image: null,
    track_count: 12,
    owner: { id: "9", name: "Some Artist" },
    is_album: false,
    permalink: "https://soundcloud.com/some-artist/sets/a-set",
  });

  const authenticated = fetchImpl.calls.filter((call) => call.path.startsWith("/users/") || call.path === "/me");
  assert.ok(authenticated.length >= 2);
  for (const call of authenticated) {
    assert.equal(header(call, "authorization"), `OAuth ${token}`);
    assert.equal(call.url.includes(token), false);
  }
  assert.equal(fetchImpl.calls.every((call) => !call.url.includes(token)), true);
  assert.equal(lines.join("\n").includes(token), false);
});

test("mappers reject incomplete objects instead of inventing data", () => {
  assert.equal(mapSoundcloudTrack(null), null);
  assert.equal(mapSoundcloudTrack({ title: "no id" }), null);
  assert.equal(mapSoundcloudPlaylist({ title: "no id" }), null);
  assert.equal(mapSoundcloudTrack({ id: 5, title: "x", permalink_url: "https://soundcloud.com/one" }).uri, "soundcloud:track:5");
});
