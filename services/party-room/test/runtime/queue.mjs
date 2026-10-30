import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { Miniflare, convertV4MiniflareOptions } = require(process.argv[2] ?? "miniflare");
const options = {
  host: "127.0.0.1", cf: false, telemetry: { enabled: false },
  workers: [{
    name: "room-queue-test",
    modules: ["queue.worker.mjs", "../../src/queue.mjs", "../../src/timeline.mjs", "../../../media-gateway/src/uploads.mjs"]
      .map(path => ({ type: "ESModule", path: fileURLToPath(new URL(path, import.meta.url)) })),
    modulesRoot: fileURLToPath(new URL("../../../", import.meta.url)),
    compatibilityDate: "2026-10-30",
    outboundService: () => { throw new Error("External network disabled for room test"); },
    durableObjects: { QUEUE_TEST: { className: "QueueProbe", useSQLite: true } },
  }],
};
const runtime = new Miniflare(convertV4MiniflareOptions ? convertV4MiniflareOptions(options) : options);
try {
  const response = await runtime.dispatchFetch("http://localhost/run");
  assert.equal(response.status, 200, await response.clone().text());
  const result = await response.json();
  assert.equal(result.cases, 9); assert.equal(result.revision, 17); assert.equal(result.pending, 1);
  console.log("Nine local Durable Object cases passed: duplicate commands, ownership/stale state, upload eligibility/cancellation, media revisions, gated starts, timeline reconstruction, media-loss pause, clock sampling and owner-departure recovery.");
} finally {
  await runtime.dispose();
}
