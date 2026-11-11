import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { MAX_ENTRIES, TTL_MS, UnplayableStore } from "../src/unplayable.mjs";
import { tempDir } from "./helpers/context.mjs";

const SOURCE = "soundcloud:artist/track";

async function setup(t, options = {}) {
  const dir = await tempDir();
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const store = new UnplayableStore({ dir, ...options });
  await store.init();
  return { dir, store };
}

test("remembers a source with its reason and time", async (t) => {
  let clock = 1_000_000;
  const { store } = await setup(t, { now: () => clock });
  assert.equal(store.has(SOURCE), false);
  assert.equal(store.get(SOURCE), null);
  await store.add(SOURCE, "drm_protected");
  assert.deepEqual(store.get(SOURCE), { reason: "drm_protected", at: clock });
  assert.equal(store.has(SOURCE), true);
  assert.equal(store.has("soundcloud:other/track"), false);
  // A stored reason is a small code, never free text.
  await store.add("soundcloud:second/track", "Not A Code");
  assert.equal(store.get("soundcloud:second/track").reason, "unplayable");
  assert.equal(await store.add("", "drm_protected"), false);
});

test("an entry expires after thirty days", async (t) => {
  let clock = 1_000_000;
  const { store } = await setup(t, { now: () => clock });
  assert.equal(TTL_MS, 30 * 24 * 60 * 60 * 1000);
  await store.add(SOURCE, "drm_protected");
  clock += TTL_MS - 1;
  assert.equal(store.has(SOURCE), true);
  clock += 2;
  assert.equal(store.has(SOURCE), false);
  assert.equal(store.get(SOURCE), null);
});

test("the set is bounded and drops the oldest entry", async (t) => {
  const { store } = await setup(t, { maxEntries: 3 });
  assert.equal(MAX_ENTRIES, 5000);
  for (const name of ["a", "b", "c", "d"]) await store.add(`soundcloud:artist/${name}`, "drm_protected");
  assert.equal(store.has("soundcloud:artist/a"), false);
  for (const name of ["b", "c", "d"]) assert.equal(store.has(`soundcloud:artist/${name}`), true, name);
  assert.equal(store.stats().sources, 3);
  // Marking the same source again keeps one entry and moves it last.
  await store.add("soundcloud:artist/d", "drm_protected");
  assert.equal(store.stats().sources, 3);
});

test("the set survives a restart", async (t) => {
  const { dir, store } = await setup(t);
  await store.add(SOURCE, "drm_protected");
  await store.flush();
  const restarted = await new UnplayableStore({ dir }).init();
  assert.deepEqual(restarted.get(SOURCE), store.get(SOURCE));
});

test("a corrupt or missing file starts empty and never throws", async (t) => {
  const { dir, store } = await setup(t);
  assert.equal(store.stats().sources, 0, "a missing file is an empty set");

  await fs.writeFile(path.join(dir, "unplayable.json"), "{not json");
  const corrupt = new UnplayableStore({ dir });
  await assert.doesNotReject(corrupt.init());
  assert.equal(corrupt.stats().sources, 0);
  assert.equal(corrupt.has(SOURCE), false);

  // The next write replaces the corrupt bytes with a readable file.
  await corrupt.add(SOURCE, "drm_protected");
  const reloaded = await new UnplayableStore({ dir }).init();
  assert.equal(reloaded.has(SOURCE), true);

  await fs.writeFile(path.join(dir, "unplayable.json"), JSON.stringify({ version: 1, sources: [] }));
  const wrongShape = new UnplayableStore({ dir });
  await assert.doesNotReject(wrongShape.init());
  assert.equal(wrongShape.stats().sources, 0);
});
