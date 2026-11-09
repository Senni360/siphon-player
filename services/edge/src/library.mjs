// Read-only library, search and home for the phone. Everything here is
// metadata: Spotify audio is never requested. Mixed playlists come from the
// phone's existing sync snapshot, so they keep working without Spotify.

import { TtlCache } from "./ttlcache.mjs";

const MAX_LIMIT = 50;
const SEARCH_PAGE = 10;
const MAX_CURSOR_LENGTH = 200;
const SPOTIFY_ID = /^[A-Za-z0-9]{1,64}$/;

export class LibraryError extends Error {
  constructor(code, status = 400) {
    super(code);
    this.name = "LibraryError";
    this.code = code;
    this.status = status;
  }
}

export function normalizeLimit(value, fallback = MAX_LIMIT) {
  if (value === undefined || value === null || value === "") return fallback;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1 || parsed > MAX_LIMIT) {
    throw new LibraryError("invalid_limit", 400);
  }
  return parsed;
}

export function encodeCursor(value) {
  return Buffer.from(JSON.stringify(value), "utf8").toString("base64url");
}

export function decodeCursor(raw) {
  if (raw === null || raw === undefined || raw === "") return null;
  if (typeof raw !== "string" || !/^[A-Za-z0-9_-]{1,200}$/.test(raw) || raw.length > MAX_CURSOR_LENGTH) {
    throw new LibraryError("invalid_cursor", 400);
  }
  let value;
  try {
    value = JSON.parse(Buffer.from(raw, "base64url").toString("utf8"));
  } catch {
    throw new LibraryError("invalid_cursor", 400);
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new LibraryError("invalid_cursor", 400);
  return value;
}

function offsetFrom(raw) {
  const value = decodeCursor(raw);
  if (!value) return 0;
  if (!Number.isSafeInteger(value.o) || value.o < 0 || value.o > 1_000_000) {
    throw new LibraryError("invalid_cursor", 400);
  }
  return value.o;
}

function pickImage(images) {
  if (!Array.isArray(images)) return null;
  for (const image of images) {
    if (image && typeof image.url === "string" && image.url.length > 0) return image.url;
  }
  return null;
}

function cleanQuery(value) {
  if (typeof value !== "string") throw new LibraryError("invalid_query", 400);
  const text = value.replace(/[\u0000-\u001f\u007f]/g, " ").trim();
  if (text.length === 0 || text.length > 200) throw new LibraryError("invalid_query", 400);
  return text;
}

function spotifyId(value) {
  if (typeof value !== "string" || !SPOTIFY_ID.test(value)) throw new LibraryError("invalid_id", 400);
  return value;
}

export function mapSpotifyTrack(track) {
  if (!track || typeof track !== "object" || typeof track.id !== "string") return null;
  const album = track.album && typeof track.album === "object" ? track.album : null;
  return {
    source: "spotify",
    uri: typeof track.uri === "string" ? track.uri : `spotify:track:${track.id}`,
    name: typeof track.name === "string" ? track.name : "",
    artists: Array.isArray(track.artists)
      ? track.artists
        .filter((artist) => artist && typeof artist === "object")
        .map((artist) => ({ id: artist.id ?? null, name: typeof artist.name === "string" ? artist.name : "" }))
      : [],
    album: album ? { id: album.id ?? null, name: typeof album.name === "string" ? album.name : "" } : null,
    image: pickImage(track.album?.images),
    duration_ms: Number.isFinite(track.duration_ms) ? track.duration_ms : null,
  };
}

export function mapSpotifyAlbum(album) {
  if (!album || typeof album !== "object" || typeof album.id !== "string") return null;
  return {
    source: "spotify",
    id: album.id,
    uri: typeof album.uri === "string" ? album.uri : `spotify:album:${album.id}`,
    name: typeof album.name === "string" ? album.name : "",
    image: pickImage(album.images),
    artists: Array.isArray(album.artists)
      ? album.artists
        .filter((artist) => artist && typeof artist === "object")
        .map((artist) => ({ id: artist.id ?? null, name: typeof artist.name === "string" ? artist.name : "" }))
      : [],
    release_date: typeof album.release_date === "string" ? album.release_date : null,
    total_tracks: Number.isFinite(album.total_tracks) ? album.total_tracks : null,
    album_type: typeof album.album_type === "string" ? album.album_type : null,
  };
}

export function mapSpotifyArtist(artist) {
  if (!artist || typeof artist !== "object" || typeof artist.id !== "string") return null;
  return {
    source: "spotify",
    id: artist.id,
    uri: typeof artist.uri === "string" ? artist.uri : `spotify:artist:${artist.id}`,
    name: typeof artist.name === "string" ? artist.name : "",
    image: pickImage(artist.images),
    genres: Array.isArray(artist.genres) ? artist.genres.filter((genre) => typeof genre === "string") : [],
    followers: Number.isFinite(artist.followers?.total) ? artist.followers.total : null,
  };
}

export function mapSpotifyPlaylist(playlist) {
  if (!playlist || typeof playlist !== "object" || typeof playlist.id !== "string") return null;
  const owner = playlist.owner && typeof playlist.owner === "object" ? playlist.owner : null;
  return {
    source: "spotify",
    id: playlist.id,
    uri: typeof playlist.uri === "string" ? playlist.uri : `spotify:playlist:${playlist.id}`,
    name: typeof playlist.name === "string" ? playlist.name : "",
    image: pickImage(playlist.images),
    owner: owner ? { id: owner.id ?? null, name: typeof owner.display_name === "string" ? owner.display_name : "" } : null,
    track_count: Number.isFinite(playlist.tracks?.total) ? playlist.tracks.total : null,
    collaborative: playlist.collaborative === true,
  };
}

function playlistEntries(data) {
  if (Array.isArray(data)) return data;
  if (Array.isArray(data?.playlists)) return data.playlists;
  return [];
}

/**
 * The snapshot may carry mixed rows beside the Spotify rows (`playlists`) or
 * in their own `mixed` list, as the desktop app's Library payload does. Both
 * are accepted without guessing that an ordinary Spotify row is mixed.
 */
function mixedCandidates(data) {
  const candidates = [];
  if (Array.isArray(data?.mixed)) {
    for (const playlist of data.mixed) candidates.push({ playlist, explicit: true });
  }
  for (const playlist of playlistEntries(data)) candidates.push({ playlist, explicit: false });
  return candidates;
}

function isMixedCandidate(playlist, explicit) {
  if (!playlist || typeof playlist !== "object" || Array.isArray(playlist)) return false;
  if (playlist.spotify === true) return false;
  if (explicit || playlist.mixed === true || playlist.spotify === false) return true;
  return Array.isArray(playlist.entries);
}

function trackCount(playlist) {
  if (Array.isArray(playlist.tracks)) return playlist.tracks.length;
  if (Array.isArray(playlist.entries)) return playlist.entries.length;
  if (Number.isFinite(playlist.count)) return playlist.count;
  if (Number.isFinite(playlist.track_count)) return playlist.track_count;
  return null;
}

function mixedEntries(playlist) {
  if (Array.isArray(playlist.tracks)) return playlist.tracks;
  if (Array.isArray(playlist.entries)) return playlist.entries;
  return [];
}

export class LibraryService {
  #vault;
  #spotify;
  #soundcloud;
  #sync;
  #config;
  #logger;
  #now;
  #homeCache;

  constructor({ vault, spotify, soundcloud, sync, config, logger, now = Date.now }) {
    this.#vault = vault;
    this.#spotify = spotify;
    this.#soundcloud = soundcloud;
    this.#sync = sync;
    this.#config = config;
    this.#logger = logger;
    this.#now = now;
    this.#homeCache = new TtlCache({ ttlMs: config.spotifyHomeCacheTtlMs, maxEntries: 100, now });
  }

  #require() {
    if (!this.#vault.enabled) throw new LibraryError("vault_not_configured", 503);
  }

  async #mixedPlaylists(sub) {
    const record = await this.#sync.get(sub, "playlists");
    const out = [];
    const seen = new Set();
    for (const { playlist, explicit } of mixedCandidates(record?.data)) {
      if (!isMixedCandidate(playlist, explicit)) continue;
      const id = typeof playlist.id === "string" || typeof playlist.id === "number" ? String(playlist.id) : "";
      if (id.length === 0 || id.length > 128 || seen.has(id)) continue;
      seen.add(id);
      out.push({
        source: "mixed",
        id,
        uri: typeof playlist.uri === "string" ? playlist.uri : `spotifast:playlist:${id}`,
        name: typeof playlist.name === "string" ? playlist.name : "",
        image: typeof playlist.image === "string" ? playlist.image : null,
        track_count: trackCount(playlist),
      });
      if (out.length >= 100) break;
    }
    return out;
  }

  async #mixedPlaylist(sub, id) {
    const record = await this.#sync.get(sub, "playlists");
    for (const { playlist, explicit } of mixedCandidates(record?.data)) {
      if (!isMixedCandidate(playlist, explicit)) continue;
      if (String(playlist.id ?? "") !== id) continue;
      const items = mixedEntries(playlist)
        .map((entry) => {
          if (typeof entry === "string") return { uri: entry };
          if (!entry || typeof entry !== "object") return null;
          // The desktop mixed store wraps each row as { id, track, permalink }.
          const value = entry.track && typeof entry.track === "object" ? entry.track : entry;
          return {
            uri: typeof value.uri === "string" ? value.uri
              : typeof value.sourceUri === "string" ? value.sourceUri : null,
            name: typeof value.name === "string" ? value.name : null,
          };
        })
        .filter(Boolean);
      return {
        v: 1,
        source: "mixed",
        playlist: {
          source: "mixed",
          id,
          uri: typeof playlist.uri === "string" ? playlist.uri : `spotifast:playlist:${id}`,
          name: typeof playlist.name === "string" ? playlist.name : "",
          image: typeof playlist.image === "string" ? playlist.image : null,
          track_count: items.length,
        },
        items,
        next: null,
      };
    }
    return null;
  }

  async playlists(sub, { limit = MAX_LIMIT, cursor = null } = {}) {
    this.#require();
    const pageSize = normalizeLimit(limit);
    const offset = offsetFrom(cursor);
    let page = null;
    let error = null;
    try {
      page = await this.#spotify.api(sub, "/me/playlists", {
        query: { limit: pageSize, offset },
        ttlMs: this.#config.spotifyCacheTtlMs,
      });
    } catch (failure) {
      if (failure.code === "spotify_not_linked" && offset === 0) {
        error = "spotify_not_linked";
      } else {
        throw failure;
      }
    }
    const items = (Array.isArray(page?.items) ? page.items : [])
      .map(mapSpotifyPlaylist)
      .filter(Boolean);
    if (offset === 0) items.push(...(await this.#mixedPlaylists(sub)));
    return {
      v: 1,
      items,
      next: page?.next ? encodeCursor({ o: offset + pageSize }) : null,
      ...(error ? { error } : {}),
    };
  }

  async playlist(sub, id, { limit = MAX_LIMIT, cursor = null } = {}) {
    this.#require();
    const playlistId = spotifyId(id);
    const pageSize = normalizeLimit(limit);
    const offset = offsetFrom(cursor);
    try {
      const [header, tracks] = await Promise.all([
        this.#spotify.api(sub, `/playlists/${playlistId}`, { ttlMs: this.#config.spotifyCacheTtlMs }),
        this.#spotify.api(sub, `/playlists/${playlistId}/tracks`, {
          query: { limit: pageSize, offset },
          ttlMs: this.#config.spotifyCacheTtlMs,
        }),
      ]);
      const items = (Array.isArray(tracks?.items) ? tracks.items : [])
        .map((item) => mapSpotifyTrack(item?.track))
        .filter(Boolean);
      return {
        v: 1,
        playlist: mapSpotifyPlaylist(header),
        items,
        next: tracks?.next ? encodeCursor({ o: offset + pageSize }) : null,
      };
    } catch (failure) {
      if (failure.code === "spotify_not_found" || failure.code === "spotify_not_linked") {
        const mixed = await this.#mixedPlaylist(sub, playlistId);
        if (mixed) return mixed;
      }
      throw failure;
    }
  }

  async liked(sub, { limit = MAX_LIMIT, cursor = null } = {}) {
    this.#require();
    const pageSize = normalizeLimit(limit);
    const offset = offsetFrom(cursor);
    const page = await this.#spotify.api(sub, "/me/tracks", {
      query: { limit: pageSize, offset },
      ttlMs: this.#config.spotifyCacheTtlMs,
    });
    const items = (Array.isArray(page?.items) ? page.items : [])
      .map((item) => mapSpotifyTrack(item?.track))
      .filter(Boolean);
    return { v: 1, items, next: page?.next ? encodeCursor({ o: offset + pageSize }) : null };
  }

  async albums(sub, { limit = MAX_LIMIT, cursor = null } = {}) {
    this.#require();
    const pageSize = normalizeLimit(limit);
    const offset = offsetFrom(cursor);
    const page = await this.#spotify.api(sub, "/me/albums", {
      query: { limit: pageSize, offset },
      ttlMs: this.#config.spotifyCacheTtlMs,
    });
    const items = (Array.isArray(page?.items) ? page.items : [])
      .map((item) => mapSpotifyAlbum(item?.album))
      .filter(Boolean);
    return { v: 1, items, next: page?.next ? encodeCursor({ o: offset + pageSize }) : null };
  }

  async soundcloudLikes(sub, { limit = MAX_LIMIT, cursor = null } = {}) {
    this.#require();
    const stored = await this.#vault.getSoundcloud(sub);
    if (!stored) throw new LibraryError("soundcloud_not_linked", 409);
    const pageSize = normalizeLimit(limit);
    const offset = offsetFrom(cursor);
    const items = await this.#soundcloud.likes(stored.oauthToken, {
      limit: pageSize,
      offset,
      scope: sub,
      ttlMs: this.#config.spotifyCacheTtlMs,
    });
    return {
      v: 1,
      items,
      next: items.length === pageSize ? encodeCursor({ o: offset + pageSize }) : null,
    };
  }

  async soundcloudPlaylists(sub, { limit = MAX_LIMIT, cursor = null } = {}) {
    this.#require();
    const stored = await this.#vault.getSoundcloud(sub);
    if (!stored) throw new LibraryError("soundcloud_not_linked", 409);
    const pageSize = normalizeLimit(limit);
    const offset = offsetFrom(cursor);
    const items = await this.#soundcloud.playlists(stored.oauthToken, {
      limit: pageSize,
      offset,
      scope: sub,
      ttlMs: this.#config.spotifyCacheTtlMs,
    });
    return {
      v: 1,
      items,
      next: items.length === pageSize ? encodeCursor({ o: offset + pageSize }) : null,
    };
  }

  async artists(sub, { limit = MAX_LIMIT, cursor = null } = {}) {
    this.#require();
    const pageSize = normalizeLimit(limit);
    const decoded = decodeCursor(cursor);
    const after = decoded?.a;
    if (after !== undefined && (typeof after !== "string" || !SPOTIFY_ID.test(after))) {
      throw new LibraryError("invalid_cursor", 400);
    }
    const page = await this.#spotify.api(sub, "/me/following", {
      query: { type: "artist", limit: pageSize, ...(after ? { after } : {}) },
      ttlMs: this.#config.spotifyCacheTtlMs,
    });
    const followed = page?.artists;
    const items = (Array.isArray(followed?.items) ? followed.items : [])
      .map(mapSpotifyArtist)
      .filter(Boolean);
    const nextAfter = followed?.cursors?.after;
    return {
      v: 1,
      items,
      next: followed?.next && typeof nextAfter === "string" ? encodeCursor({ a: nextAfter }) : null,
    };
  }

  async search(sub, query, { limit = SEARCH_PAGE, cursor = null } = {}) {
    this.#require();
    const text = cleanQuery(query);
    const pageSize = Math.min(normalizeLimit(limit, SEARCH_PAGE), SEARCH_PAGE);
    const offset = offsetFrom(cursor);
    const ttlMs = this.#config.spotifySearchTtlMs;

    const spotifyTask = this.#spotify.api(sub, "/search", {
      query: { q: text, type: "track,artist,album,playlist", limit: pageSize, offset },
      ttlMs,
    }).then((page) => ({
      tracks: (Array.isArray(page?.tracks?.items) ? page.tracks.items : []).map(mapSpotifyTrack).filter(Boolean),
      artists: (Array.isArray(page?.artists?.items) ? page.artists.items : []).map(mapSpotifyArtist).filter(Boolean),
      albums: (Array.isArray(page?.albums?.items) ? page.albums.items : []).map(mapSpotifyAlbum).filter(Boolean),
      playlists: (Array.isArray(page?.playlists?.items) ? page.playlists.items : []).map(mapSpotifyPlaylist).filter(Boolean),
    })).catch((failure) => ({
      tracks: [],
      artists: [],
      albums: [],
      playlists: [],
      error: failure.code ?? "spotify_failed",
    }));

    const soundcloudTracksTask = this.#soundcloud
      .searchTracks(text, { limit: pageSize, offset, scope: sub, ttlMs })
      .then((items) => ({ items }))
      .catch((failure) => ({ items: [], error: failure.code ?? "soundcloud_failed" }));
    const soundcloudPlaylistsTask = this.#soundcloud
      .searchPlaylists(text, { limit: pageSize, offset, scope: sub, ttlMs })
      .then((items) => ({ items }))
      .catch((failure) => ({ items: [], error: failure.code ?? "soundcloud_failed" }));

    const [spotify, soundcloudTracks, soundcloudPlaylists] = await Promise.all([
      spotifyTask,
      soundcloudTracksTask,
      soundcloudPlaylistsTask,
    ]);
    const soundcloudError = soundcloudTracks.error ?? soundcloudPlaylists.error;
    const fullPage = spotify.tracks.length === pageSize
      || spotify.artists.length === pageSize
      || spotify.albums.length === pageSize
      || spotify.playlists.length === pageSize
      || soundcloudTracks.items.length === pageSize
      || soundcloudPlaylists.items.length === pageSize;

    return {
      v: 1,
      query: text,
      spotify,
      soundcloud: {
        tracks: soundcloudTracks.items,
        playlists: soundcloudPlaylists.items,
        ...(soundcloudError ? { error: soundcloudError } : {}),
      },
      next: fullPage ? encodeCursor({ o: offset + pageSize }) : null,
    };
  }

  async artist(sub, id) {
    this.#require();
    const artistId = spotifyId(id);
    const [artist, top] = await Promise.all([
      this.#spotify.api(sub, `/artists/${artistId}`, { ttlMs: this.#config.spotifyCacheTtlMs }),
      this.#spotify.api(sub, `/artists/${artistId}/top-tracks`, {
        query: { market: "from_token" },
        ttlMs: this.#config.spotifyCacheTtlMs,
      }).then((page) => ({
        items: (Array.isArray(page?.tracks) ? page.tracks : []).map(mapSpotifyTrack).filter(Boolean),
      })).catch((failure) => ({ items: [], error: failure.code ?? "spotify_failed" })),
    ]);
    return {
      v: 1,
      artist: mapSpotifyArtist(artist),
      top_tracks: top.items,
      ...(top.error ? { error: top.error } : {}),
    };
  }

  async album(sub, id, { limit = MAX_LIMIT, cursor = null } = {}) {
    this.#require();
    const albumId = spotifyId(id);
    const pageSize = normalizeLimit(limit);
    const offset = offsetFrom(cursor);
    const [header, tracks] = await Promise.all([
      this.#spotify.api(sub, `/albums/${albumId}`, { ttlMs: this.#config.spotifyCacheTtlMs }),
      this.#spotify.api(sub, `/albums/${albumId}/tracks`, {
        query: { limit: pageSize, offset },
        ttlMs: this.#config.spotifyCacheTtlMs,
      }),
    ]);
    const compactAlbum = mapSpotifyAlbum(header);
    const items = (Array.isArray(tracks?.items) ? tracks.items : [])
      .map(mapSpotifyTrack)
      .filter(Boolean)
      .map((track) => ({
        ...track,
        album: compactAlbum ? { id: compactAlbum.id, name: compactAlbum.name } : track.album,
        image: track.image ?? compactAlbum?.image ?? null,
      }));
    return {
      v: 1,
      album: compactAlbum,
      items,
      next: tracks?.next ? encodeCursor({ o: offset + pageSize }) : null,
    };
  }

  async #recentlyPlayed(sub) {
    try {
      const page = await this.#spotify.api(sub, "/me/player/recently-played", {
        query: { limit: 50 },
        ttlMs: this.#config.spotifyHomeCacheTtlMs,
      });
      const items = [];
      const seen = new Set();
      for (const entry of Array.isArray(page?.items) ? page.items : []) {
        const track = mapSpotifyTrack(entry?.track);
        if (!track || seen.has(track.uri)) continue;
        seen.add(track.uri);
        items.push({ ...track, played_at: typeof entry.played_at === "string" ? entry.played_at : null });
      }
      return { items, error: null };
    } catch (failure) {
      return { items: [], error: failure };
    }
  }

  async #newReleases(sub) {
    let artists;
    try {
      const page = await this.#spotify.api(sub, "/me/following", {
        query: { type: "artist", limit: 50 },
        ttlMs: this.#config.spotifyHomeCacheTtlMs,
      });
      artists = Array.isArray(page?.artists?.items) ? page.artists.items : [];
    } catch (failure) {
      return { items: [], error: failure };
    }

    const wanted = artists.slice(0, this.#config.homeArtistLimit);
    const found = new Map();
    let firstError = null;
    for (let index = 0; index < wanted.length && firstError?.code !== "spotify_rate_limited"; index += 4) {
      const batch = wanted.slice(index, index + 4);
      const results = await Promise.all(batch.map((artist) => {
        if (typeof artist?.id !== "string" || !SPOTIFY_ID.test(artist.id)) return Promise.resolve({ items: [] });
        return this.#spotify.api(sub, `/artists/${artist.id}/albums`, {
          query: { include_groups: "album,single", limit: 10, market: "from_token" },
          ttlMs: this.#config.spotifyHomeCacheTtlMs,
        }).then((page) => ({
          items: (Array.isArray(page?.items) ? page.items : []).map(mapSpotifyAlbum).filter(Boolean),
        })).catch((failure) => ({ items: [], error: failure }));
      }));
      for (const result of results) {
        if (result.error) firstError ??= result.error;
        for (const album of result.items) {
          if (!album || found.has(album.id)) continue;
          found.set(album.id, album);
        }
      }
    }

    const items = [...found.values()]
      .sort((left, right) => String(right.release_date ?? "").localeCompare(String(left.release_date ?? "")))
      .slice(0, 50);
    return { items, error: firstError };
  }

  async home(sub) {
    this.#require();
    const cached = this.#homeCache.get(sub);
    if (cached !== undefined) return cached;

    const [recent, releases] = await Promise.all([
      this.#recentlyPlayed(sub),
      this.#newReleases(sub),
    ]);
    if (recent.error && releases.error) throw recent.error;

    const result = {
      v: 1,
      recently_played: recent.items,
      new_releases: releases.items,
      generated_at: new Date(this.#now()).toISOString(),
    };
    const errors = {};
    if (recent.error) errors.recently_played = recent.error.code ?? "spotify_failed";
    if (releases.error) errors.new_releases = releases.error.code ?? "spotify_failed";
    if (Object.keys(errors).length > 0) {
      // A partial shelf is useful, but do not cache it so the next open can
      // recover from a brief rate limit.
      result.errors = errors;
      return result;
    }
    return this.#homeCache.set(sub, result);
  }
}
