// Source allowlist and streaming resolver. A source is only ever one of
//   youtube:video:<11 chars>
//   soundcloud:<artist>/<track>
//   https://soundcloud.com/<artist>/<track>
// Anything else is refused before a child process exists. Commands use
// argument arrays and end options with "--"; no shell is involved.
//
// yt-dlp writes the chosen audio stream to stdout, ffmpeg remuxes it (copy
// only, never transcode) into a fragmented file, and that file is written into
// the cache as `.part` while phones can already read it. The finished file is
// fsynced and renamed into place, so a crash leaves no valid-looking fragment.

import fs from "node:fs/promises";
import path from "node:path";
import { pipeline } from "node:stream/promises";
import { killTree, ProcessError, startProcess } from "./proc.mjs";

export const MAX_SOURCE_LENGTH = 512;
const YOUTUBE_SOURCE = /^youtube:video:([A-Za-z0-9_-]{11})$/;
const SOUNDCLOUD_URI = /^soundcloud:([A-Za-z0-9][A-Za-z0-9._-]*(?:\/[A-Za-z0-9][A-Za-z0-9._-]*){0,4})$/;
const SOUNDCLOUD_HOSTS = new Set(["soundcloud.com", "www.soundcloud.com", "m.soundcloud.com"]);
const SOUNDCLOUD_PATH = /^\/([A-Za-z0-9][A-Za-z0-9._-]*(?:\/[A-Za-z0-9][A-Za-z0-9._-]*){0,4})\/?$/;

export class ResolveError extends Error {
  constructor(code, status = 400, cause) {
    super(code);
    this.name = "ResolveError";
    this.code = code;
    this.status = status;
    if (cause !== undefined) this.cause = cause;
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
export function buildYtDlpArgs({ url }) {
  if (typeof url !== "string" || !url.startsWith("https://")) throw new ResolveError("invalid_source");
  return [
    "--no-playlist",
    "--no-progress",
    "--no-warnings",
    "--no-continue",
    "--no-part",
    "--no-cache-dir",
    "--no-mtime",
    "-f",
    "bestaudio[ext=m4a]/140/bestaudio",
    "--output",
    "-",
    "--",
    url,
  ];
}

const AUDIO_KINDS = Object.freeze({
  m4a: {
    contentType: "audio/mp4",
    // Fragmented MP4 puts the moov atom first, so the first chunk is playable.
    args: ["-movflags", "frag_keyframe+empty_moov+default_base_moof", "-f", "mp4"],
  },
  mp3: { contentType: "audio/mpeg", args: ["-f", "mp3"] },
  ogg: { contentType: "audio/ogg", args: ["-f", "ogg"] },
  flac: { contentType: "audio/flac", args: ["-f", "flac"] },
});

/** Remux only: -c:a copy. The container follows the source's own audio. */
export function buildFfmpegArgs({ format }) {
  const kind = AUDIO_KINDS[format];
  if (!kind) throw new ResolveError("invalid_source");
  return [
    "-nostdin",
    "-hide_banner",
    "-loglevel",
    "error",
    "-i",
    "pipe:0",
    "-map",
    "0:a:0",
    "-vn",
    "-c:a",
    "copy",
    ...kind.args,
    "pipe:1",
  ];
}

/** Maps the first bytes of the download to a container ffmpeg can copy into. */
export function audioFromHead(head) {
  const bytes = Buffer.isBuffer(head) ? head : Buffer.alloc(0);
  if (bytes.length >= 8 && bytes.toString("latin1", 4, 8) === "ftyp") return "m4a";
  if (bytes.length >= 3 && bytes.toString("latin1", 0, 3) === "ID3") return "mp3";
  if (bytes.length >= 2 && bytes[0] === 0xff && (bytes[1] & 0xe0) === 0xe0) return "mp3";
  if (bytes.length >= 4 && bytes.toString("latin1", 0, 4) === "OggS") return "ogg";
  if (bytes.length >= 4 && bytes.toString("latin1", 0, 4) === "fLaC") return "flac";
  // WebM/Matroska (usually Opus): an Ogg container is playable everywhere this
  // service is used.
  if (bytes.length >= 4 && bytes[0] === 0x1a && bytes[1] === 0x45 && bytes[2] === 0xdf && bytes[3] === 0xa3) {
    return "ogg";
  }
  return "m4a";
}

function readHead(stream, size) {
  return new Promise((resolve, reject) => {
    let buffer = Buffer.alloc(0);
    let done = false;
    const cleanup = () => {
      stream.off("data", onData);
      stream.off("end", onEnd);
      stream.off("error", onError);
    };
    const onData = (chunk) => {
      if (done) return;
      buffer = Buffer.concat([buffer, chunk]);
      if (buffer.length < size) return;
      done = true;
      cleanup();
      stream.pause();
      const extra = buffer.subarray(size);
      if (extra.length > 0) stream.unshift(extra);
      resolve(buffer.subarray(0, size));
    };
    const onEnd = () => {
      if (done) return;
      done = true;
      cleanup();
      resolve(buffer);
    };
    const onError = (error) => {
      if (done) return;
      done = true;
      cleanup();
      reject(error);
    };
    stream.on("data", onData);
    stream.once("end", onEnd);
    stream.once("error", onError);
  });
}

async function writeAll(handle, chunk) {
  let offset = 0;
  while (offset < chunk.length) {
    const { bytesWritten } = await handle.write(chunk, offset, chunk.length - offset);
    if (bytesWritten <= 0) throw new Error("short_write");
    offset += bytesWritten;
  }
}

const CANCELED = () => new ResolveError("cancelled", 499);

export class ResolveService {
  #cache;
  #config;
  #logger;
  #spawn;
  #jobs = new Map();
  #foreground = [];
  #background = [];
  #running = new Set();
  #active = 0;
  #children = new Set();
  #closing = false;
  #warmMax;

  constructor({ cache, config, logger, spawn }) {
    this.#cache = cache;
    this.#config = config;
    this.#logger = logger;
    this.#spawn = spawn;
    this.#warmMax = config.warmQueueMax ?? 4;
  }

  get concurrency() {
    return Math.min(2, this.#config.resolveConcurrency ?? 2);
  }

  /** Removes abandoned temp directories from an older, pre-streaming build. */
  async init() {
    const dir = path.join(this.#config.dataDir, "tmp");
    await fs.mkdir(dir, { recursive: true, mode: 0o750 }).catch(() => {});
    const names = await fs.readdir(dir).catch(() => []);
    const now = Date.now();
    for (const name of names) {
      if (!name.startsWith("encore-edge-")) continue;
      const target = path.join(dir, name);
      try {
        const stat = await fs.stat(target);
        if (now - stat.mtimeMs > 60 * 60 * 1000) {
          await fs.rm(target, { recursive: true, force: true });
        }
      } catch {
        // Already gone or busy; the next start sweeps it.
      }
    }
    return this;
  }

  stats() {
    return {
      running: this.#active,
      queued: this.#foreground.length + this.#background.length,
      warmQueued: this.#background.length,
      jobs: this.#jobs.size,
    };
  }

  /**
   * Starts (or joins) a resolve and returns immediately. `started` settles when
   * the `.part` file exists and can be served; `done` settles when the file is
   * complete. `release` drops this caller's interest: a job with no other
   * caller is killed, so a phone that skips a track does not hold the slot.
   */
  acquire(sub, source, { signal } = {}) {
    if (this.#closing) throw new ResolveError("shutting_down", 503);
    const parsed = parseSource(source);
    const key = this.#cache.keyFor(sub, parsed.sourceUri);
    const finished = this.#cache.get(key);
    if (finished) {
      this.#cache.touch(key);
      const view = this.#view(finished, true);
      return { started: Promise.resolve(view), done: Promise.resolve(view), release() {} };
    }
    let job = this.#jobs.get(key);
    if (!job) {
      job = this.#newJob(key, sub, parsed);
      this.#jobs.set(key, job);
      this.#foreground.push(job);
      this.#maybePreempt();
      this.#pump();
    } else if (job.background) {
      // A user tap turns an in-flight prefetch into foreground work.
      job.background = false;
      this.#maybePreempt();
    }
    return this.#attach(job, signal);
  }

  /** Resolves a source completely, sharing one job between identical requests. */
  ensure(sub, source, options) {
    const handle = this.acquire(sub, source, options);
    // Only `done` is awaited here; a cancellation before the first byte must
    // not surface as an unhandled rejection on the unused `started` promise.
    handle.started.catch(() => {});
    return handle.done.finally(() => handle.release());
  }

  #newJob(key, sub, parsed) {
    const job = {
      key,
      sub,
      parsed,
      background: false,
      clients: 0,
      started: false,
      startedView: null,
      streaming: false,
      settled: false,
      cancelled: false,
      controller: new AbortController(),
      startedWaiters: new Set(),
      doneWaiters: new Set(),
    };
    return job;
  }

  #attach(job, signal) {
    job.clients += 1;
    let startedSettled = false;
    let doneSettled = false;
    let released = false;
    const local = {
      started: null,
      done: null,
    };
    const settleStarted = (value, error) => {
      if (startedSettled) return;
      startedSettled = true;
      job.startedWaiters.delete(startedWaiter);
      if (error) local.startedReject(error);
      else local.startedResolve(value);
    };
    const settleDone = (value, error) => {
      if (doneSettled) return;
      doneSettled = true;
      job.doneWaiters.delete(doneWaiter);
      if (error) local.doneReject(error);
      else local.doneResolve(value);
    };
    const startedWaiter = (value, error) => settleStarted(value, error);
    const doneWaiter = (value, error) => settleDone(value, error);
    local.started = new Promise((resolve, reject) => {
      local.startedResolve = resolve;
      local.startedReject = reject;
    });
    local.done = new Promise((resolve, reject) => {
      local.doneResolve = resolve;
      local.doneReject = reject;
    });
    job.startedWaiters.add(startedWaiter);
    job.doneWaiters.add(doneWaiter);
    local.started.catch(() => {});
    local.done.catch(() => {});
    if (job.startedView) settleStarted(job.startedView);

    const release = () => {
      if (released) return;
      released = true;
      job.clients -= 1;
      // A streaming URL has been handed out: the download must finish so the
      // cache is ready for seeking, even if this response has already closed.
      if (job.clients <= 0 && !job.streaming) this.#cancel(job);
    };
    const onAbort = () => {
      const error = CANCELED();
      settleStarted(null, error);
      settleDone(null, error);
      release();
    };
    if (signal) {
      if (signal.aborted) onAbort();
      else signal.addEventListener("abort", onAbort, { once: true });
    }
    return { started: local.started, done: local.done, release };
  }

  #jobStarted(job, view) {
    if (job.settled || job.cancelled) return;
    job.started = true;
    job.startedView = view;
    job.streaming = true;
    for (const waiter of [...job.startedWaiters]) {
      try {
        waiter(view, null);
      } catch {
        // The caller is gone.
      }
    }
    job.startedWaiters.clear();
  }

  #jobSucceeded(job, value) {
    if (job.settled) return;
    job.settled = true;
    this.#running.delete(job);
    if (this.#jobs.get(job.key) === job) this.#jobs.delete(job.key);
    for (const waiter of [...job.doneWaiters]) {
      try {
        waiter(value, null);
      } catch {
        // The caller is gone.
      }
    }
    job.doneWaiters.clear();
    this.#pump();
  }

  #jobFailed(job, error) {
    if (job.settled) return;
    job.settled = true;
    this.#running.delete(job);
    if (this.#jobs.get(job.key) === job) this.#jobs.delete(job.key);
    for (const waiter of [...job.startedWaiters]) {
      try {
        waiter(null, error);
      } catch {
        // The caller is gone.
      }
    }
    job.startedWaiters.clear();
    for (const waiter of [...job.doneWaiters]) {
      try {
        waiter(null, error);
      } catch {
        // The caller is gone.
      }
    }
    job.doneWaiters.clear();
    this.#pump();
  }

  #cancel(job) {
    if (job.settled || job.cancelled) return;
    job.cancelled = true;
    job.controller.abort();
    this.#removeQueued(job);
    this.#jobFailed(job, CANCELED());
  }

  #removeQueued(job) {
    const foreground = this.#foreground.indexOf(job);
    if (foreground >= 0) this.#foreground.splice(foreground, 1);
    const background = this.#background.indexOf(job);
    if (background >= 0) this.#background.splice(background, 1);
  }

  #maybePreempt() {
    if (this.#foreground.length === 0) return;
    for (const job of this.#running) {
      if (job.background && !job.streaming && !job.cancelled) {
        this.#cancel(job);
        if (this.concurrency === 1) return;
      }
    }
  }

  /** Joins (or starts) a prefetch without waiting for it. */
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
      if (this.#cache.get(key)) {
        this.#cache.touch(key);
        warmed.push({ key, state: "cached" });
        continue;
      }
      if (this.#background.length >= this.#warmMax) {
        warmed.push({ key, state: "dropped" });
        continue;
      }
      let job = this.#jobs.get(key);
      if (!job) {
        job = this.#newJob(key, sub, parsed);
        job.background = true;
        this.#jobs.set(key, job);
        this.#background.push(job);
        this.#pump();
      }
      warmed.push({ key, state: "queued" });
    }
    return { warmed, rejected: rejected.length };
  }

  #view(entry, cached) {
    return {
      key: entry.key,
      contentType: entry.contentType,
      size: entry.size,
      cached,
      streaming: entry.complete === false,
    };
  }

  #pump() {
    while (!this.#closing && this.#active < this.concurrency) {
      const job = this.#nextJob();
      if (!job) break;
      if (job.cancelled || job.settled) continue;
      this.#active += 1;
      this.#running.add(job);
      this.#run(job).then(
        (value) => this.#jobSucceeded(job, value),
        (error) => this.#jobFailed(job, error),
      ).finally(() => {
        this.#active -= 1;
        this.#running.delete(job);
        this.#pump();
      });
    }
  }

  #nextJob() {
    return this.#foreground.shift() ?? this.#background.shift() ?? null;
  }

  async #run(job) {
    const signal = job.controller.signal;
    let pending = null;
    let handle;
    let downloader = null;
    let remuxer = null;
    try {
      downloader = startProcess(this.#config.ytdlpBin, buildYtDlpArgs({ url: job.parsed.url }), {
        timeoutMs: this.#config.resolveTimeoutMs,
        spawn: this.#spawn,
        signal,
        onSpawn: (child) => this.#trackChild(child),
      });
      if (!downloader.child) throw new ResolveError("resolve_failed", 502);
      downloader.done.catch(() => {});
      const ended = Symbol("ended");
      const head = await Promise.race([
        readHead(downloader.child.stdout, 12),
        downloader.done.then(() => ended, (error) => Promise.reject(error)),
      ]);
      if (head === ended || head.length === 0) {
        await downloader.done.catch(() => {});
        throw new ResolveError("resolve_failed", 502);
      }
      const format = audioFromHead(head);
      // A partial file from a cancelled job can never be resumed; start clean.
      if (this.#cache.pending(job.key)) await this.#cache.abort(job.key);
      pending = await this.#cache.begin(job.key, { contentType: AUDIO_KINDS[format].contentType });
      // The file exists before any URL is handed out, so the first media read
      // only has to wait for bytes, never for a name.
      handle = await fs.open(pending.file, "wx", 0o600);
      this.#jobStarted(job, this.#view({ ...pending, size: 0 }, false));

      remuxer = startProcess(this.#config.ffmpegBin, buildFfmpegArgs({ format }), {
        timeoutMs: this.#config.ffmpegTimeoutMs,
        spawn: this.#spawn,
        signal,
        stdio: ["pipe", "pipe", "pipe"],
        onSpawn: (child) => this.#trackChild(child),
      });
      if (!remuxer.child) throw new ResolveError("resolve_failed", 502);
      remuxer.done.catch(() => {});
      remuxer.child.stdin.on("error", () => {});
      // The sniffed bytes are part of the stream too: ffmpeg sees them before
      // the rest of stdout is piped in.
      remuxer.child.stdin.write(head);
      const copied = pipeline(downloader.child.stdout, remuxer.child.stdin).catch(() => {});

      let written = 0;
      for await (const chunk of remuxer.child.stdout) {
        await writeAll(handle, chunk);
        written += chunk.length;
        this.#cache.progress(job.key, written);
      }
      await handle.sync();
      await handle.close();
      handle = undefined;
      const results = await Promise.allSettled([downloader.done, remuxer.done, copied]);
      const failed = results.find((result) => result.status === "rejected");
      if (failed) throw failed.reason;
      if (written === 0) throw new ResolveError("resolve_failed", 502);
      const entry = await this.#cache.complete(job.key);
      return this.#view(entry, false);
    } catch (error) {
      await handle?.close().catch(() => {});
      killTree(downloader?.child);
      killTree(remuxer?.child);
      if (pending) await this.#cache.abort(job.key, error).catch(() => {});
      if (signal.aborted || error instanceof ProcessError && error.code === "cancelled") {
        throw CANCELED();
      }
      if (error instanceof ResolveError) throw error;
      this.#logger?.warn("resolve_failed", { code: error?.code ?? "error" });
      throw new ResolveError("resolve_failed", 502, error);
    }
  }

  #trackChild(child) {
    this.#children.add(child);
    child.once("close", () => this.#children.delete(child));
  }

  async close() {
    this.#closing = true;
    const error = new ResolveError("shutting_down", 503);
    for (const job of [...this.#jobs.values()]) {
      job.cancelled = true;
      job.controller.abort();
      this.#jobFailed(job, error);
    }
    this.#foreground.splice(0);
    this.#background.splice(0);
    for (const child of this.#children) killTree(child);
    this.#children.clear();
  }
}
