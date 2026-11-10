// HTTP surface. Every route takes a short-lived HMAC token except /health and
// admin /status. Response errors are small codes, never echoed input.

import fs from "node:fs/promises";
import http from "node:http";
import { CacheStore } from "./cache.mjs";
import { createLogger } from "./log.mjs";
import { LibraryService } from "./library.mjs";
import { LiveHub } from "./live.mjs";
import { MatchService } from "./search.mjs";
import { MediaService } from "./media.mjs";
import { ResolveService } from "./resolve.mjs";
import { SoundCloudClient } from "./soundcloud.mjs";
import { SpotifyClient } from "./spotify.mjs";
import { SYNC_DOCS, SyncStore } from "./sync.mjs";
import { TokenError, sameSecret, verifyToken } from "./tokens.mjs";
import { UploadError, UploadStore } from "./uploads.mjs";
import { Vault } from "./vault.mjs";

const DOC_SET = new Set(SYNC_DOCS);

class HttpError extends Error {
  constructor(code, status = 400) {
    super(code);
    this.name = "HttpError";
    this.code = code;
    this.status = status;
  }
}

function tokenFrom(req, url) {
  const header = req.headers.authorization;
  if (typeof header === "string" && header.toLowerCase().startsWith("bearer ")) return header.slice(7).trim();
  const custom = req.headers["x-edge-token"];
  if (typeof custom === "string" && custom.length > 0) return custom;
  return url.searchParams.get("token") ?? "";
}

function authenticate(req, url, { scope, role = null }) {
  try {
    return verifyToken(tokenFrom(req, url), req.edgeConfig.tokenSecret, { scope, role });
  } catch (error) {
    if (error instanceof TokenError) throw new HttpError("unauthorized", 401);
    throw error;
  }
}

async function readJson(req, limit) {
  let total = 0;
  const chunks = [];
  try {
    for await (const chunk of req) {
      total += chunk.length;
      if (total > limit) throw new HttpError("payload_too_large", 413);
      chunks.push(chunk);
    }
  } catch (error) {
    if (error instanceof HttpError) throw error;
    throw new HttpError("invalid_body", 400);
  }
  if (total === 0) throw new HttpError("invalid_json", 400);
  let value;
  try {
    value = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    throw new HttpError("invalid_json", 400);
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new HttpError("invalid_body", 400);
  return value;
}

function decodeKey(pathname, prefix) {
  const rest = pathname.slice(prefix.length);
  if (rest.length === 0 || rest.includes("/")) throw new HttpError("not_found", 404);
  let key;
  try {
    key = decodeURIComponent(rest);
  } catch {
    throw new HttpError("invalid_key", 400);
  }
  if (key.length === 0 || key.length > 200 || key.includes("/") || key.includes("\\")) {
    throw new HttpError("invalid_key", 400);
  }
  return key;
}

function routeId(rest) {
  let id;
  try {
    id = decodeURIComponent(rest);
  } catch {
    throw new HttpError("invalid_id", 400);
  }
  if (typeof id !== "string" || !/^[A-Za-z0-9]{1,64}$/.test(id)) throw new HttpError("invalid_id", 400);
  return id;
}

function pageQuery(url, { defaultLimit } = {}) {
  const raw = url.searchParams.get("limit");
  let limit;
  if (raw !== null) {
    limit = Number(raw);
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 50) throw new HttpError("invalid_limit", 400);
  } else if (defaultLimit !== undefined) {
    limit = defaultLimit;
  }
  const cursor = url.searchParams.get("cursor");
  if (cursor !== null && (cursor.length === 0 || cursor.length > 200)) throw new HttpError("invalid_cursor", 400);
  return { limit, cursor };
}

function parseContentLength(req) {
  const header = req.headers["content-length"];
  if (header === undefined) return undefined;
  if (typeof header !== "string" || !/^\d{1,15}$/.test(header)) throw new HttpError("invalid_length", 400);
  const value = Number(header);
  if (!Number.isSafeInteger(value) || value < 0) throw new HttpError("invalid_length", 400);
  return value;
}

function json(res, status, value, extra = {}) {
  const body = Buffer.from(JSON.stringify(value));
  res.writeHead(status, {
    ...extra,
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": String(body.length),
    "Cache-Control": "no-store",
  });
  res.end(body);
}

function routeLabel(pathname) {
  if (pathname === "/health") return "/health";
  if (pathname === "/status") return "/status";
  if (pathname.startsWith("/v1/sync/")) return "/v1/sync/:doc";
  if (pathname.startsWith("/v1/vault/")) return "/v1/vault/:provider";
  if (pathname === "/v1/library/playlists") return "/v1/library/playlists";
  if (pathname.startsWith("/v1/library/playlists/")) return "/v1/library/playlists/:id";
  if (pathname === "/v1/library/soundcloud/likes") return "/v1/library/soundcloud/likes";
  if (pathname === "/v1/library/soundcloud/playlists") return "/v1/library/soundcloud/playlists";
  if (pathname === "/v1/library/liked") return "/v1/library/liked";
  if (pathname === "/v1/library/albums") return "/v1/library/albums";
  if (pathname === "/v1/library/artists") return "/v1/library/artists";
  if (pathname === "/v1/search") return "/v1/search";
  if (pathname.startsWith("/v1/artist/")) return "/v1/artist/:id";
  if (pathname.startsWith("/v1/album/")) return "/v1/album/:id";
  if (pathname === "/v1/home") return "/v1/home";
  if (pathname === "/v1/resolve") return "/v1/resolve";
  if (pathname === "/v1/warm") return "/v1/warm";
  if (pathname === "/v1/match") return "/v1/match";
  if (pathname === "/v1/media-token") return "/v1/media-token";
  if (pathname.startsWith("/v1/media/")) return "/v1/media/:key";
  if (pathname.startsWith("/v1/upload/")) return "/v1/upload/:key";
  if (pathname === "/v1/live") return "/v1/live";
  return "other";
}

function corsHeaders(config, origin) {
  const headers = {
    Vary: "Origin",
    "Access-Control-Allow-Methods": "GET, HEAD, POST, PUT, DELETE, OPTIONS",
    "Access-Control-Allow-Headers": "authorization, content-type, range, if-none-match, x-edge-token",
    "Access-Control-Expose-Headers": "Content-Length, Content-Range, Accept-Ranges, ETag, Retry-After",
    "Access-Control-Max-Age": "86400",
  };
  if (typeof origin === "string" && config.allowedOrigins.includes(origin)) {
    headers["Access-Control-Allow-Origin"] = origin;
  }
  return headers;
}

export async function createEdgeServer(config, {
  logger = createLogger({ level: config.logLevel }),
  now = Date.now,
  spawn,
  fetch: fetchImpl = globalThis.fetch,
} = {}) {
  const cache = new CacheStore({ dir: config.dataDir, maxBytes: config.cacheMaxBytes, now });
  await cache.init();
  const uploads = new UploadStore({
    dir: config.dataDir,
    userMaxBytes: config.uploadUserMaxBytes,
    fileMaxBytes: config.uploadFileMaxBytes,
    now,
  });
  await uploads.init();
  const sync = new SyncStore({ dir: config.dataDir, userMaxBytes: config.syncUserMaxBytes, now, logger });
  const resolve = new ResolveService({ cache, config, logger, spawn });
  await resolve.init();
  const matcher = new MatchService({ config, logger, spawn });
  const media = new MediaService({ cache, uploads, config, logger });
  const vault = new Vault({ dir: config.dataDir, key: config.vaultKey, now });
  const spotify = new SpotifyClient({
    vault,
    fetch: fetchImpl,
    now,
    logger,
    cacheTtlMs: config.spotifyCacheTtlMs,
    timeoutMs: config.libraryTimeoutMs,
  });
  const soundcloud = new SoundCloudClient({
    fetch: fetchImpl,
    now,
    logger,
    cacheTtlMs: config.spotifyCacheTtlMs,
    timeoutMs: config.libraryTimeoutMs,
  });
  const library = new LibraryService({ vault, spotify, soundcloud, sync, config, logger, now });
  const startedAt = now();
  let closing = false;

  const server = http.createServer((req, res) => {
    req.edgeConfig = config;
    const started = now();
    const route = routeLabel(safePath(req.url));
    res.on("finish", () => {
      logger.info("http", { method: req.method, route, status: res.statusCode, ms: now() - started });
    });
    handle(req, res, {
      config, cache, uploads, sync, resolve, matcher, media, live, logger, now, startedAt,
      vault, spotify, soundcloud, library,
    })
      .catch((error) => {
        const status = Number.isSafeInteger(error?.status) ? error.status : 500;
        const code = typeof error?.code === "string" ? error.code : "internal";
        if (status >= 500) logger.error("request_failed", { code, route });
        if (!res.headersSent && !res.destroyed && !res.writableEnded) {
          const origin = req.headers.origin;
          const headers = corsHeaders(config, origin);
          if (Number.isSafeInteger(error?.retryAfter) && error.retryAfter > 0) {
            headers["Retry-After"] = String(error.retryAfter);
          }
          // A missing vault key is a configuration state the phone can act on,
          // and a corrupt sync document is a state it must not overwrite.
          const safe = status < 500 || code === "vault_not_configured" || code === "sync_corrupt";
          json(res, status, { v: 1, error: safe ? code : "internal" }, headers);
        } else {
          res.destroy();
        }
      });
  });
  // A bounded number of sockets keeps one slow client from exhausting the
  // container's file descriptors; Caddy pools upstream connections.
  server.maxConnections = 256;
  server.keepAliveTimeout = 30_000;
  server.headersTimeout = 40_000;
  server.requestTimeout = 300_000;
  const live = new LiveHub({ server, config, logger, now });

  async function close() {
    if (closing) return;
    closing = true;
    await live.close();
    await resolve.close();
    await cache.close();
    await sync.close();
    await new Promise((done) => {
      let finished = false;
      const finish = () => {
        if (finished) return;
        finished = true;
        clearTimeout(timer);
        done();
      };
      const timer = setTimeout(() => {
        server.closeAllConnections?.();
        finish();
      }, 3000);
      timer.unref?.();
      if (!server.listening) {
        finish();
        return;
      }
      server.close(() => finish());
    });
  }

  return {
    server,
    cache,
    uploads,
    sync,
    resolve,
    matcher,
    media,
    live,
    vault,
    spotify,
    soundcloud,
    library,
    logger,
    close,
    async listen({ port = config.port, host = config.host } = {}) {
      await new Promise((resolveListen, rejectListen) => {
        server.once("error", rejectListen);
        server.listen(port, host, () => {
          server.off("error", rejectListen);
          resolveListen();
        });
      });
      return server.address();
    },
  };
}

function safePath(raw) {
  try {
    return new URL(raw, "http://localhost").pathname;
  } catch {
    return "";
  }
}

async function handle(req, res, services) {
  const { config, cache, uploads, sync, resolve, matcher, media, live, now, startedAt, vault, library } = services;
  let url;
  try {
    url = new URL(req.url, "http://localhost");
  } catch {
    throw new HttpError("bad_request", 400);
  }
  const pathname = url.pathname;
  const origin = req.headers.origin;
  const cors = corsHeaders(config, origin);

  if (req.method === "OPTIONS") {
    res.writeHead(204, cors);
    res.end();
    return "options";
  }

  if (pathname === "/health") {
    if (req.method !== "GET" && req.method !== "HEAD") {
      throw new HttpError("method_not_allowed", 405);
    }
    json(res, 200, { v: 1, ok: true }, cors);
    return "/health";
  }

  if (pathname === "/status") {
    if (req.method !== "GET" && req.method !== "HEAD") throw new HttpError("method_not_allowed", 405);
    if (!config.adminToken) throw new HttpError("admin_not_configured", 503);
    if (!sameSecret(tokenFrom(req, url), config.adminToken)) throw new HttpError("unauthorized", 401);
    json(res, 200, await statusView({ config, cache, uploads, sync, resolve, live, now }, startedAt), cors);
    return "/status";
  }

  if (pathname.startsWith("/v1/sync/")) {
    const doc = pathname.slice("/v1/sync/".length);
    if (!DOC_SET.has(doc) || doc.includes("/")) throw new HttpError("not_found", 404);
    const claims = authenticate(req, url, { scope: "sync" });
    if (req.method === "GET" || req.method === "HEAD") {
      const record = await sync.get(claims.sub, doc);
      json(res, 200, record, { ...cors, ETag: `"${record.revision}"` });
      return "/v1/sync/:doc";
    }
    if (req.method === "PUT") {
      const body = await readJson(req, config.maxJsonBytes);
      const record = await sync.put(claims.sub, doc, body);
      json(res, 200, record, { ...cors, ETag: `"${record.revision}"` });
      return "/v1/sync/:doc";
    }
    throw new HttpError("method_not_allowed", 405);
  }

  if (pathname.startsWith("/v1/vault/")) {
    const provider = pathname.slice("/v1/vault/".length);
    if (provider !== "spotify" && provider !== "soundcloud") throw new HttpError("not_found", 404);
    if (req.method !== "PUT" && req.method !== "DELETE") throw new HttpError("method_not_allowed", 405);
    // Only the computer may write a sign-in. A phone can read the library but
    // never install or remove a credential.
    const claims = authenticate(req, url, { scope: "sync", role: "computer" });
    if (!vault.enabled) throw new HttpError("vault_not_configured", 503);
    if (req.method === "PUT") {
      const body = await readJson(req, 16 * 1024);
      if (provider === "spotify") await vault.putSpotify(claims.sub, body);
      else await vault.putSoundcloud(claims.sub, body);
      json(res, 200, { v: 1, provider }, cors);
      return "/v1/vault/:provider";
    }
    if (provider === "spotify") await vault.deleteSpotify(claims.sub);
    else await vault.deleteSoundcloud(claims.sub);
    json(res, 200, { v: 1, provider, deleted: true }, cors);
    return "/v1/vault/:provider";
  }

  if (pathname.startsWith("/v1/library/")) {
    if (req.method !== "GET" && req.method !== "HEAD") throw new HttpError("method_not_allowed", 405);
    const claims = authenticate(req, url, { scope: "sync", role: "phone" });
    const rest = pathname.slice("/v1/library/".length);
    if (rest === "playlists") {
      const page = pageQuery(url);
      json(res, 200, await library.playlists(claims.sub, page), cors);
      return "/v1/library/playlists";
    }
    if (rest.startsWith("playlists/")) {
      const id = routeId(rest.slice("playlists/".length));
      const page = pageQuery(url);
      json(res, 200, await library.playlist(claims.sub, id, page), cors);
      return "/v1/library/playlists/:id";
    }
    if (rest === "soundcloud/likes") {
      const page = pageQuery(url);
      json(res, 200, await library.soundcloudLikes(claims.sub, page), cors);
      return "/v1/library/soundcloud/likes";
    }
    if (rest === "soundcloud/playlists") {
      const page = pageQuery(url);
      json(res, 200, await library.soundcloudPlaylists(claims.sub, page), cors);
      return "/v1/library/soundcloud/playlists";
    }
    if (rest === "liked") {
      const page = pageQuery(url);
      json(res, 200, await library.liked(claims.sub, page), cors);
      return "/v1/library/liked";
    }
    if (rest === "albums") {
      const page = pageQuery(url);
      json(res, 200, await library.albums(claims.sub, page), cors);
      return "/v1/library/albums";
    }
    if (rest === "artists") {
      const page = pageQuery(url);
      json(res, 200, await library.artists(claims.sub, page), cors);
      return "/v1/library/artists";
    }
    throw new HttpError("not_found", 404);
  }

  if (pathname === "/v1/search") {
    if (req.method !== "GET" && req.method !== "HEAD") throw new HttpError("method_not_allowed", 405);
    const claims = authenticate(req, url, { scope: "sync", role: "phone" });
    const page = pageQuery(url, { defaultLimit: 10 });
    json(res, 200, await library.search(claims.sub, url.searchParams.get("q") ?? "", page), cors);
    return "/v1/search";
  }

  if (pathname.startsWith("/v1/artist/")) {
    if (req.method !== "GET" && req.method !== "HEAD") throw new HttpError("method_not_allowed", 405);
    const claims = authenticate(req, url, { scope: "sync", role: "phone" });
    const id = routeId(pathname.slice("/v1/artist/".length));
    json(res, 200, await library.artist(claims.sub, id), cors);
    return "/v1/artist/:id";
  }

  if (pathname.startsWith("/v1/album/")) {
    if (req.method !== "GET" && req.method !== "HEAD") throw new HttpError("method_not_allowed", 405);
    const claims = authenticate(req, url, { scope: "sync", role: "phone" });
    const id = routeId(pathname.slice("/v1/album/".length));
    const page = pageQuery(url);
    json(res, 200, await library.album(claims.sub, id, page), cors);
    return "/v1/album/:id";
  }

  if (pathname === "/v1/home") {
    if (req.method !== "GET" && req.method !== "HEAD") throw new HttpError("method_not_allowed", 405);
    const claims = authenticate(req, url, { scope: "sync", role: "phone" });
    json(res, 200, await library.home(claims.sub), cors);
    return "/v1/home";
  }

  if (pathname === "/v1/resolve" && req.method === "POST") {
    const claims = authenticate(req, url, { scope: "media" });
    const body = await readJson(req, 64 * 1024);
    if (typeof body.source !== "string") throw new HttpError("invalid_source", 400);
    // A playlist local file this computer offered is `upload:<key>`; a song it
    // fetched for the phone is `yt-<video-id>`. Prefer either over yt-dlp,
    // which YouTube may refuse here.
    const local = /^upload:([A-Za-z0-9][A-Za-z0-9_-]{0,127})$/.exec(body.source);
    const youtube = /^youtube:video:([A-Za-z0-9_-]{11})$/.exec(body.source);
    const wantedKey = local ? local[1] : youtube ? `yt-${youtube[1]}` : null;
    if (wantedKey) {
      const uploaded = uploads.lookup(claims.sub, wantedKey);
      // A missing local file is final; a YouTube video without an upload is
      // still worth resolving on the edge.
      if (!uploaded && local) throw new HttpError("not_found", 404);
      if (uploaded) {
        json(res, 200, {
          v: 1,
          key: wantedKey,
          contentType: uploaded.contentType,
          size: uploaded.size,
          cached: true,
          streaming: false,
          url: media.urlFor(claims.sub, wantedKey, { role: claims.role }),
        }, cors);
        return "/v1/resolve";
      }
    }
    const job = resolve.acquire(claims.sub, body.source);
    const release = () => job.release();
    res.once("close", release);
    let entry;
    try {
      entry = await job.started;
    } finally {
      // Before the first byte this drops the job; after it the download keeps
      // going so the cache is ready for seeking.
      release();
      res.off("close", release);
    }
    if (res.destroyed || res.writableEnded) return "/v1/resolve";
    json(res, 200, {
      v: 1,
      key: entry.key,
      contentType: entry.contentType,
      size: entry.size,
      cached: entry.cached,
      streaming: entry.streaming === true,
      url: media.urlFor(claims.sub, entry.key, { role: claims.role }),
    }, cors);
    return "/v1/resolve";
  }

  if (pathname === "/v1/warm" && req.method === "POST") {
    const claims = authenticate(req, url, { scope: "media" });
    const body = await readJson(req, 64 * 1024);
    if (!Array.isArray(body.sources) || body.sources.length > 20) throw new HttpError("invalid_sources", 400);
    const result = resolve.warm(claims.sub, body.sources);
    json(res, 202, {
      v: 1,
      warmed: result.warmed.map((entry) => ({
        key: entry.key,
        state: entry.state,
        url: entry.state === "cached" ? media.urlFor(claims.sub, entry.key, { role: claims.role }) : undefined,
      })),
      rejected: result.rejected,
    }, cors);
    return "/v1/warm";
  }

  if (pathname === "/v1/match" && req.method === "POST") {
    const claims = authenticate(req, url, { scope: "media" });
    const body = await readJson(req, 64 * 1024);
    const result = await matcher.match(claims.sub, body);
    json(res, 200, { v: 1, ...result }, cors);
    return "/v1/match";
  }

  if (pathname === "/v1/media-token" && req.method === "POST") {
    const claims = authenticate(req, url, { scope: "media" });
    const body = await readJson(req, 16 * 1024);
    if (typeof body.key !== "string") throw new HttpError("invalid_key", 400);
    if (!media.lookup(claims.sub, body.key)) throw new HttpError("not_found", 404);
    json(res, 200, {
      v: 1,
      key: body.key,
      url: media.urlFor(claims.sub, body.key, { role: claims.role }),
      expiresIn: config.mediaTokenTtlSeconds,
    }, cors);
    return "/v1/media-token";
  }

  if (pathname.startsWith("/v1/media/")) {
    if (req.method !== "GET" && req.method !== "HEAD") throw new HttpError("method_not_allowed", 405);
    const key = decodeKey(pathname, "/v1/media/");
    await media.serve(req, res, { key, token: tokenFrom(req, url), origin });
    return "/v1/media/:key";
  }

  if (pathname.startsWith("/v1/upload/")) {
    if (req.method !== "PUT") throw new HttpError("method_not_allowed", 405);
    const key = decodeKey(pathname, "/v1/upload/");
    const claims = authenticate(req, url, { scope: "media", role: "computer" });
    if (claims.key !== undefined && claims.key !== key) throw new HttpError("forbidden", 403);
    const entry = await uploads.store(claims.sub, key, req, { contentLength: parseContentLength(req) });
    json(res, 201, {
      v: 1,
      key,
      size: entry.size,
      contentType: entry.contentType,
      etag: `"${entry.size.toString(16)}-${Math.floor(entry.mtimeMs).toString(16)}"`,
      url: media.urlFor(claims.sub, key, { role: claims.role }),
    }, cors);
    return "/v1/upload/:key";
  }

  if (pathname === "/v1/live") throw new HttpError("not_found", 404);
  throw new HttpError("not_found", 404);
}

async function statusView({ config, cache, uploads, sync, resolve, live, now }, startedAt) {
  let disk = null;
  try {
    const stat = await fs.statfs(config.dataDir);
    disk = { freeBytes: stat.bavail * stat.bsize, totalBytes: stat.blocks * stat.bsize };
  } catch {
    disk = null;
  }
  return {
    v: 1,
    uptimeSeconds: Math.floor((now() - startedAt) / 1000),
    disk,
    cache: cache.stats(),
    uploads: uploads.stats(),
    queue: resolve.stats(),
    connections: live.stats(),
    sync: await sync.stats(),
  };
}

export { HttpError, readJson, statusView };
