// Explicit operator utility. Never uploads audio or provisions infrastructure.
// The private grant job is retained for idempotent retry after an uncertain
// response. Only its hash goes into the admin request; its token never prints.
import { open, unlink } from "node:fs/promises";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";
import { LIMITS, tokenHash } from "./src/registry.mjs";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const HEX = /^[0-9a-f]{64}$/;
function requireValue(condition, message) { if (!condition) throw new Error(message); }

export function prepareGrant(assetId, duration, now = Date.now()) {
  requireValue(UUID.test(assetId) && Number.isInteger(duration) && duration >= 10 && duration <= 3600, "Invalid recording identity or duration");
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return {
    action: "grant", grant_id: crypto.randomUUID(), asset_id: assetId,
    capability: Array.from(bytes, byte => byte.toString(16).padStart(2, "0")).join(""),
    duration_seconds: duration, expires_at: now + LIMITS.grantMs,
  };
}

export async function applyJob(credentials, job, fetcher = fetch, now = Date.now()) {
  let origin;
  try { origin = new URL(credentials.origin); } catch { throw new Error("Invalid private credentials file"); }
  requireValue(origin.protocol === "https:" && origin.origin === credentials.origin && HEX.test(credentials.secret), "Invalid private credentials file");
  let path;
  let method;
  let data;
  switch (job.action) {
    case "register":
      requireValue(UUID.test(job.asset_id) && job.metadata?.approvedSyntheticMedia === true, "Invalid registration job");
      path = `/admin/assets/${job.asset_id}`; method = "PUT"; data = job.metadata;
      break;
    case "grant":
      requireValue(UUID.test(job.asset_id) && UUID.test(job.grant_id) && HEX.test(job.capability)
        && Number.isInteger(job.duration_seconds) && job.duration_seconds >= 10 && job.duration_seconds <= 3600
        && Number.isSafeInteger(job.expires_at) && job.expires_at - now >= (job.duration_seconds + 600) * 1000
        && job.expires_at - now <= LIMITS.grantMs, "Invalid or expiring grant job; prepare a fresh job");
      path = `/admin/grants/${job.grant_id}`; method = "PUT";
      data = { asset_id: job.asset_id, token_hash: await tokenHash(job.capability), expires_at: job.expires_at };
      break;
    case "revoke":
      requireValue(UUID.test(job.grant_id), "Invalid revocation job"); path = `/admin/grants/${job.grant_id}`; method = "DELETE";
      break;
    case "delete":
      requireValue(UUID.test(job.asset_id), "Invalid deletion job"); path = `/admin/assets/${job.asset_id}`; method = "DELETE";
      break;
    case "cleanup": path = "/admin/cleanup"; method = "POST"; break;
    default: throw new Error("Unknown admin job");
  }
  let response;
  try {
    response = await fetcher(new URL(path, credentials.origin), {
      method, headers: { Authorization: `Bearer ${credentials.secret}`, "Content-Type": "application/json" },
      ...(data === undefined ? {} : { body: JSON.stringify(data) }),
      redirect: "error", signal: AbortSignal.timeout(15_000),
    });
  } catch { throw new Error("Admin request failed; keep the private job for an idempotent retry"); }
  // Do not print or consume an arbitrary server error body.
  void response.body?.cancel().catch(() => {});
  requireValue(response.ok, `Admin request returned HTTP ${response.status}; private job retained for review or retry`);
  return job.action === "grant" ? {
    url: `${credentials.origin}/v1/media/${job.asset_id}?grant=${job.capability}`,
    expires_at_unix: Math.floor(job.expires_at / 1000), duration_seconds: job.duration_seconds,
  } : { status: response.status };
}

async function readPrivateJson(path) {
  requireValue(typeof path === "string", "A private JSON path is required");
  const file = await open(path, "r");
  try {
    const buffer = new Uint8Array(8193);
    let size = 0;
    while (size < buffer.length) {
      const read = await file.read(buffer, size, buffer.length - size, null);
      if (!read.bytesRead) break;
      size += read.bytesRead;
    }
    requireValue(size <= 8192, "Private JSON file exceeds 8 KiB");
    try { return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(buffer.subarray(0, size))); }
    catch { throw new Error("Invalid private JSON file"); }
  } finally { await file.close(); }
}

async function writeNewPrivateJson(path, value) {
  requireValue(typeof path === "string", "A new private output path is required");
  const file = await open(path, "wx", 0o600);
  try {
    await file.writeFile(JSON.stringify(value, null, 2) + "\n");
    await file.sync();
  } catch (error) {
    await file.close(); await unlink(path).catch(() => {}); throw error;
  }
  await file.close();
}

export async function main(args = process.argv.slice(2)) {
  const { values, positionals } = parseArgs({ args, allowPositionals: true, options: {
    credentials: { type: "string" }, job: { type: "string" }, out: { type: "string" },
    asset: { type: "string" }, duration: { type: "string" }, help: { type: "boolean" },
  } });
  if (values.help) {
    console.log("Offline: node admin.mjs prepare --asset UUID --duration SECONDS --out NEW_PRIVATE_JOB.json\nExplicit network action: node admin.mjs apply --credentials PRIVATE_ADMIN.json --job PRIVATE_JOB.json [--out NEW_PRIVATE_PLAYBACK.json]");
    return;
  }
  requireValue(positionals.length === 1, "Choose prepare or apply; see --help");
  if (positionals[0] === "prepare") {
    await writeNewPrivateJson(values.out, prepareGrant(values.asset, Number(values.duration)));
    console.log("Private grant job prepared offline. No network request made.");
    return;
  }
  requireValue(positionals[0] === "apply", "Choose prepare or apply; see --help");
  const credentials = await readPrivateJson(values.credentials);
  const job = await readPrivateJson(values.job);
  requireValue(job?.action !== "grant" || values.out, "Grant application requires a new private output path");
  // The stable private job retains the grant even if writing the playback
  // manifest fails, so rerunning apply never creates another grant.
  const result = await applyJob(credentials, job);
  if (job.action === "grant") {
    await writeNewPrivateJson(values.out, result);
    console.log("Private playback manifest written. Keep its URL out of logs and screenshots.");
  } else {
    console.log(result.status === 202 ? "Access revoked; storage deletion is pending retry." : "Admin operation completed.");
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(error => {
    // File and fetch exceptions can contain paths or URLs. Print no raw error.
    const safe = /^(Invalid |A private |A new private |Private JSON |Choose |Grant application |Unknown admin |Admin request )/.test(error.message);
    console.error(safe ? error.message : "Admin operation failed; private files were not overwritten. Review paths and retry the same job.");
    process.exitCode = 1;
  });
}
