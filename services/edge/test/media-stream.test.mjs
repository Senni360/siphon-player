// The phone starts playing before the download has finished. The `.part` file
// is served with chunked responses while ffmpeg writes it, a range waits only
// for the bytes it asked for, and a client that disappears kills the tools.
import assert from "node:assert/strict";
import test from "node:test";
import { mintToken } from "../src/tokens.mjs";
import { SECRET, request, startEdge } from "./helpers/context.mjs";
import { fakeSpawn } from "./helpers/fakes.mjs";

const SUB = "stream-user";
const SOURCE = "youtube:video:abcdefghijk";

function token() {
  return mintToken({ sub: SUB, role: "phone", scope: "media", ttlSeconds: 120 }, SECRET);
}

/** The same bytes test/fixtures/fake-ytdlp.mjs streams. */
function fixtureBytes(total) {
  const head = Buffer.concat([Buffer.from([0, 0, 0, 0x18]), Buffer.from("ftypM4A ")]);
  const body = Buffer.alloc(Math.max(0, total - head.length));
  for (let index = 0; index < body.length; index += 1) body[index] = index % 251;
  return Buffer.concat([head, body]);
}

async function waitFor(predicate, timeout = 3000) {
  const started = Date.now();
  while (Date.now() - started < timeout) {
    if (await predicate()) return true;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  throw new Error("condition did not settle");
}

async function resolve(ctx, source = SOURCE) {
  const response = await request(ctx.base, "/v1/resolve", { method: "POST", token: token(), body: { source } });
  assert.equal(response.status, 200);
  return response.json();
}

test("resolve answers before the file is complete and media follows it as it grows", async (t) => {
  const total = 128 * 1024;
  const { spawn, state } = fakeSpawn({
    env: {
      FAKE_STREAM_BYTES: String(total),
      FAKE_STREAM_CHUNK: "4096",
      FAKE_STREAM_DELAY_MS: "12",
    },
  });
  const ctx = await startEdge({ spawn });
  t.after(() => ctx.close());

  const body = await resolve(ctx);
  assert.equal(body.streaming, true, "the URL is usable before the download ends");
  assert.equal(body.cached, false);
  assert.equal(body.size, 0);

  const media = await fetch(`${ctx.base}${body.url}`);
  assert.equal(media.status, 200);
  assert.equal(media.headers.get("content-length"), null, "a growing file is chunked");
  assert.equal(media.headers.get("transfer-encoding"), "chunked");
  assert.equal(media.headers.get("content-type"), "audio/mp4");
  const reader = media.body.getReader();
  const first = await reader.read();
  assert.ok(first.value?.byteLength > 0, "the first bytes arrive while the tools still run");

  const chunks = [Buffer.from(first.value)];
  for (;;) {
    const next = await reader.read();
    if (next.done) break;
    chunks.push(Buffer.from(next.value));
  }
  assert.deepEqual(Buffer.concat(chunks), fixtureBytes(total));
  await waitFor(() => ctx.edge.resolve.stats().running === 0);
  assert.equal(state.active, 0, "no child outlives the job");

  const again = await resolve(ctx);
  assert.equal(again.cached, true);
  assert.equal(again.streaming, false);
  assert.equal(again.size, total);

  const ranged = await fetch(`${ctx.base}${again.url}`, { headers: { range: "bytes=4096-8191" } });
  assert.equal(ranged.status, 206);
  assert.equal(ranged.headers.get("content-range"), `bytes 4096-8191/${total}`);
  assert.deepEqual(Buffer.from(await ranged.arrayBuffer()), fixtureBytes(total).subarray(4096, 8192));
});

test("a range request during the fetch returns exactly the bytes it asked for", async (t) => {
  const total = 256 * 1024;
  const { spawn } = fakeSpawn({
    env: {
      FAKE_STREAM_BYTES: String(total),
      FAKE_STREAM_CHUNK: "4096",
      FAKE_STREAM_DELAY_MS: "8",
    },
  });
  const ctx = await startEdge({ spawn });
  t.after(() => ctx.close());

  const body = await resolve(ctx);
  const start = 100_000;
  const end = 120_000;
  const response = await fetch(`${ctx.base}${body.url}`, { headers: { range: `bytes=${start}-${end}` } });
  assert.equal(response.status, 206);
  assert.match(response.headers.get("content-range"), new RegExp(`^bytes ${start}-${end}/(\\*|${total})$`));
  assert.equal(response.headers.get("content-length"), String(end - start + 1));
  assert.deepEqual(Buffer.from(await response.arrayBuffer()), fixtureBytes(total).subarray(start, end + 1));

  await waitFor(() => ctx.edge.resolve.stats().running === 0);
});

test("a client that disappears before the first byte kills the tools and frees the slot", async (t) => {
  const { spawn, state } = fakeSpawn({ env: { FAKE_DELAY_MS: "500", FAKE_STREAM_BYTES: "65536" } });
  const ctx = await startEdge({ spawn });
  t.after(() => ctx.close());
  const controller = new AbortController();
  const started = fetch(`${ctx.base}/v1/resolve`, {
    method: "POST",
    headers: { authorization: `Bearer ${token()}`, "content-type": "application/json" },
    body: JSON.stringify({ source: SOURCE }),
    signal: controller.signal,
  }).catch(() => null);
  await waitFor(() => ctx.edge.resolve.stats().running === 1);
  controller.abort();
  await started;
  await waitFor(() => ctx.edge.resolve.stats().running === 0 && ctx.edge.resolve.stats().jobs === 0);
  await waitFor(() => state.active === 0);

  // The slot is immediately usable for the next song.
  const body = await resolve(ctx);
  assert.equal(body.key.length > 0, true);
});
