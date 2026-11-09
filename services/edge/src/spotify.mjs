// Spotify Web API access with the listener's own developer app. Only metadata
// endpoints are used; audio is never requested here.
//
// The refresh token stays in the vault. Access tokens live in memory for the
// life of the process. A refresh rotates the saved refresh token atomically,
// one refresh runs per listener at a time, and a Retry-After answer pauses
// that listener's requests until the wait ends.

import { TtlCache } from "./ttlcache.mjs";

const TOKEN_URL = "https://accounts.spotify.com/api/token";
const API_BASE = "https://api.spotify.com/v1";
const MAX_LIMIT = 50;
const EARLY_REFRESH_MS = 30_000;
const MAX_RETRY_AFTER_SECONDS = 3600;

export class SpotifyError extends Error {
  constructor(code, status = 400, { retryAfter } = {}) {
    super(code);
    this.name = "SpotifyError";
    this.code = code;
    this.status = status;
    if (Number.isSafeInteger(retryAfter) && retryAfter > 0) this.retryAfter = retryAfter;
  }
}

export function parseRetryAfter(value, now = Date.now()) {
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

function statusError(status) {
  if (status === 404) return new SpotifyError("spotify_not_found", 404);
  if (status === 403) return new SpotifyError("spotify_forbidden", 403);
  if (status === 401) return new SpotifyError("spotify_unauthorized", 401);
  if (status === 429) return new SpotifyError("spotify_rate_limited", 429);
  if (status >= 500) return new SpotifyError("spotify_unavailable", 502);
  return new SpotifyError("spotify_request_failed", 400);
}

function clampLimit(value, fallback = MAX_LIMIT) {
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1) return fallback;
  return Math.min(MAX_LIMIT, parsed);
}

export class SpotifyClient {
  #vault;
  #fetch;
  #now;
  #logger;
  #timeoutMs;
  #cacheTtlMs;
  #cache;
  #tokens = new Map();
  #refreshes = new Map();
  #backoff = new Map();

  constructor({
    vault,
    fetch: fetchImpl = globalThis.fetch,
    now = Date.now,
    logger,
    cacheTtlMs = 60_000,
    timeoutMs = 15_000,
    cacheMaxEntries = 500,
  } = {}) {
    this.#vault = vault;
    this.#fetch = fetchImpl;
    this.#now = now;
    this.#logger = logger;
    this.#timeoutMs = timeoutMs;
    this.#cacheTtlMs = cacheTtlMs;
    this.#cache = new TtlCache({ ttlMs: cacheTtlMs, maxEntries: cacheMaxEntries, now });
  }

  /** Drops the in-memory access token, for instance after Spotify refused it. */
  forgetTokens(sub) {
    this.#tokens.delete(sub);
  }

  clearCache() {
    this.#cache.clear();
  }

  retryAfter(sub) {
    const until = this.#backoff.get(sub);
    if (!until) return 0;
    const remaining = until - this.#now();
    if (remaining <= 0) {
      this.#backoff.delete(sub);
      return 0;
    }
    return Math.ceil(remaining / 1000);
  }

  #assertBackoff(sub) {
    const retryAfter = this.retryAfter(sub);
    if (retryAfter > 0) throw new SpotifyError("spotify_rate_limited", 429, { retryAfter });
  }

  #setBackoff(sub, response) {
    const retryAfter = parseRetryAfter(response?.headers?.get?.("retry-after"), this.#now());
    this.#backoff.set(sub, this.#now() + retryAfter * 1000);
    return retryAfter;
  }

  async #fetchWithTimeout(url, init) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.#timeoutMs);
    timer.unref?.();
    try {
      return await this.#fetch(url, { ...init, signal: controller.signal });
    } catch (error) {
      if (error?.name === "AbortError") throw new SpotifyError("spotify_timeout", 504);
      throw new SpotifyError("spotify_network", 502);
    } finally {
      clearTimeout(timer);
    }
  }

  /**
   * Returns a usable access token. Concurrent callers share one refresh so a
   * phone opening two screens at once spends one token request, not two.
   */
  async accessToken(sub) {
    const current = this.#tokens.get(sub);
    if (current && current.expiresAt - EARLY_REFRESH_MS > this.#now()) return current.token;

    const running = this.#refreshes.get(sub);
    if (running) return (await running).token;

    const refresh = this.#refresh(sub).finally(() => {
      if (this.#refreshes.get(sub) === refresh) this.#refreshes.delete(sub);
    });
    this.#refreshes.set(sub, refresh);
    return (await refresh).token;
  }

  async #refresh(sub) {
    this.#assertBackoff(sub);
    const stored = await this.#vault.getSpotify(sub);
    if (!stored) throw new SpotifyError("spotify_not_linked", 409);

    const body = new URLSearchParams({
      grant_type: "refresh_token",
      refresh_token: stored.refreshToken,
      client_id: stored.clientId,
    });
    const response = await this.#fetchWithTimeout(TOKEN_URL, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded", accept: "application/json" },
      body: body.toString(),
    });

    if (response.status === 429) {
      const retryAfter = this.#setBackoff(sub, response);
      throw new SpotifyError("spotify_rate_limited", 429, { retryAfter });
    }
    if (!response.ok) {
      if (response.status === 400 || response.status === 401) {
        throw new SpotifyError("spotify_sign_in_expired", 401);
      }
      throw new SpotifyError("spotify_token_failed", 502);
    }

    let payload;
    try {
      payload = await response.json();
    } catch {
      throw new SpotifyError("spotify_bad_response", 502);
    }
    if (!payload || typeof payload.access_token !== "string" || payload.access_token.length === 0) {
      throw new SpotifyError("spotify_bad_response", 502);
    }
    const expiresIn = Number(payload.expires_in);
    const ttl = Number.isFinite(expiresIn) && expiresIn > 0 ? expiresIn : 3600;
    const token = { token: payload.access_token, expiresAt: this.#now() + ttl * 1000 };
    this.#tokens.set(sub, token);

    // Spotify rotates the refresh token on most refreshes. Save it back before
    // this access token is handed out, so a restart never loses the new one.
    if (typeof payload.refresh_token === "string" && payload.refresh_token.length >= 8
      && payload.refresh_token !== stored.refreshToken) {
      await this.#vault.saveSpotifyRefresh(sub, {
        clientId: stored.clientId,
        refreshToken: payload.refresh_token,
      });
    }
    this.#logger?.debug("spotify_token_refreshed");
    return token;
  }

  #cacheKey(sub, url, ttlMs) {
    return `${sub}\0${ttlMs}\0${url.toString()}`;
  }

  /**
   * Read-only Web API call. `pathname` is relative to /v1. Successful reads are
   * kept for the route's TTL so a phone reopening a screen costs no quota.
   */
  async api(sub, pathname, { query = {}, ttlMs = this.#cacheTtlMs } = {}) {
    if (typeof pathname !== "string" || !pathname.startsWith("/") || pathname.includes("..")) {
      throw new SpotifyError("invalid_spotify_path", 500);
    }
    const url = new URL(`${API_BASE}${pathname}`);
    for (const [key, value] of Object.entries(query)) {
      if (value === undefined || value === null) continue;
      url.searchParams.set(key, String(value));
    }
    const key = this.#cacheKey(sub, url, ttlMs);
    const cached = this.#cache.get(key);
    if (cached !== undefined) return cached;

    this.#assertBackoff(sub);
    for (let attempt = 0; attempt < 2; attempt += 1) {
      const token = await this.accessToken(sub);
      const response = await this.#fetchWithTimeout(url, {
        headers: { authorization: `Bearer ${token}`, accept: "application/json" },
      });
      if (response.status === 401 && attempt === 0) {
        this.#tokens.delete(sub);
        continue;
      }
      if (response.status === 429) {
        const retryAfter = this.#setBackoff(sub, response);
        throw new SpotifyError("spotify_rate_limited", 429, { retryAfter });
      }
      if (!response.ok) {
        this.#logger?.warn("spotify_request_failed", {
          path: pathname,
          status: response.status,
          code: statusError(response.status).code,
        });
        throw statusError(response.status);
      }
      let value = null;
      if (response.status !== 204) {
        try {
          value = await response.json();
        } catch {
          throw new SpotifyError("spotify_bad_response", 502);
        }
      }
      this.#cache.set(key, value, ttlMs);
      return value;
    }
    throw new SpotifyError("spotify_unauthorized", 401);
  }

  /** Normalises a caller's limit without ever asking Spotify for more than 50. */
  static limit(value, fallback = MAX_LIMIT) {
    return clampLimit(value, fallback);
  }
}
