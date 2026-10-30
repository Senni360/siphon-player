// Private, admin-only M1 test-media registry. No upload or participant API.
// Every external failure is sanitized. Never log request URLs, bearer tokens,
// bodies, R2 keys or provider credentials.
const encoder = new TextEncoder();
export const LIMITS = Object.freeze({
  fileBytes: 16 * 1024 * 1024,
  totalBytes: 128 * 1024 * 1024,
  assets: 16,
  lifetimeAssets: 64,
  grants: 128,
  bodyBytes: 4096,
  retentionMs: 24 * 60 * 60 * 1000,
  grantMs: 2 * 60 * 60 * 1000,
  retryMs: 60 * 1000,
  maxRetryMs: 15 * 60 * 1000,
});
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const HEX = /^[0-9a-f]{64}$/;
const ETAG = /^[!#-~]{1,128}$/; // Strong R2 raw ETag, without quotes or whitespace.
const MEDIA = /^\/v1\/media\/([^/]+)$/;
const ADMIN = /^\/admin\/(assets|grants)\/([^/]+)$/;

class HttpError extends Error {
  constructor(status, message, headers = {}) {
    super(message);
    this.status = status;
    this.headers = headers;
  }
}
function reject(status, message, headers) { throw new HttpError(status, message, headers); }

export function configured(env) {
  try {
    const origin = new URL(env.MEDIA_ORIGIN);
    return HEX.test(env.ADMIN_SECRET) && origin.protocol === "https:"
      && origin.origin === env.MEDIA_ORIGIN && !origin.username && !origin.password
      && ["head", "get", "delete"].every(method => typeof env.MEDIA_BUCKET?.[method] === "function");
  } catch { return false; }
}

function headers(media = false) {
  const result = new Headers({ "Cache-Control": "private, no-store", "X-Content-Type-Options": "nosniff", "Referrer-Policy": "no-referrer" });
  if (media) {
    result.set("Access-Control-Allow-Origin", "*");
    result.set("Access-Control-Expose-Headers", "Accept-Ranges, Content-Length, Content-Range, Content-Type, ETag");
  }
  return result;
}
export function failure(status, message, head = false, media = false, extra = {}) {
  const result = headers(media);
  result.set("Content-Type", "text/plain; charset=utf-8");
  for (const [key, value] of Object.entries(extra)) result.set(key, value);
  return new Response(head ? null : message, { status, headers: result });
}
function json(value, status = 200) {
  const result = headers();
  result.set("Content-Type", "application/json");
  return new Response(JSON.stringify(value), { status, headers: result });
}
function bytes(hex) { return Uint8Array.from(hex.match(/../g), value => Number.parseInt(value, 16)); }
export async function tokenHash(token) {
  const digest = await crypto.subtle.digest("SHA-256", encoder.encode(token));
  return Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, "0")).join("");
}
async function adminAuthorized(request, expected) {
  const supplied = request.headers.get("Authorization")?.match(/^Bearer ([0-9a-f]{64})$/)?.[1];
  if (!supplied) return false;
  // Let WebCrypto compare MACs rather than comparing secret prefixes in JS.
  const algorithm = { name: "HMAC", hash: "SHA-256" };
  const [signing, verifying] = await Promise.all([
    crypto.subtle.importKey("raw", bytes(supplied), algorithm, false, ["sign"]),
    crypto.subtle.importKey("raw", bytes(expected), algorithm, false, ["verify"]),
  ]);
  const message = encoder.encode("siphon-media-admin-v1");
  const signature = await crypto.subtle.sign("HMAC", signing, message);
  return crypto.subtle.verify("HMAC", verifying, signature, message);
}

async function body(request) {
  if (request.headers.get("Content-Type")?.split(";")[0].trim() !== "application/json") reject(415, "JSON required");
  if (Number(request.headers.get("Content-Length")) > LIMITS.bodyBytes) reject(413, "Request too large");
  if (!request.body) reject(400, "Missing request body");
  const reader = request.body.getReader();
  let timer;
  const deadline = new Promise((_, rejectPromise) => {
    timer = setTimeout(() => rejectPromise(new HttpError(408, "Request body timed out")), 5000);
  });
  try {
    const chunks = [];
    let size = 0;
    while (true) {
      const { done, value } = await Promise.race([reader.read(), deadline]);
      if (done) break;
      size += value.byteLength;
      if (size > LIMITS.bodyBytes) reject(413, "Request too large");
      chunks.push(value);
    }
    const buffer = new Uint8Array(size);
    let offset = 0;
    for (const chunk of chunks) { buffer.set(chunk, offset); offset += chunk.byteLength; }
    try { return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(buffer)); }
    catch { reject(400, "Invalid JSON"); }
  } finally {
    clearTimeout(timer);
    // An attacker-controlled stream can leave its cancellation promise
    // unresolved. The body deadline must still bound this request.
    void reader.cancel().catch(() => {});
  }
}
function exactFields(value, keys) {
  if (!value || typeof value !== "object" || Array.isArray(value)
      || Object.keys(value).sort().join(",") !== [...keys].sort().join(",")) reject(400, "Invalid fields");
}
function integer(value, min, max) { return Number.isSafeInteger(value) && value >= min && value <= max; }
function objectMatches(object, asset) {
  return object && object.size === asset.size && object.etag === asset.etag
    && object.httpEtag === asset.http_etag && object.version === asset.version
    && object.httpMetadata?.contentType === "audio/mpeg" && !object.httpMetadata?.contentEncoding;
}
function ifMatch(request, etag) {
  const supplied = request.headers.get("If-Match");
  if (supplied === null || supplied.trim() === "*") return;
  // A comma inside a quoted opaque tag belongs to the tag, not the list.
  const values = [];
  let quoted = false;
  let start = 0;
  for (let index = 0; index < supplied.length; index++) {
    if (supplied[index] === '"') quoted = !quoted;
    if (supplied[index] === "," && !quoted) { values.push(supplied.slice(start, index).trim()); start = index + 1; }
  }
  values.push(supplied.slice(start).trim());
  if (quoted || !values.every(value => /^(?:W\/)?"[\x21\x23-\x7e]*"$/.test(value))
      || !values.includes(etag)) reject(412, "Recording version no longer matches");
}

export function singleRange(value, size) {
  const unsatisfied = () => reject(416, "Range unavailable", { "Content-Range": `bytes */${size}`, "Accept-Ranges": "bytes" });
  const match = value.match(/^bytes=(\d*)-(\d*)$/);
  if (!match || (!match[1] && !match[2])) return unsatisfied();
  let start;
  let end;
  if (!match[1]) {
    const suffix = Number(match[2]);
    if (!Number.isSafeInteger(suffix) || suffix <= 0) return unsatisfied();
    start = Math.max(0, size - suffix);
    end = size - 1;
  } else {
    start = Number(match[1]);
    end = match[2] ? Number(match[2]) : size - 1;
    if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start >= size || end < start) return unsatisfied();
    end = Math.min(end, size - 1);
  }
  return { offset: start, length: end - start + 1 };
}

export class Registry {
  constructor(state, env, clock = Date.now) {
    this.state = state;
    this.env = env;
    this.now = clock; // Test constructor injection only, never a binding/query.
    this.sql = state.storage.sql;
    this.sql.exec(`CREATE TABLE IF NOT EXISTS assets (
      id TEXT PRIMARY KEY, size INTEGER NOT NULL, etag TEXT NOT NULL, http_etag TEXT NOT NULL,
      version TEXT NOT NULL, duration_seconds INTEGER NOT NULL, expires_at INTEGER NOT NULL,
      state TEXT NOT NULL CHECK (state IN ('ready','deleting','deleted')), attempts INTEGER NOT NULL DEFAULT 0,
      retry_at INTEGER NOT NULL DEFAULT 0)`);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS grants (
      id TEXT PRIMARY KEY, asset_id TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE,
      expires_at INTEGER NOT NULL, revoked INTEGER NOT NULL DEFAULT 0)`);
    // Re-arm persisted work on wakeup as well as after every mutation.
    this.ready = state.blockConcurrencyWhile(() => this.schedule());
  }
  rows(query, ...args) { return this.sql.exec(query, ...args).toArray(); }
  asset(id) { return this.rows("SELECT * FROM assets WHERE id = ?", id)[0]; }

  async fetch(request) {
    let media = false;
    try {
      await this.ready;
      if (!configured(this.env)) reject(503, "Media service unavailable");
      const url = new URL(request.url);
      media = MEDIA.test(url.pathname);
      if (url.origin !== this.env.MEDIA_ORIGIN || url.username || url.password || url.hash) reject(400, "Invalid service URL");
      const path = MEDIA.exec(url.pathname);
      if (path && UUID.test(path[1])) return await this.playback(request, url, path[1]);
      if (url.search || !url.pathname.startsWith("/admin/")) reject(404, "Not found");
      if (!await adminAuthorized(request, this.env.ADMIN_SECRET)) reject(401, "Admin authorization required");
      if (url.pathname === "/admin/cleanup" && request.method === "POST") {
        await this.cleanup();
        const pending = this.rows("SELECT COUNT(*) AS count FROM assets WHERE state = 'deleting'")[0].count;
        return json({ status: pending ? "storage deletion pending retry" : "cleanup complete", pending_deletions: pending }, pending ? 202 : 200);
      }
      const admin = ADMIN.exec(url.pathname);
      if (!admin || !UUID.test(admin[2])) reject(404, "Not found");
      const [, kind, id] = admin;
      if (request.method === "PUT") {
        const input = await body(request);
        return kind === "assets" ? await this.register(id, input) : await this.grant(id, input);
      }
      if (request.method === "DELETE") {
        if (kind === "assets") return await this.remove(id);
        const prior = this.rows("SELECT id FROM grants WHERE id = ?", id)[0];
        if (!prior) {
          if (this.rows("SELECT COUNT(*) AS count FROM grants")[0].count >= LIMITS.grants) reject(429, "Pilot grant quota reached");
          // Cancel an issuance whose request body has not arrived yet. Keep
          // its identity reserved for the maximum possible grant lifetime.
          this.sql.exec("INSERT INTO grants (id,asset_id,token_hash,expires_at,revoked) VALUES (?, '', ?, ?, 1)", id, `revoked:${id}`, this.now() + LIMITS.grantMs);
        } else this.sql.exec("UPDATE grants SET revoked = 1 WHERE id = ?", id);
        await this.schedule();
        return new Response(null, { status: 204, headers: headers() });
      }
      reject(405, "Method unavailable", { Allow: "PUT, DELETE" });
    } catch (error) {
      return error instanceof HttpError
        ? failure(error.status, error.message, request.method === "HEAD", media, error.headers)
        : failure(503, "Media service unavailable", request.method === "HEAD", media);
    }
  }

  async register(id, input) {
    exactFields(input, ["size", "etag", "duration_seconds", "expires_at", "approvedSyntheticMedia"]);
    const now = this.now();
    if (input.approvedSyntheticMedia !== true || !integer(input.size, 1, LIMITS.fileBytes)
        || !ETAG.test(input.etag) || input.etag.includes('"') || !integer(input.duration_seconds, 10, 3600)
        || !integer(input.expires_at, now + 1, now + LIMITS.retentionMs)) reject(400, "Invalid test recording metadata");
    const prior = this.asset(id);
    if (prior) { await this.schedule(); return this.registered(this.asset(id), input); }
    this.checkAssetQuota(input.size);
    const object = await this.env.MEDIA_BUCKET.head(`spike/${id}.mp3`);
    if (!object || object.size !== input.size || object.etag !== input.etag
        || object.httpEtag !== `"${input.etag}"` || typeof object.version !== "string" || !object.version
        || object.httpMetadata?.contentType !== "audio/mpeg" || object.httpMetadata?.contentEncoding) reject(409, "Test recording metadata does not match storage");
    // Network awaits may interleave with another PUT/DELETE. Recheck under SQL
    // transaction before admitting this immutable identity.
    const created = this.state.storage.transactionSync(() => {
      const existing = this.asset(id);
      if (existing) return false;
      if (this.now() >= input.expires_at) reject(400, "Recording retention expired");
      this.checkAssetQuota(input.size);
      this.sql.exec("INSERT INTO assets (id,size,etag,http_etag,version,duration_seconds,expires_at,state) VALUES (?,?,?,?,?,?,?,'ready')",
        id, input.size, input.etag, object.httpEtag, object.version, input.duration_seconds, input.expires_at);
      return true;
    });
    await this.schedule();
    return this.registered(this.asset(id), input, created ? 201 : 200);
  }
  registered(asset, input, status = 200) {
    if (asset.state !== "ready" || asset.size !== input.size || asset.etag !== input.etag
        || asset.duration_seconds !== input.duration_seconds || asset.expires_at !== input.expires_at) reject(409, "Recording identity cannot be replaced");
    return json({ id: asset.id, size: asset.size, etag: asset.http_etag, expires_at: asset.expires_at }, status);
  }
  checkAssetQuota(size) {
    const all = this.rows("SELECT COUNT(*) AS count FROM assets")[0].count;
    const active = this.rows("SELECT COUNT(*) AS count, COALESCE(SUM(size),0) AS bytes FROM assets WHERE state != 'deleted'")[0];
    if (all >= LIMITS.lifetimeAssets || active.count >= LIMITS.assets || active.bytes + size > LIMITS.totalBytes) reject(429, "Pilot recording quota reached");
  }
  async grant(id, input) {
    exactFields(input, ["asset_id", "token_hash", "expires_at"]);
    const now = this.now();
    if (!UUID.test(input.asset_id) || !HEX.test(input.token_hash)
        || !integer(input.expires_at, now + 1, now + LIMITS.grantMs)) reject(400, "Invalid playback grant");
    const asset = this.asset(input.asset_id);
    if (!asset || asset.state !== "ready" || asset.expires_at <= now || input.expires_at > asset.expires_at) reject(409, "Recording unavailable");
    const prior = this.rows("SELECT * FROM grants WHERE id = ?", id)[0];
    if (prior) {
      if (prior.revoked || prior.asset_id !== input.asset_id || prior.token_hash !== input.token_hash
          || prior.expires_at !== input.expires_at) reject(409, "Grant identity cannot be replaced");
      await this.schedule();
      return json({ id, expires_at: prior.expires_at });
    }
    if (this.rows("SELECT COUNT(*) AS count FROM grants")[0].count >= LIMITS.grants) reject(429, "Pilot grant quota reached");
    if (this.rows("SELECT id FROM grants WHERE token_hash = ?", input.token_hash).length) reject(409, "Grant must use a new capability");
    this.sql.exec("INSERT INTO grants (id,asset_id,token_hash,expires_at) VALUES (?,?,?,?)", id, input.asset_id, input.token_hash, input.expires_at);
    await this.schedule();
    return json({ id, expires_at: input.expires_at }, 201);
  }
  authorize(id, hash) {
    const asset = this.asset(id);
    const grant = this.rows("SELECT * FROM grants WHERE token_hash = ? AND asset_id = ?", hash, id)[0];
    const now = this.now();
    if (!asset || asset.state !== "ready" || asset.expires_at <= now || !grant || grant.revoked || grant.expires_at <= now) reject(404, "Playback access unavailable");
    return asset;
  }
  async playback(request, url, id) {
    if (request.method === "OPTIONS") {
      const result = headers(true);
      result.set("Access-Control-Allow-Methods", "GET, HEAD, OPTIONS");
      result.set("Access-Control-Allow-Headers", "Range, If-Match, If-Range");
      return new Response(null, { status: 204, headers: result });
    }
    if (!["GET", "HEAD"].includes(request.method)) reject(405, "Method unavailable", { Allow: "GET, HEAD, OPTIONS" });
    const token = url.searchParams.get("grant");
    if (!HEX.test(token) || [...url.searchParams.keys()].join(",") !== "grant") reject(404, "Playback access unavailable");
    const hash = await tokenHash(token);
    const asset = this.authorize(id, hash);
    ifMatch(request, asset.http_etag);
    let requested = request.method === "GET" ? request.headers.get("Range") : null;
    // If-Range dates and weak/nonmatching tags cannot establish this immutable
    // representation. Serve its complete pinned body, as HTTP requires.
    if (request.headers.has("If-Range") && request.headers.get("If-Range").trim() !== asset.http_etag) requested = null;
    const range = requested === null ? null : singleRange(requested, asset.size);
    const object = request.method === "HEAD"
      ? await this.env.MEDIA_BUCKET.head(`spike/${id}.mp3`)
      : await this.env.MEDIA_BUCKET.get(`spike/${id}.mp3`, { onlyIf: { etagMatches: asset.etag }, ...(range ? { range } : {}) });
    try {
      // A deletion/revocation during the R2 await wins before any response body
      // is handed to the participant. Already admitted streaming bytes cannot
      // be recalled after the response has started.
      this.authorize(id, hash);
      if (object && request.headers.has("If-Match")) ifMatch(request, object.httpEtag);
      if (!objectMatches(object, asset) || (request.method === "GET" && !object.body)
          || (range && (object.range?.offset !== range.offset || object.range?.length !== range.length))) reject(409, "Stored recording changed or disappeared");
    } catch (error) {
      await object?.body?.cancel().catch(() => {});
      throw error;
    }
    const result = headers(true);
    result.set("Content-Type", "audio/mpeg");
    result.set("Accept-Ranges", "bytes");
    result.set("ETag", asset.http_etag);
    result.set("Content-Length", String(range?.length ?? asset.size));
    if (range) result.set("Content-Range", `bytes ${range.offset}-${range.offset + range.length - 1}/${asset.size}`);
    return new Response(request.method === "HEAD" ? null : object.body, { status: range ? 206 : 200, headers: result });
  }
  async remove(id) {
    // Persist the tombstone before R2 I/O. Future grants/reads and late register
    // responses cannot resurrect it, even if deletion fails or the DO restarts.
    const prior = this.asset(id);
    if (!prior) {
      if (this.rows("SELECT COUNT(*) AS count FROM assets")[0].count >= LIMITS.lifetimeAssets) reject(429, "Pilot recording identity quota reached");
      // An in-flight registration may still be awaiting R2. Reserve even an
      // unknown identity so that its late HEAD response cannot resurrect it.
      this.sql.exec("INSERT INTO assets (id,size,etag,http_etag,version,duration_seconds,expires_at,state,retry_at) VALUES (?,0,'','','',0,?,'deleting',?)", id, this.now(), this.now());
    } else this.sql.exec("UPDATE assets SET state = 'deleting', retry_at = ? WHERE id = ? AND state != 'deleted'", this.now(), id);
    await this.state.storage.sync();
    await this.schedule();
    const asset = this.asset(id);
    if (!asset || asset.state === "deleted") return new Response(null, { status: 204, headers: headers() });
    await this.deleteObject(asset);
    await this.schedule();
    return this.asset(id).state === "deleted"
      ? new Response(null, { status: 204, headers: headers() })
      : json({ status: "revoked; storage deletion pending" }, 202);
  }
  async deleteObject(asset) {
    try {
      await this.env.MEDIA_BUCKET.delete(`spike/${asset.id}.mp3`);
      this.sql.exec("UPDATE assets SET state = 'deleted', retry_at = 0 WHERE id = ?", asset.id);
      this.sql.exec("DELETE FROM grants WHERE asset_id = ?", asset.id);
    } catch {
      const attempts = Math.min(asset.attempts + 1, 16);
      const wait = Math.min(LIMITS.retryMs * 2 ** (attempts - 1), LIMITS.maxRetryMs);
      this.sql.exec("UPDATE assets SET attempts = ?, retry_at = ? WHERE id = ? AND state = 'deleting'", attempts, this.now() + wait, asset.id);
    }
  }
  async cleanup() {
    const now = this.now();
    this.sql.exec("UPDATE assets SET state = 'deleting', retry_at = ? WHERE state = 'ready' AND expires_at <= ?", now, now);
    this.sql.exec("DELETE FROM grants WHERE expires_at <= ?", now);
    await this.state.storage.sync();
    // Both collection and work are bounded by the pilot's hard asset cap.
    const due = this.rows("SELECT * FROM assets WHERE state = 'deleting' AND retry_at <= ? ORDER BY retry_at LIMIT 16", now);
    for (const asset of due) await this.deleteObject(asset);
    await this.schedule();
  }
  async schedule() {
    const times = this.rows("SELECT expires_at AS time FROM assets WHERE state = 'ready' UNION ALL SELECT retry_at AS time FROM assets WHERE state = 'deleting' UNION ALL SELECT expires_at AS time FROM grants");
    if (times.length) await this.state.storage.setAlarm(Math.max(this.now() + 1000, Math.min(...times.map(row => row.time))));
    else await this.state.storage.deleteAlarm();
  }
  async alarm() {
    await this.ready;
    if (!configured(this.env)) throw new Error("Media cleanup unavailable");
    await this.cleanup();
  }
}
