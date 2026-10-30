import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import worker, { MediaRegistry } from "../src/gateway.mjs";
import { Registry, LIMITS, tokenHash } from "../src/registry.mjs";

const origin = "https://media.senni.dev";
const adminSecret = "a".repeat(64);
const assetId = "00000000-0000-4000-8000-000000000001";
const grantId = "00000000-0000-4000-8000-000000000002";
const capability = "b".repeat(64);
const objectKey = `spike/${assetId}.mp3`;

class Storage {
  constructor() {
    this.db = new DatabaseSync(":memory:");
    this.alarm = null;
    this.syncs = 0;
    this.sql = {
      exec: (query, ...parameters) => {
        const statement = this.db.prepare(query);
        const rows = statement.columns().length ? statement.all(...parameters) : (statement.run(...parameters), []);
        return { toArray: () => rows.map(row => ({ ...row })) };
      },
    };
  }
  transactionSync(callback) {
    this.db.exec("BEGIN");
    try { const result = callback(); this.db.exec("COMMIT"); return result; }
    catch (error) { this.db.exec("ROLLBACK"); throw error; }
  }
  async setAlarm(time) { this.alarm = time; }
  async deleteAlarm() { this.alarm = null; }
  async sync() { this.syncs++; }
}

class Bucket {
  constructor() {
    this.objects = new Map();
    this.calls = [];
    this.bytesRead = 0;
    this.cancelled = 0;
    this.failDelete = 0;
  }
  putFixture(key = objectKey, overrides = {}) {
    this.objects.set(key, {
      key, size: 16, etag: "0123456789abcdef", httpEtag: '"0123456789abcdef"',
      version: crypto.randomUUID(), httpMetadata: { contentType: "audio/mpeg" },
      bytes: new TextEncoder().encode("0123456789abcdef"), ...overrides,
    });
  }
  async head(key) {
    this.calls.push(["head", key]);
    const object = this.objects.get(key);
    const copy = object ? { ...object } : null;
    if (this.onHead) await this.onHead();
    return copy;
  }
  async get(key, options) {
    this.calls.push(["get", key, options]);
    const object = this.objects.get(key);
    if (!object) return null;
    if (options.onlyIf.etagMatches !== object.etag) return { ...object };
    const range = options.range ?? { offset: 0, length: object.size };
    const data = object.bytes.slice(range.offset, range.offset + range.length);
    this.bytesRead += data.length;
    const body = new ReadableStream({
      start(controller) { controller.enqueue(data); controller.close(); },
      cancel: () => { this.cancelled++; },
    });
    const result = { ...object, body, range: { ...range } };
    if (this.onGet) await this.onGet();
    return result;
  }
  async delete(key) {
    this.calls.push(["delete", key]);
    if (this.failDelete-- > 0) throw new Error("provider failure containing PRIVATE material");
    this.objects.delete(key);
  }
}

function fixture(t) {
  const storage = new Storage();
  t.after(() => storage.db.close());
  const bucket = new Bucket();
  bucket.putFixture();
  let time = 1_800_000_000_000;
  const state = { storage, blockConcurrencyWhile: callback => callback() };
  const env = { MEDIA_ORIGIN: origin, ADMIN_SECRET: adminSecret, MEDIA_BUCKET: bucket };
  const f = { storage, state, env, bucket, now: () => time, advance: ms => { time += ms; } };
  f.registry = new Registry(state, env, f.now);
  f.restart = () => { f.registry = new Registry(state, env, f.now); };
  f.request = (path, options = {}) => f.registry.fetch(new Request(origin + path, options));
  f.admin = (method, path, input) => f.request(path, {
    method, headers: { Authorization: `Bearer ${adminSecret}`, "Content-Type": "application/json" },
    ...(input === undefined ? {} : { body: JSON.stringify(input) }),
  });
  f.metadata = () => ({ size: 16, etag: "0123456789abcdef", duration_seconds: 30, expires_at: time + LIMITS.retentionMs, approvedSyntheticMedia: true });
  f.register = (input = f.metadata(), id = assetId) => f.admin("PUT", `/admin/assets/${id}`, input);
  f.issue = async (overrides = {}, id = grantId) => f.admin("PUT", `/admin/grants/${id}`, {
    asset_id: assetId, token_hash: await tokenHash(capability), expires_at: time + LIMITS.grantMs, ...overrides,
  });
  f.media = (options = {}, token = capability, id = assetId) => f.request(`/v1/media/${id}?grant=${token}`, options);
  f.ready = async () => { assert.equal((await f.register()).status, 201); assert.equal((await f.issue()).status, 201); f.bucket.calls = []; };
  return f;
}

test("registration requires admin authorization before touching storage or parsing content", async t => {
  const f = fixture(t);
  for (const token of [undefined, "c".repeat(64), "short"]) {
    const response = await f.request(`/admin/assets/${assetId}`, { method: "PUT", headers: token ? { Authorization: `Bearer ${token}` } : {}, body: "PRIVATE invalid JSON" });
    assert.equal(response.status, 401);
    assert.doesNotMatch(await response.text(), /PRIVATE/);
  }
  assert.equal(f.bucket.calls.length, 0);
  assert.equal((await f.media()).status, 404);
  assert.equal(f.bucket.calls.length, 0);
});

test("configuration fails closed and production entrypoint uses persistent registry", async t => {
  const f = fixture(t);
  for (const missing of ["ADMIN_SECRET", "MEDIA_ORIGIN", "MEDIA_BUCKET"]) {
    const value = f.env[missing]; delete f.env[missing];
    assert.equal((await f.register()).status, 503); f.env[missing] = value;
  }
  assert.equal((await worker.fetch(new Request(origin), f.env)).status, 503);
  f.env.MEDIA_REGISTRY = { idFromName: name => { assert.equal(name, "m1-pilot-v1"); return name; }, get: () => f.registry };
  assert.equal((await worker.fetch(new Request(`${origin}/v1/media/${assetId}?grant=${capability}`), f.env)).status, 404);
  assert.equal(MediaRegistry.prototype instanceof Registry, true);
  await worker.scheduled({}, f.env);
});

test("admin registration is bounded, immutable, idempotent and contains no arbitrary key", async t => {
  const f = fixture(t);
  const metadata = f.metadata();
  for (const changed of [{ approvedSyntheticMedia: false }, { size: LIMITS.fileBytes + 1 }, { expires_at: f.now() + LIMITS.retentionMs + 1 }, { key: "someone-else.mp3" }]) {
    assert.equal((await f.register({ ...metadata, ...changed })).status, 400);
  }
  assert.equal(f.bucket.calls.length, 0);
  assert.equal((await f.register({ ...metadata, size: 15 })).status, 409);
  assert.equal((await f.register(metadata)).status, 201);
  assert.equal((await f.register(metadata)).status, 200);
  assert.equal((await f.register({ ...metadata, duration_seconds: 31 })).status, 409);
  assert.equal(f.storage.alarm, metadata.expires_at);
  const row = f.registry.asset(assetId);
  assert.equal(row.version, f.bucket.objects.get(objectKey).version);
  assert.equal(row.http_etag, '"0123456789abcdef"');
});

test("correct byte ranges, suffixes, full GET, HEAD and CORS use the same grant", async t => {
  const f = fixture(t); await f.ready();
  for (const [range, expected, contentRange] of [["bytes=3-5", "345", "bytes 3-5/16"], ["bytes=12-", "cdef", "bytes 12-15/16"], ["bytes=-4", "cdef", "bytes 12-15/16"], ["bytes=10-999", "abcdef", "bytes 10-15/16"]]) {
    const response = await f.media({ headers: { Range: range } });
    assert.equal(response.status, 206); assert.equal(await response.text(), expected);
    assert.equal(response.headers.get("Content-Range"), contentRange);
    assert.equal(response.headers.get("Content-Length"), String(expected.length));
    assert.equal(response.headers.get("Content-Type"), "audio/mpeg");
    assert.equal(response.headers.get("ETag"), '"0123456789abcdef"');
    assert.equal(response.headers.get("Cache-Control"), "private, no-store");
    assert.equal(response.headers.get("Access-Control-Allow-Origin"), "*");
    assert.equal(response.headers.get("Access-Control-Allow-Credentials"), null);
  }
  const full = await f.media(); assert.equal(full.status, 200); assert.equal(await full.text(), "0123456789abcdef");
  const before = f.bucket.bytesRead;
  const head = await f.media({ method: "HEAD", headers: { Range: "bytes=0-0" } });
  assert.equal(head.status, 200); assert.equal(head.headers.get("Content-Length"), "16"); assert.equal(await head.text(), "");
  assert.equal(f.bucket.bytesRead, before);
  const options = await f.request(`/v1/media/${assetId}`, { method: "OPTIONS" });
  assert.equal(options.status, 204); assert.equal(options.headers.get("Access-Control-Allow-Headers"), "Range, If-Match, If-Range");
});

test("invalid and multiple ranges return 416 without reading object bytes", async t => {
  const f = fixture(t); await f.ready();
  for (const range of ["bytes=16-", "bytes=9-2", "bytes=-0", "bytes=0-1,3-4", "items=0-1", "bytes=-", "bytes=999999999999999999999-", "bytes=1-99999999999999999999"]) {
    const response = await f.media({ headers: { Range: range } });
    assert.equal(response.status, 416, range); assert.equal(response.headers.get("Content-Range"), "bytes */16");
  }
  assert.equal(f.bucket.bytesRead, 0); assert.equal(f.bucket.calls.length, 0);
});

test("If-Match fails before storage reads and conditional R2 reads pin the immutable version", async t => {
  const f = fixture(t); await f.ready();
  for (const method of ["HEAD", "GET"]) {
    for (const value of ['"wrong"', 'W/"0123456789abcdef"']) assert.equal((await f.media({ method, headers: { "If-Match": value } })).status, 412);
  }
  assert.equal(f.bucket.calls.length, 0);
  const response = await f.media({ headers: { "If-Match": '"ignored", "0123456789abcdef"', Range: "bytes=1-2" } });
  assert.equal(await response.text(), "12");
  assert.deepEqual(f.bucket.calls[0][2], { onlyIf: { etagMatches: "0123456789abcdef" }, range: { offset: 1, length: 2 } });
  const before = f.bucket.bytesRead;
  f.bucket.putFixture(objectKey, { etag: "new", httpEtag: '"new"' });
  assert.equal((await f.media({ headers: { "If-Match": '"0123456789abcdef"' } })).status, 412);
  assert.equal(f.bucket.bytesRead, before);
  assert.equal((await f.media()).status, 409);
});

test("same-size and same-etag replacement with a new R2 version is refused", async t => {
  const f = fixture(t); await f.ready();
  f.bucket.putFixture();
  assert.equal((await f.media()).status, 409);
  assert.equal(f.bucket.cancelled, 1);
});

test("quoted comma ETags and If-Range follow HTTP conditions without weakening object pinning", async t => {
  const f = fixture(t);
  f.bucket.putFixture(objectKey, { etag: "tag,with,commas", httpEtag: '"tag,with,commas"' });
  assert.equal((await f.register({ ...f.metadata(), etag: "tag,with,commas" })).status, 201);
  assert.equal((await f.issue()).status, 201);
  const response = await f.media({ headers: { "If-Match": 'W/"weak", "tag,with,commas"', Range: "bytes=1-2", "If-Range": '"tag,with,commas"' } });
  assert.equal(response.status, 206); assert.equal(await response.text(), "12");
  for (const tag of ['"old"', 'W/"tag,with,commas"', "Fri, 30 Oct 2026 00:00:00 GMT", "invalid"]) {
    const full = await f.media({ headers: { "If-Range": tag, Range: "bytes=1-2" } });
    assert.equal(full.status, 200); assert.equal(await full.text(), "0123456789abcdef");
    assert.equal(full.headers.get("Content-Range"), null);
  }
  assert.equal((await f.media({ headers: { "If-Match": '"tag,with,commas",bad' } })).status, 412);
});

test("capabilities are asset-specific, hashed at rest, bounded and revocable on every read", async t => {
  const f = fixture(t); await f.ready();
  const serialized = JSON.stringify(f.registry.rows("SELECT * FROM grants"));
  assert.doesNotMatch(serialized, new RegExp(capability));
  assert.match(serialized, new RegExp(await tokenHash(capability)));
  assert.equal((await f.media({}, "c".repeat(64))).status, 404);
  assert.equal((await f.media({}, capability, crypto.randomUUID())).status, 404);
  assert.equal((await f.issue({ expires_at: f.now() + LIMITS.grantMs + 1 }, crypto.randomUUID())).status, 400);
  assert.equal((await f.issue()).status, 200);
  assert.equal((await f.admin("DELETE", `/admin/grants/${grantId}`)).status, 204);
  f.restart();
  assert.equal((await f.media()).status, 404);
  assert.equal((await f.media({ method: "HEAD" })).status, 404);
  assert.equal((await f.issue()).status, 409);
  assert.equal(f.bucket.calls.length, 0);
});

test("expiry is checked at its exact boundary and refresh is a new explicit grant", async t => {
  const f = fixture(t); await f.ready();
  f.advance(LIMITS.grantMs - 1);
  assert.equal((await f.media({ method: "HEAD" })).status, 200);
  f.advance(1);
  const before = f.bucket.calls.length;
  assert.equal((await f.media()).status, 404); assert.equal(f.bucket.calls.length, before);
  assert.equal((await f.issue({ token_hash: await tokenHash("d".repeat(64)) }, crypto.randomUUID())).status, 201);
  assert.equal((await f.media({}, "d".repeat(64))).status, 200);
  assert.equal((await f.media()).status, 404);
});

test("revocation during an R2 await cancels its body before response admission", async t => {
  const f = fixture(t); await f.ready();
  f.bucket.onGet = async () => { assert.equal((await f.admin("DELETE", `/admin/grants/${grantId}`)).status, 204); };
  assert.equal((await f.media()).status, 404);
  assert.equal(f.bucket.cancelled, 1);
});

test("failed asset deletion stays revoked over restart and alarm retries idempotently", async t => {
  const f = fixture(t); await f.ready();
  f.bucket.failDelete = 2;
  assert.equal((await f.admin("DELETE", `/admin/assets/${assetId}`)).status, 202);
  assert.equal(f.registry.asset(assetId).state, "deleting"); assert.ok(f.storage.syncs > 0);
  f.restart();
  assert.equal((await f.media()).status, 404); assert.equal((await f.issue()).status, 409);
  assert.equal((await f.register()).status, 409);
  f.advance(LIMITS.retryMs); await f.registry.alarm();
  assert.equal(f.registry.asset(assetId).state, "deleting");
  assert.equal(f.storage.alarm, f.now() + 2 * LIMITS.retryMs);
  f.advance(2 * LIMITS.retryMs); await f.registry.alarm();
  assert.equal(f.registry.asset(assetId).state, "deleted");
  assert.equal(f.bucket.objects.has(objectKey), false); assert.equal(f.registry.rows("SELECT * FROM grants").length, 0);
  const calls = f.bucket.calls.length;
  assert.equal((await f.admin("DELETE", `/admin/assets/${assetId}`)).status, 204);
  await f.registry.alarm(); assert.equal(f.bucket.calls.length, calls);
  assert.equal(f.storage.alarm, null);
});

test("retention deletes without reads and scheduled fallback resumes after lost alarms", async t => {
  const f = fixture(t); await f.ready();
  f.advance(LIMITS.retentionMs);
  assert.equal((await f.media()).status, 404);
  f.storage.alarm = null;
  f.env.MEDIA_REGISTRY = { idFromName: name => name, get: () => f.registry };
  await worker.scheduled({}, f.env);
  assert.equal(f.bucket.objects.has(objectKey), false);
  assert.equal(f.registry.asset(assetId).state, "deleted");
  assert.equal(f.registry.rows("SELECT * FROM grants").length, 0);
});

test("delete wins over late registration and unknown grant revocation wins over late issuance", async t => {
  const f = fixture(t);
  let release;
  let entered;
  const started = new Promise(resolve => { entered = resolve; });
  f.bucket.onHead = async () => { entered(); await new Promise(resolve => { release = resolve; }); };
  const pending = f.register(); await started;
  assert.equal((await f.admin("DELETE", `/admin/assets/${assetId}`)).status, 204);
  release(); assert.equal((await pending).status, 409);
  assert.equal(f.registry.asset(assetId).state, "deleted");
  const other = crypto.randomUUID();
  assert.equal((await f.admin("DELETE", `/admin/grants/${other}`)).status, 204);
  // The tombstone itself remains durable, even without an associated asset.
  f.restart(); assert.equal(f.registry.rows("SELECT revoked FROM grants WHERE id = ?", other)[0].revoked, 1);
});

test("grant quota includes revoked and expired records until cleanup", async t => {
  const f = fixture(t); assert.equal((await f.register()).status, 201);
  for (let i = 0; i < LIMITS.grants; i++) assert.equal((await f.issue({ token_hash: await tokenHash(`unique-${i}`), expires_at: f.now() + 1000 }, crypto.randomUUID())).status, 201);
  assert.equal((await f.issue()).status, 429);
  f.advance(1000); assert.equal((await f.issue()).status, 429);
  await f.registry.alarm(); assert.equal((await f.issue()).status, 201);
});

test("total-byte quota includes ready and deleting storage", async t => {
  const f = fixture(t);
  for (let i = 0; i < 8; i++) {
    const id = crypto.randomUUID();
    f.bucket.putFixture(`spike/${id}.mp3`, { size: LIMITS.fileBytes });
    assert.equal((await f.register({ ...f.metadata(), size: LIMITS.fileBytes }, id)).status, 201);
  }
  assert.equal((await f.register()).status, 429);
});

test("concurrent registration rechecks the asset cap after R2 awaits", async t => {
  const f = fixture(t);
  for (let i = 0; i < LIMITS.assets - 1; i++) {
    const id = crypto.randomUUID(); f.bucket.putFixture(`spike/${id}.mp3`);
    assert.equal((await f.register(f.metadata(), id)).status, 201);
  }
  const ids = [crypto.randomUUID(), crypto.randomUUID()];
  for (const id of ids) f.bucket.putFixture(`spike/${id}.mp3`);
  let release;
  const blocked = new Promise(resolve => { release = resolve; });
  let reads = 0; let entered;
  const bothEntered = new Promise(resolve => { entered = resolve; });
  f.bucket.onHead = async () => { if (++reads === 2) entered(); await blocked; };
  const requests = ids.map(id => f.register(f.metadata(), id));
  await bothEntered; release();
  const statuses = (await Promise.all(requests)).map(response => response.status).sort();
  assert.deepEqual(statuses, [201, 429]);
  assert.equal(f.registry.rows("SELECT COUNT(*) AS count FROM assets WHERE state='ready'")[0].count, LIMITS.assets);
});

test("revoking an unknown grant defeats a delayed issuance body", async t => {
  const f = fixture(t); assert.equal((await f.register()).status, 201);
  let controller;
  const stream = new ReadableStream({ start(value) { controller = value; } });
  const pending = f.request(`/admin/grants/${grantId}`, {
    method: "PUT", duplex: "half", body: stream,
    headers: { Authorization: `Bearer ${adminSecret}`, "Content-Type": "application/json" },
  });
  assert.equal((await f.admin("DELETE", `/admin/grants/${grantId}`)).status, 204);
  controller.enqueue(new TextEncoder().encode(JSON.stringify({ asset_id: assetId, token_hash: await tokenHash(capability), expires_at: f.now() + LIMITS.grantMs })));
  controller.close();
  assert.equal((await pending).status, 409); assert.equal((await f.media()).status, 404);
});

test("cleanup failures retain bounded retries beyond the platform's automatic retry count", async t => {
  const f = fixture(t); await f.ready(); f.bucket.failDelete = 20;
  f.advance(LIMITS.retentionMs);
  for (let count = 0; count < 10; count++) {
    const result = await f.admin("POST", "/admin/cleanup");
    assert.equal(result.status, 202); assert.equal((await result.json()).pending_deletions, 1);
    assert.equal(f.registry.asset(assetId).state, "deleting");
    const wait = f.storage.alarm - f.now();
    assert.ok(wait > 0 && wait <= LIMITS.maxRetryMs); f.advance(wait);
    f.restart();
  }
  f.bucket.failDelete = 0; await f.registry.alarm();
  assert.equal(f.registry.asset(assetId).state, "deleted");
});

test("oversized and malformed admin bodies are bounded and errors reveal no supplied values", async t => {
  const f = fixture(t);
  for (const data of ["PRIVATE", JSON.stringify({ secret: "PRIVATE" })]) {
    const response = await f.request(`/admin/assets/${assetId}`, { method: "PUT", headers: { Authorization: `Bearer ${adminSecret}`, "Content-Type": "application/json" }, body: data });
    assert.equal(response.status, 400); assert.doesNotMatch(await response.text(), /PRIVATE/);
  }
  const large = await f.request(`/admin/assets/${assetId}`, { method: "PUT", headers: { Authorization: `Bearer ${adminSecret}`, "Content-Type": "application/json" }, body: "x".repeat(LIMITS.bodyBytes + 1) });
  assert.equal(large.status, 413); assert.equal(f.bucket.calls.length, 0);
  f.bucket.head = async () => { throw new Error("PRIVATE provider URL"); };
  const unavailable = await f.register(); assert.equal(unavailable.status, 503); assert.doesNotMatch(await unavailable.text(), /PRIVATE/);
});

test("oversize response does not await an uncooperative stream cancellation", { timeout: 1000 }, async t => {
  const f = fixture(t);
  const stream = new ReadableStream({
    start(controller) { controller.enqueue(new Uint8Array(LIMITS.bodyBytes + 1)); },
    cancel() { return new Promise(() => {}); },
  });
  const response = await f.request(`/admin/assets/${assetId}`, {
    method: "PUT", duplex: "half", body: stream,
    headers: { Authorization: `Bearer ${adminSecret}`, "Content-Type": "application/json" },
  });
  assert.equal(response.status, 413);
});

test("unknown routes, insecure origins, duplicate query grants and encoded paths are rejected", async t => {
  const f = fixture(t); await f.ready();
  assert.equal((await f.registry.fetch(new Request(`http://media.senni.dev/v1/media/${assetId}?grant=${capability}`))).status, 400);
  assert.equal((await f.request(`/v1/media/${assetId}?grant=${capability}&grant=${capability}`)).status, 404);
  assert.equal((await f.request(`/v1/media/%2e%2e%2fsecret?grant=${capability}`)).status, 404);
  assert.equal((await f.request("/upload", { method: "POST", body: "no" })).status, 404);
  assert.equal(f.bucket.calls.length, 0);
});
