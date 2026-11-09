// yt-dlp search plus the conservative matcher. Search results are metadata
// only; this module never asks for audio. The query reaches yt-dlp inside a
// single encoded HTTPS URL after "--", never through a shell.

import { bestMatch, normalizeTrack, score, youtubeUrl } from "./matcher.mjs";
import { runProcess } from "./proc.mjs";

const MAX_QUERY = 300;
const MEMO_LIMIT = 500;

export class MatchError extends Error {
  constructor(code, status = 400) {
    super(code);
    this.name = "MatchError";
    this.code = code;
    this.status = status;
  }
}

function cleanQuery(text) {
  if (typeof text !== "string") return "";
  return text.replace(/[\u0000-\u001f\u007f]/g, " ").trim().slice(0, MAX_QUERY);
}

export function buildYtSearchUrl(query) {
  return `https://www.youtube.com/results?search_query=${encodeURIComponent(cleanQuery(query))}`;
}

export function buildSoundCloudSearchUrl(query) {
  return `scsearch8:${cleanQuery(query)}`;
}

export function buildSearchArgs(url) {
  return [
    "--flat-playlist",
    "--dump-single-json",
    "--skip-download",
    "--no-cache-dir",
    "--no-warnings",
    "--no-progress",
    "--",
    url,
  ];
}

function parseDuration(value) {
  if (typeof value === "number" && Number.isFinite(value) && value > 0) return value;
  if (typeof value === "string") {
    const parts = value.split(":").map(Number);
    if (parts.length >= 1 && parts.length <= 3 && parts.every((part) => Number.isFinite(part) && part >= 0)) {
      return parts.reduce((total, part) => total * 60 + part, 0);
    }
  }
  return null;
}

function soundcloudSource(text) {
  try {
    const url = new URL(text);
    if (url.protocol !== "https:" || url.username !== "" || url.password !== "") return null;
    if (!["soundcloud.com", "www.soundcloud.com", "m.soundcloud.com"].includes(url.hostname)) return null;
    if (url.search !== "" || url.hash !== "") return null;
    const match = /^\/([A-Za-z0-9][A-Za-z0-9._-]*(?:\/[A-Za-z0-9][A-Za-z0-9._-]*){0,4})\/?$/.exec(url.pathname);
    if (!match) return null;
    return `soundcloud:${match[1]}`;
  } catch {
    return null;
  }
}

function youtubeSource(text) {
  if (typeof text === "string" && /^[A-Za-z0-9_-]{11}$/.test(text)) return `youtube:video:${text}`;
  const canonical = youtubeUrl(text);
  if (!canonical) return null;
  const id = new URL(canonical).searchParams.get("v");
  return id ? `youtube:video:${id}` : null;
}

export function candidatesFromProbe(value, provider) {
  const entries = Array.isArray(value?.entries) ? value.entries : Array.isArray(value) ? value : [];
  const candidates = [];
  for (const entry of entries) {
    if (!entry || typeof entry !== "object") continue;
    const title = typeof entry.title === "string" ? entry.title : "";
    if (title.length === 0) continue;
    const web = typeof entry.webpage_url === "string"
      ? entry.webpage_url
      : typeof entry.url === "string"
        ? entry.url
        : "";
    const sourceUri = provider === "soundcloud" ? soundcloudSource(web) : youtubeSource(web);
    if (!sourceUri) continue;
    const duration = parseDuration(entry.duration);
    candidates.push({
      source: provider,
      sourceUri,
      url: sourceUri.startsWith("youtube:video:") ? youtubeUrl(`https://www.youtube.com/watch?v=${sourceUri.slice(14)}`) : undefined,
      title,
      uploader: typeof entry.uploader === "string"
        ? entry.uploader
        : typeof entry.channel === "string"
          ? entry.channel
          : typeof entry.artist === "string"
            ? entry.artist
            : "",
      duration,
      live: entry.is_live === true || entry.live_status === "is_live",
    });
  }
  return candidates;
}

export class MatchService {
  #config;
  #logger;
  #spawn;
  #memo = new Map();

  constructor({ config, logger, spawn }) {
    this.#config = config;
    this.#logger = logger;
    this.#spawn = spawn;
  }

  async #search(url) {
    const { stdout } = await runProcess(this.#config.ytdlpBin, buildSearchArgs(url), {
      timeoutMs: this.#config.searchTimeoutMs,
      spawn: this.#spawn,
    });
    let parsed;
    try {
      parsed = JSON.parse(stdout);
    } catch {
      return [];
    }
    return Array.isArray(parsed) ? parsed : [parsed];
  }

  /**
   * Looks for the same song on YouTube and SoundCloud. Below the Rust gates
   * this returns `{match: null}`; it never guesses.
   */
  async match(sub, track) {
    const wanted = normalizeTrack(track);
    if (!wanted.name || !Number.isFinite(wanted.durationMs) || wanted.durationMs <= 0 || wanted.artists.length === 0) {
      throw new MatchError("invalid_track");
    }
    const id = wanted.uri || `${wanted.name}\0${wanted.durationMs}`;
    const memoKey = `${sub}\0${id}`;
    const remembered = this.#memo.get(memoKey);
    if (remembered) return remembered;

    const query = `${wanted.artists.map((artist) => (typeof artist === "string" ? artist : artist?.name ?? "")).join(", ")} - ${wanted.name}`;
    // SoundCloud only. YouTube's audio is refused from a server, so a YouTube
    // match couldn't play here anyway, and its search takes many seconds.
    // The computer finds and seeds those matches instead.
    const urls = [buildSoundCloudSearchUrl(query)];
    const results = await Promise.allSettled(urls.map((url) => this.#search(url)));
    const candidates = [];
    for (const result of results) {
      if (result.status === "rejected") {
        this.#logger?.warn("search_failed", { provider: "soundcloud", code: result.reason?.code ?? "error" });
        continue;
      }
      for (const value of result.value) {
        candidates.push(...candidatesFromProbe(value, "soundcloud"));
      }
    }

    const candidate = bestMatch(wanted, candidates);
    const result = candidate
      ? {
          match: {
            source: candidate.sourceUri,
            provider: candidate.source,
            score: score(wanted, candidate),
            title: candidate.title,
            uploader: candidate.uploader,
            duration: candidate.duration,
          },
        }
      : { match: null };

    // Remembered for the life of the process, as the desktop app does.
    this.#memo.set(memoKey, result);
    if (this.#memo.size > MEMO_LIMIT) {
      const oldest = this.#memo.keys().next().value;
      if (oldest !== undefined) this.#memo.delete(oldest);
    }
    return result;
  }

  clear() {
    this.#memo.clear();
  }
}
