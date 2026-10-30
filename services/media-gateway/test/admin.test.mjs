import test from "node:test";
import assert from "node:assert/strict";
import { prepareGrant, applyJob } from "../admin.mjs";
import { tokenHash, LIMITS } from "../src/registry.mjs";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { mkdtemp, readFile, unlink, rmdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const id = "00000000-0000-4000-8000-000000000001";
const now = 1_800_000_000_000;
const credentials = { origin: "https://media.senni.dev", secret: "a".repeat(64) };

test("offline preparation uses separate random capabilities and keeps grant retries stable", async () => {
  const job = prepareGrant(id, 30, now);
  const other = prepareGrant(id, 30, now);
  assert.notEqual(job.grant_id, other.grant_id); assert.notEqual(job.capability, other.capability);
  assert.match(job.capability, /^[0-9a-f]{64}$/); assert.equal(job.expires_at, now + LIMITS.grantMs);
  const requests = [];
  const fetcher = async (url, options) => {
    requests.push([url.toString(), options]);
    return new Response(null, { status: requests.length === 1 ? 201 : 200 });
  };
  const first = await applyJob(credentials, job, fetcher, now);
  assert.deepEqual(await applyJob(credentials, job, fetcher, now + 1), first);
  assert.equal(first.expires_at_unix, (now + LIMITS.grantMs) / 1000);
  assert.equal(first.duration_seconds, 30); assert.ok(first.url.endsWith(`grant=${job.capability}`));
  assert.equal(requests[0][0], requests[1][0]); assert.equal(requests[0][1].body, requests[1][1].body);
  assert.equal(JSON.parse(requests[0][1].body).token_hash, await tokenHash(job.capability));
  assert.doesNotMatch(requests[0][1].body, new RegExp(job.capability));
  assert.equal(requests[0][1].redirect, "error");
});

test("client failures do not expose credentials, capability or response body", async () => {
  const job = prepareGrant(id, 30, now);
  for (const fetcher of [async () => { throw new Error(`PRIVATE ${job.capability}`); }, async () => new Response(`PRIVATE ${job.capability}`, { status: 403 })]) {
    await assert.rejects(applyJob(credentials, job, fetcher, now), error => {
      assert.doesNotMatch(error.message, /PRIVATE/); assert.doesNotMatch(error.message, new RegExp(job.capability)); return true;
    });
  }
});

test("invalid origins, expiring jobs and ambiguous actions never make a request", async () => {
  let requests = 0;
  const fetcher = async () => { requests++; return new Response(null); };
  const job = prepareGrant(id, 30, now);
  await assert.rejects(applyJob({ ...credentials, origin: "http://media.senni.dev" }, job, fetcher, now));
  await assert.rejects(applyJob(credentials, job, fetcher, now + LIMITS.grantMs));
  await assert.rejects(applyJob(credentials, { ...job, action: "upload" }, fetcher, now));
  assert.equal(requests, 0);
});

test("registration, revocation and cleanup route only to bounded admin endpoints", async () => {
  const requests = [];
  const fetcher = async (url, options) => { requests.push([url.pathname, options.method]); return new Response(null, { status: 204 }); };
  await applyJob(credentials, { action: "register", asset_id: id, metadata: { approvedSyntheticMedia: true } }, fetcher, now);
  await applyJob(credentials, { action: "revoke", grant_id: id }, fetcher, now);
  await applyJob(credentials, { action: "delete", asset_id: id }, fetcher, now);
  await applyJob(credentials, { action: "cleanup" }, fetcher, now);
  assert.deepEqual(requests, [[`/admin/assets/${id}`, "PUT"], [`/admin/grants/${id}`, "DELETE"], [`/admin/assets/${id}`, "DELETE"], ["/admin/cleanup", "POST"]]);
});

test("prepare CLI writes a private job without printing capabilities or overwriting files", async t => {
  const folder = await mkdtemp(join(tmpdir(), "siphon-media-admin-test-"));
  const path = join(folder, "grant.private.json");
  t.after(async () => { await unlink(path).catch(() => {}); await rmdir(folder); });
  const command = fileURLToPath(new URL("../admin.mjs", import.meta.url));
  const args = [command, "prepare", "--asset", id, "--duration", "30", "--out", path];
  const { stdout, stderr } = await promisify(execFile)(process.execPath, args);
  const text = await readFile(path, "utf8");
  const job = JSON.parse(text);
  assert.match(job.capability, /^[0-9a-f]{64}$/);
  assert.doesNotMatch(stdout + stderr, new RegExp(job.capability));
  await assert.rejects(promisify(execFile)(process.execPath, args));
  assert.equal(await readFile(path, "utf8"), text);
});
