import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { buildSoundCloudSearchUrl, buildYtSearchUrl, candidatesFromProbe, MatchService } from "../src/search.mjs";
import { fakeSpawn } from "./helpers/fakes.mjs";
import { tempDir } from "./helpers/context.mjs";

test("SoundCloud search asks for thirty rows in one request", () => {
  assert.equal(buildSoundCloudSearchUrl("Playboi Carti - Sky"), "scsearch30:Playboi Carti - Sky");
  assert.equal(buildSoundCloudSearchUrl("A\nB"), "scsearch30:A B");
});

test("a query with a lone surrogate is still encodable", () => {
  assert.equal(
    buildYtSearchUrl("A\uD800B"),
    "https://www.youtube.com/results?search_query=A%EF%BF%BDB",
  );
});

test("a probe row carries publisher credits, verification and ISRC", () => {
  const [candidate] = candidatesFromProbe(
    {
      entries: [
        {
          title: "Sky",
          webpage_url: "https://soundcloud.com/opium/sky",
          uploader: "OPIUM",
          artists: ["Playboi Carti"],
          duration: 195.9,
          channel_is_verified: true,
          isrc: "USUM72100001",
        },
      ],
    },
    "soundcloud",
  );
  assert.equal(candidate.title, "Sky");
  assert.equal(candidate.uploader, "OPIUM");
  assert.deepEqual(candidate.credits, ["OPIUM", "Playboi Carti"]);
  assert.equal(candidate.verified, true);
  assert.equal(candidate.isrc, "USUM72100001");
  assert.equal(candidate.duration, 195.9);
});

test("a probe row without credits stays an uploader-only candidate", () => {
  const [candidate] = candidatesFromProbe(
    [{ title: "Sky", webpage_url: "https://soundcloud.com/carti/sky", uploader: "Playboi Carti" }],
    "soundcloud",
  );
  assert.deepEqual(candidate.credits, ["Playboi Carti"]);
  assert.equal(candidate.verified, false);
  assert.equal(candidate.isrc, null);
});

test("ended and post-live streams are not studio recordings", () => {
  for (const live_status of ["is_live", "was_live", "post_live"]) {
    const [candidate] = candidatesFromProbe(
      [{ title: "Sky", webpage_url: "https://soundcloud.com/carti/sky", live_status }],
      "soundcloud",
    );
    assert.equal(candidate.live, true, live_status);
  }
  const [studio] = candidatesFromProbe(
    [{ title: "Sky", webpage_url: "https://soundcloud.com/carti/sky", live_status: "not_live" }],
    "soundcloud",
  );
  assert.equal(studio.live, false);
});

test("a malformed search dump is not remembered as a miss", async (t) => {
  const dir = await tempDir();
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const file = path.join(dir, "search.json");
  await fs.writeFile(file, "{not json");
  const { spawn, state } = fakeSpawn({ env: { FAKE_SEARCH_SOUNDCLOUD: file } });
  const matcher = new MatchService({
    config: { ytdlpBin: "yt-dlp", searchTimeoutMs: 10_000 },
    logger: { warn() {} },
    spawn,
  });
  const track = {
    uri: "spotify:track:abc",
    name: "Northern Lights",
    durationMs: 200_000,
    artists: [{ name: "Aurora" }],
  };
  assert.equal((await matcher.match("user-1", track)).match, null);
  assert.equal(state.calls.length, 1);

  await fs.writeFile(file, JSON.stringify({
    entries: [{
      webpage_url: "https://soundcloud.com/aurora/northern-lights",
      title: "Northern Lights",
      uploader: "Aurora",
      duration: 200,
    }],
  }));
  const second = await matcher.match("user-1", track);
  assert.equal(second.match?.source, "soundcloud:aurora/northern-lights");
  assert.equal(state.calls.length, 2, "the failed dump was not cached");
});

test("the memo key includes the duration, so changed metadata is searched again", async (t) => {
  const dir = await tempDir();
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const file = path.join(dir, "search.json");
  await fs.writeFile(file, JSON.stringify({
    entries: [{
      webpage_url: "https://soundcloud.com/aurora/northern-lights",
      title: "Northern Lights",
      uploader: "Aurora",
      duration: 200,
    }],
  }));
  const { spawn, state } = fakeSpawn({ env: { FAKE_SEARCH_SOUNDCLOUD: file } });
  const matcher = new MatchService({
    config: { ytdlpBin: "yt-dlp", searchTimeoutMs: 10_000 },
    logger: { warn() {} },
    spawn,
  });
  const track = {
    uri: "spotify:track:abc",
    name: "Northern Lights",
    durationMs: 200_000,
    artists: [{ name: "Aurora" }],
  };
  assert.notEqual((await matcher.match("user-1", track)).match, null);
  assert.equal(state.calls.length, 1);
  assert.equal((await matcher.match("user-1", { ...track, durationMs: 150_000 })).match, null);
  assert.equal(state.calls.length, 2, "a new duration is a new lookup");
});

test("a real miss expires, so a later upload is searched again", async (t) => {
  const dir = await tempDir();
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const file = path.join(dir, "search.json");
  await fs.writeFile(file, JSON.stringify({ entries: [] }));
  let clock = 1_000_000;
  const { spawn, state } = fakeSpawn({ env: { FAKE_SEARCH_SOUNDCLOUD: file } });
  const matcher = new MatchService({
    config: { ytdlpBin: "yt-dlp", searchTimeoutMs: 10_000 },
    logger: { warn() {} },
    spawn,
    now: () => clock,
  });
  const track = {
    uri: "spotify:track:abc",
    name: "Northern Lights",
    durationMs: 200_000,
    artists: [{ name: "Aurora" }],
  };
  assert.equal((await matcher.match("user-1", track)).match, null);
  assert.equal((await matcher.match("user-1", track)).match, null);
  assert.equal(state.calls.length, 1, "the miss is remembered for now");
  clock += 15 * 60 * 1000 + 1;
  assert.equal((await matcher.match("user-1", track)).match, null);
  assert.equal(state.calls.length, 2, "the miss expired");
});
