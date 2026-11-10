// Private media reads. A token must be bound to both the user and the exact
// key. Single ranges, suffix ranges and open-ended ranges are supported; a
// multi-range request or an unsatisfiable one gets 416. Compression is left
// off, the ETag is strong, and CORS names only the configured phone origin.
//
// A cache entry that is still being fetched is served from its growing `.part`
// file with a chunked response. A reader pins the key before the first byte and
// unpins it when the response closes, so eviction never deletes a file that a
// phone is playing.

import fs from "node:fs";
import fsp from "node:fs/promises";
import { Readable } from "node:stream";
import { ownerPrefix } from "./atomic.mjs";
import { CACHE_KEY } from "./cache.mjs";
import { UPLOAD_KEY } from "./uploads.mjs";
import { contentRange, parsePendingRange, parseRange, unsatisfiedRange } from "./range.mjs";
import { mintToken, verifyToken } from "./tokens.mjs";

const CHUNK_BYTES = 64 * 1024;
const DEFAULT_IDLE_TIMEOUT_MS = 120_000;

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
      const entry = this.#cache.get(key) ?? this.#cache.pending(key);
      if (!entry) return null;
      if (this.#cache.ownerOf(key) !== ownerPrefix(sub)) return null;
      if (this.#cache.get(key)) this.#cache.touch(key);
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
  async serve(req, res, { key, token, origin }) {
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
    const cacheKey = CACHE_KEY.test(key);
    if (!entry || (cacheKey && !this.#cache.pin(key))) {
      this.#fail(res, origin, 404, "not_found");
      return;
    }
    let released = false;
    const release = () => {
      if (released) return;
      released = true;
      if (cacheKey) this.#cache.unpin(key);
    };
    res.once("close", release);
    res.once("finish", release);
    const controller = new AbortController();
    res.once("close", () => controller.abort());
    const idleMs = this.#config.mediaIdleTimeoutMs ?? DEFAULT_IDLE_TIMEOUT_MS;
    const socket = res.socket;
    let idleTimer = null;
    const armIdle = () => {
      if (idleMs <= 0) return;
      if (idleTimer) clearTimeout(idleTimer);
      idleTimer = setTimeout(() => {
        idleTimer = null;
        // A keep-alive socket may already carry a newer response; leave that
        // one to its own guard.
        if (!socket || socket.destroyed) return;
        if (socket._httpMessage && socket._httpMessage !== res) return;
        socket.destroy();
        release();
      }, idleMs);
      idleTimer.unref?.();
    };
    socket?.once("close", () => {
      if (idleTimer) clearTimeout(idleTimer);
      idleTimer = null;
    });
    const write = res.write.bind(res);
    res.write = (...args) => {
      armIdle();
      return write(...args);
    };
    const end = res.end.bind(res);
    res.end = (...args) => {
      armIdle();
      return end(...args);
    };
    armIdle();

    try {
      const status = cacheKey ? this.#cache.status(key) : { ...entry, complete: true };
      if (!status) {
        release();
        this.#fail(res, origin, 404, "not_found");
        return;
      }
      if (status.complete) {
        this.#serveComplete(req, res, status, origin);
        return;
      }
      const pendingRange = parsePendingRange(req.headers.range);
      if (pendingRange?.unsatisfiable) {
        this.#unsatisfiable(res, origin, null);
        return;
      }
      if (pendingRange?.suffix) {
        // The length of a growing file is not final yet: wait for the end.
        await this.#cache.waitForBytes(key, Number.MAX_SAFE_INTEGER, controller.signal);
        const settled = this.#cache.status(key);
        if (!settled?.complete) {
          release();
          this.#fail(res, origin, 404, "not_found");
          return;
        }
        this.#serveComplete(req, res, settled, origin);
        return;
      }
      await this.#serveGrowing(req, res, { key, status, range: pendingRange, origin, signal: controller.signal });
    } catch (error) {
      if (error?.name === "AbortError") {
        res.destroy();
        return;
      }
      this.#logger?.warn("media_failed", { code: error?.code ?? "error" });
      if (!res.headersSent) this.#fail(res, origin, 404, "not_found");
      else res.destroy();
    }
  }

  #serveComplete(req, res, entry, origin) {
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
      this.#unsatisfiable(res, origin, entry.size);
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

  /** Serves a file that is still arriving, waiting for each requested offset. */
  async #serveGrowing(req, res, { key, status, range, origin, signal }) {
    const base = {
      ...this.#cors(origin),
      "Accept-Ranges": "bytes",
      "Cache-Control": "no-store",
      "Content-Type": status.contentType,
      "X-Content-Type-Options": "nosniff",
    };
    let httpStatus = 200;
    let start = 0;
    let end = null;
    // `bytes=0-` on a growing file is the whole file: answer 200 chunked, as a
    // plain <audio> first play expects, instead of an open-ended 206.
    if (range && !(range.start === 0 && range.end === null)) {
      start = range.start;
      end = range.end;
      let current = await this.#cache.waitForBytes(key, start + 1, signal);
      if (!current || (current.complete && current.size <= start)) {
        this.#unsatisfiable(res, origin, current?.complete ? current.size : null);
        return;
      }
      if (end !== null) {
        // Wait until the promised end exists (or the fetch finishes early), so
        // a 206 never carries fewer bytes than its Content-Length.
        current = await this.#cache.waitForBytes(key, end + 1, signal);
        if (!current) {
          this.#unsatisfiable(res, origin, null);
          return;
        }
        end = Math.min(end, current.size - 1);
        if (end < start) {
          this.#unsatisfiable(res, origin, current.complete ? current.size : null);
          return;
        }
        base["Content-Range"] = current.complete
          ? contentRange(start, end, current.size)
          : `bytes ${start}-${end}/*`;
        base["Content-Length"] = String(end - start + 1);
      } else {
        base["Content-Range"] = `bytes ${start}-*/*`;
      }
      httpStatus = 206;
    }
    res.writeHead(httpStatus, base);
    if (req.method === "HEAD") {
      res.end();
      return;
    }
    const stream = Readable.from(this.#follow(key, start, end, signal), { objectMode: false });
    stream.on("error", () => {
      if (!res.headersSent) this.#fail(res, origin, 500, "internal");
      else res.destroy();
    });
    res.on("close", () => stream.destroy());
    stream.pipe(res);
  }

  async *#follow(key, start, end, signal) {
    let position = start;
    for (;;) {
      if (signal?.aborted) return;
      let state;
      try {
        state = await this.#cache.waitForBytes(key, position + 1, signal);
      } catch (error) {
        if (error?.name === "AbortError") return;
        throw error;
      }
      if (!state) return;
      const last = end === null ? state.size - 1 : Math.min(state.size - 1, end);
      if (position > last) {
        if (state.complete || (end !== null && position > end)) return;
        continue;
      }
      const length = Math.min(last - position + 1, CHUNK_BYTES);
      const buffer = Buffer.allocUnsafe(length);
      let handle;
      try {
        handle = await fsp.open(state.file, "r");
        const { bytesRead } = await handle.read(buffer, 0, length, position);
        if (bytesRead > 0) {
          position += bytesRead;
          yield buffer.subarray(0, bytesRead);
        } else if (state.complete) {
          return;
        }
      } catch (error) {
        if (error?.code === "ENOENT" && state.complete) return;
        throw error;
      } finally {
        await handle?.close().catch(() => {});
      }
    }
  }

  #unsatisfiable(res, origin, size) {
    res.writeHead(416, {
      ...this.#cors(origin),
      "Accept-Ranges": "bytes",
      "Content-Range": typeof size === "number" ? unsatisfiedRange(size) : "bytes */*",
      "Content-Length": "0",
      "Cache-Control": "no-store",
    });
    res.end();
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
