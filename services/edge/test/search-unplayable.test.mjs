import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { MatchService } from "../src/search.mjs";
import { UnplayableStore } from "../src/unplayable.mjs";
import { fakeSpawn } from "./helpers/fakes.mjs";
import { tempDir } from "./helpers/context.mjs";

const TRACK = {
  uri: "spotify:track:abc",
  name: "Northern Lights",
  durationMs: 200_000,
  artists: [{ name: "Aurora" }],
};

const EXACT = {
  webpage_url: "https://soundcloud.com/aurora/northern-lights",
  title: "Aurora - Northern Lights",
  uploader: "Aurora",
  duration: 200,
};

const SECOND = {
  webpage_url: "https://soundcloud.com/aurora/northern-lights-second",
  title: "Northern Lights",
  uploader: "Aurora",
  duration: 200.4,
};

async function setup(t, entries) {
  const dir = await tempDir();
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const file = path.join(dir, "search.json");
  await fs.writeFile(file, JSON.stringify({ entries }));
  const { spawn, state } = fakeSpawn({ env: { FAKE_SEARCH_SOUNDCLOUD: file } });
  const unplayable = new UnplayableStore({ dir, logger: { warn() {} } });
  await unplayable.init();
  t.after(() => unplayable.flush());
  const matcher = new MatchService({
    config: { ytdlpBin: "yt-dlp", searchTimeoutMs: 10_000 },
    logger: { warn() {} },
    spawn,
    unplayable,
  });
  return { state, unplayable, matcher };
}

test("an unplayable best candidate is skipped for the next one", async (t) => {
  const { state, unplayable, matcher } = await setup(t, [EXACT, SECOND]);
  const first = await matcher.match("user-1", TRACK);
  assert.equal(first.match.source, "soundcloud:aurora/northern-lights");
  assert.equal(state.calls.length, 1);

  await unplayable.add(first.match.source, "drm_protected");
  const second = await matcher.match("user-1", TRACK);
  assert.equal(second.match.source, "soundcloud:aurora/northern-lights-second");
  assert.equal(state.calls.length, 2, "the remembered match was dropped and searched again");
});

test("an unplayable remembered match is dropped and nothing replaces it", async (t) => {
  const { state, unplayable, matcher } = await setup(t, [EXACT]);
  const first = await matcher.match("user-1", TRACK);
  assert.equal(first.match.source, "soundcloud:aurora/northern-lights");
  await unplayable.add(first.match.source, "drm_protected");
  const second = await matcher.match("user-1", TRACK);
  assert.deepEqual(second, { match: null });
  assert.equal(state.calls.length, 2);
});

test("all candidates unplayable answers no match", async (t) => {
  const { state, unplayable, matcher } = await setup(t, [EXACT, SECOND]);
  await unplayable.add("soundcloud:aurora/northern-lights", "drm_protected");
  await unplayable.add("soundcloud:aurora/northern-lights-second", "drm_protected");
  const result = await matcher.match("user-1", TRACK);
  assert.deepEqual(result, { match: null });
  // The search still ran: only the answer is a miss.
  assert.equal(state.calls.length, 1);
});

test("a source never marked unplayable stays playable", async (t) => {
  const { unplayable, matcher } = await setup(t, [EXACT, SECOND]);
  await unplayable.add("soundcloud:aurora/some-other-song", "drm_protected");
  const result = await matcher.match("user-1", TRACK);
  assert.equal(result.match.source, "soundcloud:aurora/northern-lights");
});
