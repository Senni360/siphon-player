// SoundCloud metadata. Public track and playlist search works without an
// account; likes and playlists need the OAuth token a listener stored in the
// vault. This follows the Rust app's endpoints and public client-ID discovery.
// Audio and private links are never touched or logged.

import { TtlCache } from "./ttlcache.mjs";

const API = "https://api-v2.soundcloud.com";
const HOME = "https://soundcloud.com/";
const MAX_LIMIT = 50;
const MAX_RETRY_AFTER_SECONDS = 3600;

export class SoundCloudError extends Error {
  constructor(code, status = 400, { retryAfter } = {}) {
    super(code);
    this.name = "SoundCloudError";
    this.code = code;
    this.status = status;
    if (Number.isSafeInteger(retryAfter) && retryAfter > 0) this.retryAfter = retryAfter;
  }
}

function parseRetryAfter(value, now = Date.now()) {
  if (typeof value !== "string" || value.length === 0) return 1;
  const seconds = Number(value);
  if (Number.isFinite(seconds) && seconds >= 0) {
    return Math.min(MAX_RETRY_AFTER_SECONDS, Math.max(1, Math.ceil(seconds)));
  }
  const date = Date.parse(value);
  if (!Number.isNaN(date)) {
    return Math.min(MAX_RETRY_AFTER_SECONDS, Math.max(1, Math.ceil((date - now) / 1000)));
  }
  return 1;
}

function clampLimit(value, fallback = MAX_LIMIT) {
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1) return fallback;
  return Math.min(MAX_LIMIT, parsed);
}

function allowedAssetHost(host) {
  return host === "soundcloud.com" || host.endsWith(".soundcloud.com")
    || host === "sndcdn.com" || host.endsWith(".sndcdn.com");
}

export function scriptAssets(html) {
  const assets = [];
  const pattern = /<script\b[^>]*\bsrc\s*=\s*["']([^"']+)["']/gi;
  for (const match of String(html).matchAll(pattern)) {
    const src = match[1].replaceAll("&amp;", "&");
    let url;
    try {
      url = new URL(src, HOME);
    } catch {
      continue;
    }
    if (url.protocol !== "https:" || url.username !== "" || url.password !== "" || !allowedAssetHost(url.hostname)) {
      continue;
    }
    assets.push(url.toString());
  }
  return assets;
}

export function extractClientId(script) {
  const pattern = /(?<![A-Za-z0-9_$])client_id\s*:\s*"([A-Za-z0-9]{32})"/g;
  for (const match of String(script).matchAll(pattern)) return match[1];
  return null;
}

function permalinkPath(value) {
  if (typeof value !== "string") return null;
  let url;
  try {
    url = new URL(value);
  } catch {
    return null;
  }
  if (url.protocol !== "https:" || url.username !== "" || url.password !== "") return null;
  if (!allowedAssetHost(url.hostname) || url.hostname.startsWith("api")) return null;
  return url.pathname.replace(/^\/+/, "").replace(/\/+$/, "");
}

export function soundcloudTrackUri(track) {
  const path = permalinkPath(track?.permalink_url);
  if (path && /^[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*$/.test(path)) {
    return `soundcloud:${path}`;
  }
  return `soundcloud:track:${String(track?.id ?? "")}`;
}

export function mapSoundcloudTrack(track) {
  if (!track || typeof track !== "object" || track.id === undefined || track.id === null) return null;
  const user = track.user && typeof track.user === "object" ? track.user : null;
  return {
    source: "soundcloud",
    id: String(track.id),
    uri: soundcloudTrackUri(track),
    name: typeof track.title === "string" ? track.title : "",
    artists: user && user.id !== undefined
      ? [{ id: String(user.id), name: typeof user.username === "string" ? user.username : "" }]
      : [],
    album: null,
    image: typeof track.artwork_url === "string" ? track.artwork_url : null,
    duration_ms: Number.isFinite(track.duration) ? track.duration : null,
    permalink: typeof track.permalink_url === "string" ? track.permalink_url : null,
  };
}

export function mapSoundcloudPlaylist(playlist) {
  if (!playlist || typeof playlist !== "object" || playlist.id === undefined || playlist.id === null) return null;
  const user = playlist.user && typeof playlist.user === "object" ? playlist.user : null;
  return {
    source: "soundcloud",
    id: String(playlist.id),
    uri: `soundcloud:playlist:${String(playlist.id)}`,
    name: typeof playlist.title === "string" ? playlist.title : "",
    image: typeof playlist.artwork_url === "string" ? playlist.artwork_url : null,
    track_count: Number.isFinite(playlist.track_count) ? playlist.track_count : null,
    owner: user && user.id !== undefined
      ? { id: String(user.id), name: typeof user.username === "string" ? user.username : "" }
      : null,
    is_album: playlist.is_album === true,
    permalink: typeof playlist.permalink_url === "string" ? playlist.permalink_url : null,
  };
}

export class SoundCloudClient {
  #fetch;
  #now;
  #logger;
  #timeoutMs;
  #cacheTtlMs;
  #cache;
  #clientId = null;
  #discovery = null;

  constructor({
    fetch: fetchImpl = globalThis.fetch,
    now = Date.now,
    logger,
    cacheTtlMs = 60_000,
    timeoutMs = 15_000,
    cacheMaxEntries = 500,
  } = {}) {
    this.#fetch = fetchImpl;
    this.#now = now;
    this.#logger = logger;
    this.#timeoutMs = timeoutMs;
    this.#cacheTtlMs = cacheTtlMs;
    this.#cache = new TtlCache({ ttlMs: cacheTtlMs, maxEntries: cacheMaxEntries, now });
  }

  clearCache() {
    this.#cache.clear();
  }

  async #fetchWithTimeout(url, init) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.#timeoutMs);
    timer.unref?.();
    try {
      return await this.#fetch(url, { ...init, signal: controller.signal });
    } catch (error) {
      if (error?.name === "AbortError") throw new SoundCloudError("soundcloud_timeout", 504);
      throw new SoundCloudError("soundcloud_network", 502);
    } finally {
      clearTimeout(timer);
    }
  }

  /** One discovery runs at a time; a forced call replaces a rejected ID. */
  async clientId(force = false) {
    if (!force && this.#clientId) return this.#clientId;
    if (this.#discovery) return this.#discovery;
    this.#discovery = this.#discover().finally(() => {
      this.#discovery = null;
    });
    return this.#discovery;
  }

  async #discover() {
    const home = await this.#fetchWithTimeout(HOME, {
      headers: { accept: "text/html", "user-agent": "Mozilla/5.0" },
    });
    if (!home.ok) throw new SoundCloudError("soundcloud_unavailable", 502);
    let html;
    try {
      html = await home.text();
    } catch {
      throw new SoundCloudError("soundcloud_unavailable", 502);
    }
    // The app's scripts are listed in load order; the client ID tends to be in
    // a later bundle, so try them from the end first.
    for (const asset of scriptAssets(html).reverse()) {
      let script;
      try {
        const response = await this.#fetchWithTimeout(asset, { headers: { accept: "*/*" } });
        if (!response.ok) continue;
        script = await response.text();
      } catch {
        continue;
      }
      const id = extractClientId(script);
      if (id) {
        this.#clientId = id;
        this.#logger?.debug("soundcloud_client_id_refreshed");
        return id;
      }
    }
    throw new SoundCloudError("soundcloud_client_id_unavailable", 502);
  }

  #cacheKey(scope, url) {
    return `${scope}\0${url.toString()}`;
  }

  async #request(path, { query = {}, token, ttlMs = this.#cacheTtlMs, scope = "public", cache = true } = {}, attempt = 0) {
    const url = new URL(`${API}${path}`);
    for (const [key, value] of Object.entries(query)) {
      if (value === undefined || value === null) continue;
      url.searchParams.set(key, String(value));
    }
    const key = this.#cacheKey(scope, url);
    if (cache) {
      const cached = this.#cache.get(key);
      if (cached !== undefined) return cached;
    }

    const clientId = await this.clientId();
    url.searchParams.set("client_id", clientId);
    const headers = { accept: "application/json" };
    if (token) headers.authorization = `OAuth ${token}`;
    const response = await this.#fetchWithTimeout(url, { headers });

    if ((response.status === 401 || response.status === 403) && !token && attempt === 0) {
      // A stale public client ID looks like a rejected session. Discover once
      // more, then retry the same catalogue read anonymously.
      await this.clientId(true);
      return this.#request(path, { query, token, ttlMs, scope, cache }, 1);
    }
    if (response.status === 429) {
      const retryAfter = parseRetryAfter(response?.headers?.get?.("retry-after"), this.#now());
      throw new SoundCloudError("soundcloud_rate_limited", 429, { retryAfter });
    }
    if (!response.ok) {
      if (response.status === 404) throw new SoundCloudError("soundcloud_not_found", 404);
      if (response.status === 401 || response.status === 403) {
        throw new SoundCloudError("soundcloud_sign_in_expired", 401);
      }
      this.#logger?.warn("soundcloud_request_failed", { status: response.status });
      throw new SoundCloudError("soundcloud_request_failed", 502);
    }
    let page;
    try {
      page = await response.json();
    } catch {
      throw new SoundCloudError("soundcloud_bad_response", 502);
    }
    if (cache) this.#cache.set(key, page, ttlMs);
    return page;
  }

  async searchTracks(query, { limit = MAX_LIMIT, offset = 0, scope = "public", ttlMs = this.#cacheTtlMs } = {}) {
    const page = await this.#request("/search/tracks", {
      query: {
        q: query,
        limit: clampLimit(limit),
        offset: Math.max(0, offset),
        linked_partitioning: "1",
      },
      ttlMs,
      scope,
    });
    return (Array.isArray(page?.collection) ? page.collection : []).map(mapSoundcloudTrack).filter(Boolean);
  }

  async searchPlaylists(query, { limit = MAX_LIMIT, offset = 0, scope = "public", ttlMs = this.#cacheTtlMs } = {}) {
    const page = await this.#request("/search/playlists", {
      query: {
        q: query,
        limit: clampLimit(limit),
        offset: Math.max(0, offset),
        linked_partitioning: "1",
      },
      ttlMs,
      scope,
    });
    return (Array.isArray(page?.collection) ? page.collection : []).map(mapSoundcloudPlaylist).filter(Boolean);
  }

  async me(token) {
    const user = await this.#request("/me", { token, scope: "me", cache: false });
    return user;
  }

  async #userId(token) {
    const me = await this.me(token);
    const id = me?.id;
    if (typeof id !== "number" && typeof id !== "string") throw new SoundCloudError("soundcloud_sign_in_expired", 401);
    const text = String(id);
    if (!/^\d{1,20}$/.test(text)) throw new SoundCloudError("soundcloud_sign_in_expired", 401);
    return text;
  }

  async likes(token, { limit = MAX_LIMIT, offset = 0, scope = "me", ttlMs = this.#cacheTtlMs } = {}) {
    const id = await this.#userId(token);
    const page = await this.#request(`/users/${id}/track_likes`, {
      query: { limit: clampLimit(limit), offset: Math.max(0, offset), linked_partitioning: "1" },
      token,
      ttlMs,
      scope,
    });
    return (Array.isArray(page?.collection) ? page.collection : [])
      .map((item) => mapSoundcloudTrack(item?.track))
      .filter(Boolean);
  }

  async playlists(token, { limit = MAX_LIMIT, offset = 0, scope = "me", ttlMs = this.#cacheTtlMs } = {}) {
    const id = await this.#userId(token);
    const page = await this.#request(`/users/${id}/playlists/liked_and_owned`, {
      query: { limit: clampLimit(limit), offset: Math.max(0, offset), linked_partitioning: "1" },
      token,
      ttlMs,
      scope,
    });
    return (Array.isArray(page?.collection) ? page.collection : []).map(mapSoundcloudPlaylist).filter(Boolean);
  }

  static limit(value, fallback = MAX_LIMIT) {
    return clampLimit(value, fallback);
  }
}
