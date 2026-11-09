// Private media reads. A token must be bound to both the user and the exact
// key. Single ranges, suffix ranges and open-ended ranges are supported; a
// multi-range request or an unsatisfiable one gets 416. Compression is left
// off, the ETag is strong, and CORS names only the configured phone origin.

import fs from "node:fs";
import { ownerPrefix } from "./atomic.mjs";
import { CACHE_KEY } from "./cache.mjs";
import { UPLOAD_KEY } from "./uploads.mjs";
import { contentRange, parseRange, unsatisfiedRange } from "./range.mjs";
import { mintToken, verifyToken } from "./tokens.mjs";

export class MediaError extends Error {
  constructor(code, status = 400) {
    super(code);
    this.name = "MediaError";
    this.code = code;
    this.status = status;
  }
}

export function etagFor(entry) {
  return `"${entry.size.toString(16)}-${Math.floor(entry.mtimeMs).toString(16)}"`;
}

function matchesEtag(header, etag) {
  if (typeof header !== "string") return false;
  return header.split(",").some((value) => {
    const trimmed = value.trim();
    return trimmed === "*" || trimmed === etag;
  });
}

export class MediaService {
  #cache;
  #uploads;
  #config;
  #logger;

  constructor({ cache, uploads, config, logger }) {
    this.#cache = cache;
    this.#uploads = uploads;
    this.#config = config;
    this.#logger = logger;
  }

  tokenFor(sub, key, { role = "phone" } = {}) {
    return mintToken({
      sub,
      role,
      scope: "media",
      key,
      ttlSeconds: this.#config.mediaTokenTtlSeconds,
    }, this.#config.tokenSecret);
  }

  urlFor(sub, key, options) {
    return `/v1/media/${encodeURIComponent(key)}?token=${encodeURIComponent(this.tokenFor(sub, key, options))}`;
  }

  lookup(sub, key) {
    // An upload key wins, so a client-chosen key can never be shadowed by a
    // coincidental cache key.
    if (UPLOAD_KEY.test(key)) {
      const uploaded = this.#uploads.lookup(sub, key);
      if (uploaded) return uploaded;
    }
    if (CACHE_KEY.test(key)) {
      const entry = this.#cache.get(key);
      if (!entry) return null;
      if (this.#cache.ownerOf(key) !== ownerPrefix(sub)) return null;
      this.#cache.touch(key);
      return entry;
    }
    return null;
  }

  #cors(origin) {
    const headers = {
      Vary: "Origin",
      "Access-Control-Allow-Methods": "GET, HEAD, OPTIONS",
      "Access-Control-Expose-Headers": "Content-Length, Content-Range, Accept-Ranges, ETag",
    };
    if (typeof origin === "string" && this.#config.allowedOrigins.includes(origin)) {
      headers["Access-Control-Allow-Origin"] = origin;
    }
    return headers;
  }

  /**
   * Serves GET/HEAD. Returns true when the response was handled. All error
   * paths are plain status codes; no file name, path or source URL is echoed.
   */
  serve(req, res, { key, token, origin }) {
    let claims;
    try {
      claims = verifyToken(token, this.#config.tokenSecret, { scope: "media" });
    } catch {
      this.#fail(res, origin, 401, "unauthorized");
      return;
    }
    if (claims.key !== key) {
      this.#fail(res, origin, 403, "forbidden");
      return;
    }
    const entry = this.lookup(claims.sub, key);
    if (!entry) {
      this.#fail(res, origin, 404, "not_found");
      return;
    }
    const etag = etagFor(entry);
    const rangeHeader = req.headers.range;
    if (rangeHeader === undefined && matchesEtag(req.headers["if-none-match"], etag)) {
      res.writeHead(304, {
        ...this.#cors(origin),
        ETag: etag,
        "Cache-Control": "private, max-age=3600",
        "Accept-Ranges": "bytes",
      });
      res.end();
      return;
    }
    const range = parseRange(rangeHeader, entry.size);
    const base = {
      ...this.#cors(origin),
      "Accept-Ranges": "bytes",
      ETag: etag,
      "Cache-Control": "private, max-age=3600",
      "Content-Type": entry.contentType,
      "X-Content-Type-Options": "nosniff",
    };
    if (range?.unsatisfiable) {
      res.writeHead(416, {
        ...base,
        "Content-Range": unsatisfiedRange(entry.size),
        "Content-Length": "0",
      });
      res.end();
      return;
    }
    let status = 200;
    let start = 0;
    let end = entry.size - 1;
    if (range) {
      status = 206;
      start = range.start;
      end = range.end;
      base["Content-Range"] = contentRange(start, end, entry.size);
    }
    const length = entry.size === 0 ? 0 : end - start + 1;
    res.writeHead(status, { ...base, "Content-Length": String(length) });
    if (req.method === "HEAD" || length === 0) {
      res.end();
      return;
    }
    const stream = fs.createReadStream(entry.file, { start, end });
    stream.on("error", () => {
      if (!res.headersSent) this.#fail(res, origin, 500, "internal");
      else res.destroy();
    });
    res.on("close", () => stream.destroy());
    stream.pipe(res);
  }

  #fail(res, origin, status, code) {
    if (res.headersSent) {
      res.destroy();
      return;
    }
    const body = Buffer.from(JSON.stringify({ v: 1, error: code }));
    res.writeHead(status, {
      ...this.#cors(origin),
      "Content-Type": "application/json; charset=utf-8",
      "Content-Length": String(body.length),
      "Cache-Control": "no-store",
    });
    res.end(body);
  }
}
