// Exact port of src/downloads/matching.rs. Keep the word splitting, gates,
// qualifier and boilerplate lists and formula in step with the Rust module.
// A change here without the same change there breaks the parity test.

export const QUALIFIERS = Object.freeze([
  "live",
  "remix",
  "acoustic",
  "cover",
  "karaoke",
  "instrumental",
  "remaster",
  "remastered",
  "slowed",
  "sped",
  "nightcore",
  "reverb",
  "mashup",
  "tribute",
  "demo",
  "extended",
  "edit",
]);

export const BOILERPLATE = Object.freeze([
  "official",
  "audio",
  "video",
  "music",
  "lyrics",
  "lyric",
  "visualizer",
  "hd",
  "hq",
  "4k",
]);

const YOUTUBE_ID = /^[A-Za-z0-9_-]{11}$/;

/** Rust `char::is_alphanumeric` with a Unicode split. Combining marks stay
 * with their base letter, as Rust's Alphabetic property keeps them. */
export function words(text) {
  if (typeof text !== "string") return new Set();
  return new Set(
    text
      .split(/[^\p{L}\p{N}\p{M}]+/u)
      .filter((word) => word.length > 0)
      .map((word) => word.toLowerCase()),
  );
}

export function coverage(wanted, actual) {
  if (wanted.size === 0) return 0;
  let shared = 0;
  for (const word of wanted) if (actual.has(word)) shared += 1;
  return shared / wanted.size;
}

function artistNames(track) {
  const artists = track.artists ?? [];
  return artists
    .map((artist) => (typeof artist === "string" ? artist : artist?.name ?? ""))
    .filter((name) => name.length > 0)
    .join(", ");
}

function primaryArtist(track) {
  const first = track.artists?.[0];
  if (typeof first === "string") return first;
  return first?.name ?? "";
}

/** Accepts the phone's camelCase and the Rust-shaped snake_case fields. */
export function normalizeTrack(track) {
  if (!track || typeof track !== "object") throw new Error("invalid_track");
  return {
    uri: typeof track.uri === "string" ? track.uri : "",
    name: typeof track.name === "string" ? track.name : "",
    durationMs: Number(track.durationMs ?? track.duration_ms ?? 0),
    artists: Array.isArray(track.artists) ? track.artists : [],
  };
}

/**
 * Returns a score in [0, 1] or null. Every rejection is the Rust gate: a
 * missing duration, a difference over three seconds, a live stream, weak title
 * coverage, a missing primary artist, a mismatched qualifier, or a title that
 * carries too much extra wording.
 */
export function score(track, candidate) {
  const wanted = normalizeTrack(track);
  const duration = typeof candidate?.duration === "number" && Number.isFinite(candidate.duration) && candidate.duration > 0
    ? candidate.duration
    : null;
  if (duration === null) return null;
  if (!Number.isFinite(wanted.durationMs) || wanted.durationMs <= 0) return null;
  const difference = Math.abs(duration - wanted.durationMs / 1000);
  if (difference > 3 || candidate.live === true) return null;
  const title = words(wanted.name);
  const artists = words(artistNames(wanted));
  const primary = words(primaryArtist(wanted));
  const actualTitle = words(candidate.title);
  const actual = words(`${candidate.title ?? ""} ${candidate.uploader ?? ""}`);
  if (coverage(title, actualTitle) < 0.9 || coverage(primary, actual) < 1) return null;
  const expected = new Set([...title, ...artists]);
  for (const qualifier of QUALIFIERS) {
    if (expected.has(qualifier) !== actualTitle.has(qualifier)) return null;
  }
  const meaningful = new Set(actualTitle);
  for (const boilerplate of BOILERPLATE) {
    if (!expected.has(boilerplate)) meaningful.delete(boilerplate);
  }
  const precision = coverage(meaningful, expected);
  if (precision < 0.75) return null;
  return 0.5 * precision + 0.35 * coverage(artists, actual) + 0.15 * (1 - difference / 3);
}

export function providerOf(candidate) {
  if (candidate?.source === "soundcloud" || candidate?.source === "youtube") return candidate.source;
  try {
    const host = new URL(candidate?.url ?? "").host;
    if (host === "soundcloud.com" || host === "www.soundcloud.com" || host === "m.soundcloud.com") return "soundcloud";
  } catch {
    // Not a URL; fall through.
  }
  return "youtube";
}

/**
 * The best candidate. Scores within 0.02 of the winner prefer SoundCloud,
 * because its public tracks are the more faithful copy when both fit.
 * Otherwise ties keep the Rust `max_by` behaviour: the last equal score wins.
 */
export function bestMatch(track, candidates) {
  const scored = [];
  for (const candidate of candidates ?? []) {
    const value = score(track, candidate);
    if (value !== null) scored.push({ candidate, score: value });
  }
  if (scored.length === 0) return null;
  let max = -Infinity;
  for (const entry of scored) max = Math.max(max, entry.score);
  const near = scored.filter((entry) => max - entry.score <= 0.02);
  const soundcloud = near.filter((entry) => providerOf(entry.candidate) === "soundcloud");
  const youtube = near.filter((entry) => providerOf(entry.candidate) === "youtube");
  // SoundCloud only breaks a tie between the two providers. Without both,
  // keep the Rust max_by result exactly (the last equal maximum).
  const pool = soundcloud.length > 0 && youtube.length > 0
    ? soundcloud
    : scored.filter((entry) => entry.score === max);
  let best = null;
  for (const entry of pool) {
    if (best === null || entry.score >= best.score) best = entry;
  }
  return best?.candidate ?? null;
}

/** Canonical YouTube watch URL, matching matching.rs::youtube_url. */
export function youtubeUrl(text) {
  let url;
  try {
    url = new URL(text);
  } catch {
    return null;
  }
  if (url.protocol !== "https:" || url.username !== "" || url.password !== "") return null;
  let id;
  if (["youtube.com", "www.youtube.com", "music.youtube.com"].includes(url.host) && url.pathname === "/watch") {
    id = url.searchParams.get("v") ?? "";
  } else if (url.host === "youtu.be") {
    id = url.pathname.startsWith("/") ? url.pathname.slice(1) : "";
  } else {
    return null;
  }
  if (!YOUTUBE_ID.test(id)) return null;
  return `https://www.youtube.com/watch?v=${id}`;
}

/** The search URL the Rust helper builds. */
export function searchUrl(track) {
  const wanted = normalizeTrack(track);
  const query = `${artistNames(wanted)} - ${wanted.name}`;
  return `https://www.youtube.com/results?search_query=${encodeURIComponent(query)}`;
}
