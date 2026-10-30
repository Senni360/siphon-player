// Optional local workerd integration check. Uses a separately installed tool,
// never Cloudflare credentials, existing storage, or the deployment config.
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const { Miniflare, convertV4MiniflareOptions } = require(process.argv[2] ?? "miniflare");
const options = {
  host: "127.0.0.1",
  cf: false,
  telemetry: { enabled: false },
  workers: [{
    name: "upload-cleanup-test",
    modules: ["upload-cleanup.worker.mjs", "../../src/uploads.mjs", "../../src/upload-cleanup.mjs"]
      .map(path => ({ type: "ESModule", path: fileURLToPath(new URL(path, import.meta.url)) })),
    modulesRoot: fileURLToPath(new URL("../../", import.meta.url)),
    compatibilityDate: "2026-10-30",
    outboundService: () => { throw new Error("External network disabled for cleanup test"); },
    durableObjects: { UPLOAD_TEST: { className: "UploadCleanupProbe", useSQLite: true } },
    r2Buckets: ["TEST_BUCKET"],
  }],
};
const runtime = new Miniflare(convertV4MiniflareOptions ? convertV4MiniflareOptions(options) : options);
try {
  const response = await runtime.dispatchFetch("http://localhost/run");
  assert.equal(response.status, 200, await response.clone().text());
  const result = await response.json();
  assert.equal(result.cases, 6);
  assert.equal(result.pending, "cleanup");
  const bucket = await runtime.getR2Bucket("TEST_BUCKET");
  for (const key of result.keys) assert.equal(await bucket.head(key), null);
  assert.equal((await bucket.list()).objects.length, 0);
  console.log("Six local Workers/R2 cleanup cases passed. All five generated objects are absent; unknown creation remains pending.");
} finally {
  await runtime.dispose();
}
