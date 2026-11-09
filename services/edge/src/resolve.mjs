// Source allowlist and two-job resolver. A source is only ever one of
//   youtube:video:<11 chars>
//   soundcloud:<artist>/<track>
//   https://soundcloud.com/<artist>/<track>
// Anything else is refused before a child process exists. Commands use
// argument arrays and end options with "--"; no shell is involved.

import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { runProcess } from "./proc.mjs";

export const MAX_SOURCE_LENGTH = 512;
const YOUTUBE_SOURCE = /^youtube:video:([A-Za-z0-9_-]{11})$/;
const SOUNDCLOUD_URI = /^soundcloud:([A-Za-z0-9][A-Za-z0-9._-]*(?:\/[A-Za-z0-9][A-Za-z0-9._-]*){0,4})$/;
const SOUNDCLOUD_HOSTS = new Set(["soundcloud.com", "www.soundcloud.com", "m.soundcloud.com"]);
const SOUNDCLOUD_PATH = /^\/([A-Za-z0-9][A-Za-z0-9._-]*(?:\/[A-Za-z0-9][A-Za-z0-9._-]*){0,4})\/?$/;

export class ResolveError extends Error {
  constructor(code, status = 400) {
    super(code);
    this.name = "ResolveError";
    this.code = code;
    this.status = status;
  }
}

function safePath(pathname) {
  const match = SOUNDCLOUD_PATH.exec(pathname);
  if (!match) return null;
  const segments = match[1].split("/");
  if (segments.some((segment) => segment === "." || segment === "..")) return null;
  return segments.join("/");
}

/**
 * Parses and canonicalizes a source. Returns `{provider, sourceUri, url}`.
 * Rejects leading dashes, whitespace, control characters, percent-encoding,
 * path traversal, other hosts and every arbitrary URL.
 */
export function parseSource(source) {
  if (typeof source !== "string" || source.length === 0 || source.length > MAX_SOURCE_LENGTH) {
    throw new ResolveError("invalid_source");
  }
  if (/[\u0000-\u0020\u007f]/.test(source)) throw new ResolveError("invalid_source");
  if (source.startsWith("-")) throw new ResolveError("invalid_source");
  if (source.includes("%")) throw new ResolveError("invalid_source");
  // Reject traversal before the URL parser can normalize it away.
  if (source.includes("..") || source.includes("\\")) throw new ResolveError("invalid_source");

  const youtube = YOUTUBE_SOURCE.exec(source);
  if (youtube) {
    return {
      provider: "youtube",
      sourceUri: `youtube:video:${youtube[1]}`,
      url: `https://www.youtube.com/watch?v=${youtube[1]}`,
    };
  }

  const soundcloud = SOUNDCLOUD_URI.exec(source);
  if (soundcloud) {
    const permalink = soundcloud[1];
    if (permalink.split("/").some((segment) => segment === "." || segment === "..")) {
      throw new ResolveError("invalid_source");
    }
    return {
      provider: "soundcloud",
      sourceUri: `soundcloud:${permalink}`,
      url: `https://soundcloud.com/${permalink}`,
    };
  }

  if (source.startsWith("https://")) {
    let url;
    try {
      url = new URL(source);
    } catch {
      throw new ResolveError("invalid_source");
    }
    if (url.protocol !== "https:" || url.username !== "" || url.password !== ""
      || !SOUNDCLOUD_HOSTS.has(url.hostname) || (url.port !== "" && url.port !== "443")
      || url.search !== "" || url.hash !== "") {
      throw new ResolveError("invalid_source");
    }
    const permalink = safePath(url.pathname);
    if (!permalink) throw new ResolveError("invalid_source");
    return {
      provider: "soundcloud",
      sourceUri: `soundcloud:${permalink}`,
      url: `https://soundcloud.com/${permalink}`,
    };
  }

  throw new ResolveError("invalid_source");
}

/** Argument list for yt-dlp. "--" ends options before the URL. */
export function buildYtDlpArgs({ url, outDir }) {
  if (typeof url !== "string" || !url.startsWith("https://")) throw new ResolveError("invalid_source");
  return [
    "--no-playlist",
    "--no-progress",
    "--no-warnings",
    "--no-continue",
    "--no-part",
    "--no-cache-dir",
    "--no-mtime",
    "--force-overwrites",
    "-f",
    "bestaudio[ext=m4a]/140/bestaudio",
    "--output",
    path.join(outDir, "source.%(ext)s"),
    "--",
    url,
  ];
}

/** Remux only: -c:a copy. The container follows the source's MP3-ness. */
export function buildFfmpegArgs({ input, output, format }) {
  if (typeof input !== "string" || typeof output !== "string") throw new ResolveError("invalid_source");
  if (format !== "m4a" && format !== "mp3") throw new ResolveError("invalid_source");
  return [
    "-nostdin",
    "-hide_banner",
    "-loglevel",
    "error",
    "-y",
    "-i",
    input,
    "-map",
    "0:a:0",
    "-vn",
    "-c:a",
    "copy",
    "-f",
    format === "mp3" ? "mp3" : "ipod",
    output,
  ];
}

const AUDIO_EXT = new Set(["m4a", "mp3", "webm", "ogg", "opus", "flac", "wav", "aac"]);

async function findDownloaded(dir) {
  const names = await fs.readdir(dir).catch(() => []);
  for (const name of names) {
    const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "";
    if (name.startsWith("source.") && AUDIO_EXT.has(ext)) {
      return { file: path.join(dir, name), ext };
    }
  }
  return null;
}

export class ResolveService {
  #cache;
  #config;
  #logger;
  #spawn;
  #jobs = new Map();
  #pending = [];
  #active = 0;
  #children = new Set();
  #closing = false;

  constructor({ cache, config, logger, spawn }) {
    this.#cache = cache;
    this.#config = config;
    this.#logger = logger;
    this.#spawn = spawn;
  }

  get concurrency() {
    return Math.min(2, this.#config.resolveConcurrency ?? 2);
  }

  stats() {
    return { running: this.#active, queued: this.#pending.length, jobs: this.#jobs.size };
  }

  /** Resolves a source, sharing one job between identical requests. */
  async ensure(sub, source) {
    if (this.#closing) throw new ResolveError("shutting_down", 503);
    const parsed = parseSource(source);
    const key = this.#cache.keyFor(sub, parsed.sourceUri);
    const existing = this.#cache.get(key);
    if (existing) {
      this.#cache.touch(key);
      return Promise.resolve(this.#view(existing, true));
    }
    const running = this.#jobs.get(key);
    if (running) return running.promise;
    const job = {};
    job.promise = new Promise((resolve, reject) => {
      job.resolve = resolve;
      job.reject = reject;
    });
    // A warm request or a closed socket can leave a rejection unobserved.
    job.promise.catch(() => {});
    job.key = key;
    job.sub = sub;
    job.parsed = parsed;
    this.#jobs.set(key, job);
    this.#pending.push(job);
    this.#pump();
    return job.promise;
  }

  /**
   * Starts resolving up to two distinct upcoming sources without waiting.
   * Invalid sources are counted, never retried and never passed to a tool.
   */
  warm(sub, sources) {
    if (!Array.isArray(sources)) throw new ResolveError("invalid_sources");
    const warmed = [];
    const rejected = [];
    const seen = new Set();
    for (const source of sources) {
      if (warmed.length >= 2) break;
      let parsed;
      try {
        parsed = parseSource(source);
      } catch {
        rejected.push(true);
        continue;
      }
      if (seen.has(parsed.sourceUri)) continue;
      seen.add(parsed.sourceUri);
      const key = this.#cache.keyFor(sub, parsed.sourceUri);
      if (this.#cache.has(key)) {
        this.#cache.touch(key);
        warmed.push({ key, state: "cached" });
        continue;
      }
      const pending = this.ensure(sub, parsed.sourceUri);
      pending.then(() => {}, () => {});
      warmed.push({ key, state: "queued" });
    }
    return { warmed, rejected: rejected.length };
  }

  #view(entry, cached) {
    return { key: entry.key, contentType: entry.contentType, size: entry.size, cached };
  }

  #pump() {
    while (!this.#closing && this.#active < this.concurrency && this.#pending.length > 0) {
      const job = this.#pending.shift();
      this.#active += 1;
      this.#run(job)
        .then((value) => job.resolve(value), (error) => job.reject(error))
        .finally(() => {
          this.#active -= 1;
          this.#jobs.delete(job.key);
          this.#pump();
        });
    }
  }

  async #run(job) {
    const temp = await fs.mkdtemp(path.join(os.tmpdir(), "encore-edge-"));
    try {
      const args = buildYtDlpArgs({ url: job.parsed.url, outDir: temp });
      await runProcess(this.#config.ytdlpBin, args, {
        timeoutMs: this.#config.resolveTimeoutMs,
        spawn: this.#spawn,
        onSpawn: (child) => this.#trackChild(child),
      });
      const downloaded = await findDownloaded(temp);
      if (!downloaded) throw new ResolveError("resolve_failed", 502);
      const format = downloaded.ext === "mp3" ? "mp3" : "m4a";
      const output = path.join(temp, `remuxed.${format}`);
      await runProcess(this.#config.ffmpegBin, buildFfmpegArgs({
        input: downloaded.file,
        output,
        format,
      }), {
        timeoutMs: this.#config.ffmpegTimeoutMs,
        spawn: this.#spawn,
        onSpawn: (child) => this.#trackChild(child),
      });
      const stat = await fs.stat(output).catch(() => null);
      if (!stat?.isFile() || stat.size === 0) throw new ResolveError("resolve_failed", 502);
      const contentType = format === "mp3" ? "audio/mpeg" : "audio/mp4";
      const entry = await this.#cache.put(job.key, output, { contentType });
      if (!entry) throw new ResolveError("cache_full", 507);
      return this.#view(entry, false);
    } catch (error) {
      if (error instanceof ResolveError) throw error;
      this.#logger?.warn("resolve_failed", { code: error?.code ?? "error" });
      throw new ResolveError("resolve_failed", 502);
    } finally {
      await fs.rm(temp, { recursive: true, force: true }).catch(() => {});
    }
  }

  #trackChild(child) {
    this.#children.add(child);
    child.once("close", () => this.#children.delete(child));
  }

  async close() {
    this.#closing = true;
    const error = new ResolveError("shutting_down", 503);
    for (const job of this.#pending.splice(0)) job.reject(error);
    for (const child of this.#children) {
      try {
        child.kill("SIGKILL");
      } catch {
        // Already gone.
      }
    }
    this.#children.clear();
  }
}
