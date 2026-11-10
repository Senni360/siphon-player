// A truncated sync document is not "empty". Treating it as revision 0 would
// let a phone overwrite the user's queue; keep the bytes and refuse instead.
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { mintToken } from "../src/tokens.mjs";
import { SECRET, request, startEdge } from "./helpers/context.mjs";

const SUB = "corrupt-user";
const token = () => mintToken({ sub: SUB, role: "phone", scope: "sync", ttlSeconds: 120 }, SECRET);

test("a corrupt queue document answers sync_corrupt instead of revision 0", async (t) => {
  const ctx = await startEdge();
  t.after(() => ctx.close());
  const dir = ctx.edge.sync.userDir(SUB);
  await fs.mkdir(dir, { recursive: true });
  await fs.writeFile(path.join(dir, "queue.json"), "not json at all");

  const read = await request(ctx.base, "/v1/sync/queue", { token: token() });
  assert.equal(read.status, 500);
  assert.equal((await read.json()).error, "sync_corrupt");

  const files = await fs.readdir(dir);
  assert.ok(files.some((name) => /^queue\.json\.corrupt-\d+$/.test(name)), "the original bytes were kept");

  const write = await request(ctx.base, "/v1/sync/queue", {
    method: "PUT",
    token: token(),
    body: { revision: 41, data: { tracks: [] } },
  });
  assert.equal(write.status, 500);
  assert.equal((await write.json()).error, "sync_corrupt", "a stale owner is not silently accepted");
});
