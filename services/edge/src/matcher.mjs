// Port of src/downloads/matching.rs. Keep the folding, title readings, gates,
// version tags and formula in step with the Rust module; the places where this
// side is deliberately stricter are noted in the report that came with this
// port, and the shared fixture test catches drift. A change here without the
// same change there breaks the shared fixture test.
//
// A candidate passes only when its title is the same song and its version is
// the one that was asked for. Credits, tags and spelling are metadata, never
// identity: "Sky (Prod. By Art Dealer)" is "Sky", but "Sky (Live)" is another
// recording. An account that spells the artist's name with no spaces is that
// artist, and the artist's own upload may inflect one title word. When the
// evidence is thin the answer is no match.

/** Strong version words: they can only describe another recording. */
export const QUALIFIERS = Object.freeze([
  "8d",
  "acapella",
  "bootleg",
  "chopped",
  "flip",
  "freestyle",
  "karaoke",
  "mashup",
  "nightcore",
  "refix",
  "recreate",
  "remake",
  "reverb",
  "rework",
  "screwed",
  "slowed",
  "sped",
  "stems",
  "vip",
]);

/** Version words that are also ordinary title words; segments only. */
export const SEGMENT_QUALIFIERS = Object.freeze([
  "acoustic",
  "clean",
  "cover",
  "demo",
  "edit",
  "extended",
  "instrumental",
  "live",
  "remaster",
  "remastered",
  "remix",
  "tribute",
]);

/** Tags that mark an unreleased or unfinished recording. */
export const SOFT_TAGS = Object.freeze([
  "leak",
  "leaked",
  "og",
  "reference",
  "snippet",
  "unmastered",
  "unreleased",
]);

/** A segment carrying one of these is identity, not a tag. */
const IDENTITY_MARKERS = Object.freeze([
  "chapter",
  "interlude",
  "intro",
  "outro",
  "part",
  "pt",
  "v1",
  "v2",
]);

/** A credit starts here and runs to the end of its segment. */
const CREDIT_TAIL = Object.freeze([
  "dir",
  "feat",
  "featuring",
  "ft",
  "mastered",
  "mixed",
  "prod",
  "produced",
  "producer",
]);

/** Only feature credits are safe to drop from the wanted title too. */
const FEATURE_TAIL = Object.freeze(["feat", "featuring", "ft"]);

export const BOILERPLATE = Object.freeze([
  "audio",
  "hd",
  "hq",
  "4k",
  "lyric",
  "lyrics",
  "music",
  "official",
  "video",
  "visualizer",
]);

/** An inflected title scores this much below the same title spelled exactly. */
const INFLECTION_PENALTY = 0.01;

const YOUTUBE_ID = /^[A-Za-z0-9_-]{11}$/;
const WORD_RUN = /[\p{L}\p{N}\p{M}]+/gu;
const WORD_SEPARATOR = /[^\p{L}\p{N}\p{M}]+/u;


/**
 * Accented letters grouped under the letter they are read as, copied from
 * `matching.rs` so an upload that writes `OZI` or `Strasse` still says `ØZI`
 * or `Straße`. Letters from other scripts are left alone: their marks carry
 * meaning, so they are not diacritics to drop.
 */
const ACCENT_GROUPS = Object.freeze([
  ["a", "àáâãäåāăąǎǟǡǻȁȃȧḁạảấầẩẫậắằẳẵặɑ"],
  ["ae", "æ"],
  ["b", "ḃḅḇƀ"],
  ["c", "çćĉċčḉ"],
  ["d", "ďḋḍḏḑḓđð"],
  ["e", "èéêëēĕėęěȅȇȩḕḗḙḛḝẹẻẽếềểễệ"],
  ["f", "ḟ"],
  ["g", "ĝğġģǧǵḡ"],
  ["h", "ĥȟḣḥḧḩḫẖħ"],
  ["i", "ìíîïĩīĭįİǐȉȋḭḯỉịıɨ"],
  ["j", "ĵǰ"],
  ["k", "ķǩḱḳḵ"],
  ["l", "ĺļľḷḹḻḽł"],
  ["m", "ḿṁṃ"],
  ["n", "ñńņňǹṅṇṉṋŋ"],
  ["o", "òóôõöōŏőơǒǫǭȍȏȫȭȯȱṍṏṑṓọỏốồổỗộớờởỡợø"],
  ["oe", "œ"],
  ["p", "ṕṗ"],
  ["r", "ŕŗřȑȓṙṛṝṟ"],
  ["s", "śŝşšșṡṣṥṧṩʂ"],
  ["ss", "ß"],
  ["t", "ţťțṫṭṯṱẗŧ"],
  ["th", "þ"],
  ["u", "ùúûüũūŭůűųưǔǖǘǚǜȕȗṳṵṷṹṻụủứừửữựʉ"],
  ["v", "ṽṿ"],
  ["w", "ŵẁẃẅẇẉẘ"],
  ["x", "ẋẍ"],
  ["y", "ýÿŷȳẏẙỳỵỷỹ"],
  ["z", "źżžẑẓẕƶ"],
]);
const ACCENT_BASES = new Map();
for (const [base, letters] of ACCENT_GROUPS) {
  for (const letter of letters) ACCENT_BASES.set(letter, base);
}

/**
 * Fold a title or name to the form the gates compare: compatibility forms
 * unified, apostrophes removed, Latin diacritics dropped, lowercase, everything
 * else kept so `$`, `&`, emoji and non-Latin marks still separate or join words
 * the way Rust's `char::is_alphanumeric` does.
 */
export function fold(text) {
  if (typeof text !== "string") return "";
  const normalized = text
    .normalize("NFKC")
    .replace(/['\u2018\u2019\u02bc\u00b4`]/g, "");
  let out = "";
  // Lowercase one code point at a time, as Rust does: a whole-string
  // toLowerCase() would turn a final Greek Σ into ς while Rust keeps σ.
  for (const c of normalized) {
    for (const lower of c.toLowerCase()) out += ACCENT_BASES.get(lower) ?? lower;
  }
  return out;
}

function tokensOf(text) {
  return fold(text).split(WORD_SEPARATOR).filter((word) => word.length > 0);
}

function set(tokens) {
  return new Set(tokens);
}

/** `$` read as `s`, tried only when the primary spelling does not line up. */
function secondaryTokens(text) {
  return fold(text).replaceAll("$", "s").split(WORD_SEPARATOR).filter((word) => word.length > 0);
}

function collapsed(tokens) {
  return tokens.join("");
}

function tokenCounts(tokens) {
  const counts = new Map();
  for (const token of tokens) counts.set(token, (counts.get(token) ?? 0) + 1);
  return counts;
}

/** Identity markers, with `pt` and `part` read as one word. */
function identityCounts(tokens) {
  const counts = new Map();
  for (const token of tokens) {
    const canonical = token === "pt" ? "part" : token;
    if (IDENTITY_MARKERS.includes(canonical)) counts.set(canonical, (counts.get(canonical) ?? 0) + 1);
  }
  return counts;
}

function sameCounts(left, right) {
  if (left.size !== right.size) return false;
  for (const [token, count] of left) if (right.get(token) !== count) return false;
  return true;
}

/** Rust `words()`, kept as the module's public tokenizer. */
export function words(text) {
  return set(tokensOf(text));
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
    isrc: track.externalIds?.isrc ?? track.external_ids?.isrc ?? track.isrc ?? null,
  };
}

function tagsIn(tokens, strongOnly) {
  const tags = new Set();
  for (let index = 0; index + 1 < tokens.length; index += 1) {
    const a = tokens[index];
    const b = tokens[index + 1];
    let tag = null;
    if (a === "type" && b === "beat") tag = "type beat";
    else if ((a === "sped" || a === "speed") && b === "up") tag = "speed";
    else if (a === "bass" && b === "boosted") tag = "bass";
    else if (a === "open" && b === "verse") tag = "open verse";
    else if (a === "vocals" && b === "only") tag = "vocals only";
    else if (a === "radio" && b === "edit") tag = "edit";
    else if (a === "extended" && b === "mix") tag = "extended";
    else if (a === "a" && b === "cappella") tag = "acapella";
    if (tag) tags.add(tag);
  }
  for (const token of tokens) {
    if (QUALIFIERS.includes(token)) tags.add(token === "sped" ? "speed" : token);
    if (!strongOnly && SEGMENT_QUALIFIERS.includes(token)) tags.add(token);
  }
  return tags;
}

function softTags(tokens) {
  return new Set(tokens.filter((token) => SOFT_TAGS.includes(token)));
}

function hasIdentityMarker(text) {
  return tokensOf(text).some((token) => IDENTITY_MARKERS.includes(token));
}

/** Bracketed groups, and the text outside them split on ` - `. */
function splitParts(text) {
  const outside = [];
  const brackets = [];
  let depth = 0;
  let start = 0;
  const source = String(text ?? "");
  for (let index = 0; index < source.length; index += 1) {
    const c = source[index];
    if (c === "(" || c === "[" || c === "{") {
      if (depth === 0) start = index + 1;
      depth += 1;
    } else if (c === ")" || c === "]" || c === "}") {
      if (depth > 0) {
        depth -= 1;
        if (depth === 0 && index > start) brackets.push(source.slice(start, index));
      }
    } else if (depth === 0) {
      outside.push(c);
    }
  }
  // An unclosed bracket still carries metadata: "Song (Live" is not "Song".
  if (depth > 0 && source.length > start) brackets.push(source.slice(start));
  const joined = outside.join("");
  const parts = [];
  let current = "";
  for (let index = 0; index < joined.length; index += 1) {
    const c = joined[index];
    const dash = c === "-" || c === "\u2013" || c === "\u2014";
    const spaced = index > 0 && /\s/.test(joined[index - 1]) && /\s/.test(joined[index + 1] ?? "");
    if (dash && spaced) {
      if (current.trim().length > 0) parts.push(current.trim());
      current = "";
    } else {
      current += c;
    }
  }
  if (current.trim().length > 0) parts.push(current.trim());
  return { parts, brackets };
}

/** Drop a producer or feature credit that runs on from the song name. */
function stripCreditTail(part, credits = CREDIT_TAIL) {
  for (const match of part.matchAll(WORD_RUN)) {
    if (credits.includes(fold(match[0]))) return part.slice(0, match.index).trimEnd();
  }
  return part;
}

function parseWanted(name) {
  const { parts, brackets } = splitParts(name);
  const segments = [...brackets, ...parts.slice(1)];
  let mainTokens = parts.length > 0 ? tokensOf(stripCreditTail(parts[0], FEATURE_TAIL)) : [];
  if (mainTokens.length === 0) mainTokens = tokensOf(name);
  const tokens = [...mainTokens];
  for (const segment of segments) {
    if (hasIdentityMarker(segment)) tokens.push(...tokensOf(segment));
  }
  const versions = new Set();
  for (const segment of segments) for (const tag of tagsIn(tokensOf(segment), false)) versions.add(tag);
  for (const tag of tagsIn(tokensOf(name), true)) versions.add(tag);
  const soft = softTags(tokensOf(name));
  return {
    words: set(tokens),
    tokens,
    collapsed: collapsed(tokens),
    versions,
    soft,
    identity: identityCounts(tokensOf(name)),
    mainCounts: tokenCounts(mainTokens),
  };
}

function parseCandidate(title, wanted) {
  const { parts: rawParts, brackets } = splitParts(title);
  const parts = rawParts.map((part) => stripCreditTail(part).trim()).filter((part) => part.length > 0);
  let main = null;
  let bestKey = null;
  parts.forEach((part, index) => {
    const tokens = tokensOf(part);
    const wordSet = set(tokens);
    let shared = 0;
    for (const word of wordSet) if (wanted.words.has(word)) shared += 1;
    const key = [shared, collapsed(tokens) === wanted.collapsed ? 1 : 0, index];
    if (
      bestKey === null ||
      key[0] > bestKey[0] ||
      (key[0] === bestKey[0] && key[1] > bestKey[1]) ||
      (key[0] === bestKey[0] && key[1] === bestKey[1] && key[2] > bestKey[2])
    ) {
      bestKey = key;
      main = index;
    }
  });
  const mainTokens = main === null ? [] : tokensOf(parts[main]);
  const tokens = [...mainTokens];
  const segments = [
    ...brackets,
    ...parts.filter((_, index) => index !== main),
  ];
  for (const segment of segments) {
    if (hasIdentityMarker(segment)) tokens.push(...tokensOf(segment));
  }
  const versions = new Set();
  for (const segment of segments) for (const tag of tagsIn(tokensOf(segment), false)) versions.add(tag);
  for (const tag of tagsIn(tokensOf(title), true)) versions.add(tag);
  // A version word left in the main title counts when the wanted title does
  // not carry it: "All The Stars Remix" is an edit, while "Live Forever"
  // stays the song. Comparing counts keeps "Live Forever Live" an edit too.
  for (const [token, count] of tokenCounts(mainTokens)) {
    if (SEGMENT_QUALIFIERS.includes(token) && count > (wanted.mainCounts.get(token) ?? 0)) {
      versions.add(token);
    }
  }
  const soft = softTags(tokensOf(title));
  return {
    words: set(tokens),
    tokens,
    collapsed: collapsed(tokens),
    versions,
    soft,
    identity: identityCounts(tokensOf(title)),
  };
}

function isrcOf(track, candidate) {
  const wanted = String(track.isrc ?? "").trim();
  const actual = String(candidate?.isrc ?? "").trim();
  return wanted.length > 0 && wanted.toLowerCase() === actual.toLowerCase();
}

function publisherCredit(primary, candidate) {
  if (primary.size === 0) return false;
  const uploader = fold(candidate?.uploader ?? "").trim();
  const credits = Array.isArray(candidate?.credits) ? candidate.credits : [];
  for (const credit of credits) {
    const folded = fold(credit).trim();
    if (folded.length === 0 || folded === uploader) continue;
    if (coverage(primary, set(tokensOf(credit))) >= 1) return true;
  }
  return false;
}

function artistEvidence(candidate, title) {
  const evidence = new Set(title);
  for (const word of tokensOf(candidate?.uploader ?? "")) evidence.add(word);
  const credits = Array.isArray(candidate?.credits) ? candidate.credits : [];
  for (const credit of credits) for (const word of tokensOf(credit)) evidence.add(word);
  return evidence;
}

function secondaryEvidence(candidate, title) {
  const evidence = set(secondaryTokens(title));
  for (const word of secondaryTokens(candidate?.uploader ?? "")) evidence.add(word);
  const credits = Array.isArray(candidate?.credits) ? candidate.credits : [];
  for (const credit of credits) for (const word of secondaryTokens(credit)) evidence.add(word);
  return evidence;
}

function symbolsOnlyArtist(wanted, candidate) {
  const raw = fold(wanted).trim();
  if (raw.length === 0) return false;
  const same = (text) => fold(text).trim() === raw;
  if (same(candidate?.uploader ?? "") || same(candidate?.title ?? "")) return true;
  const credits = Array.isArray(candidate?.credits) ? candidate.credits : [];
  return credits.some((credit) => same(credit));
}

/** Folded and stripped of spaces and punctuation: "Playboi Carti" is "playboicarti". */
function accountKey(text) {
  return fold(text).replace(/[^\p{L}\p{N}\p{M}]+/gu, "");
}

function sameAccount(left, right) {
  const key = accountKey(left);
  return key.length > 0 && key === accountKey(right);
}

/**
 * An uploader or credit that spells the artist with no spaces: the account
 * `playboicarti` is Playboi Carti. Equality only, so an account that merely
 * contains the name, or a one-word artist inside a longer handle, is not the
 * artist.
 */
function artistAccount(primaryName, candidate) {
  if (sameAccount(primaryName, candidate?.uploader ?? "")) return true;
  const credits = Array.isArray(candidate?.credits) ? candidate.credits : [];
  return credits.some((credit) => sameAccount(primaryName, credit));
}

/**
 * One word inflected from another: "go" and "going", "love" and "loved". The
 * shorter is a prefix of the longer, at least two letters long, and no more
 * than three letters shorter.
 */
function inflected(left, right) {
  if (left === right) return true;
  const [shorter, longer] = left.length <= right.length ? [left, right] : [right, left];
  return shorter.length >= 2 && longer.startsWith(shorter) && longer.length - shorter.length <= 3;
}

/** The same title with its words inflected; a different word count is a different title. */
function inflectedTitle(wanted, parsed) {
  if (wanted.tokens.length === 0 || wanted.tokens.length !== parsed.tokens.length) return false;
  return wanted.tokens.every((token, index) => inflected(token, parsed.tokens[index]));
}

function better(left, right) {
  if (left.score !== right.score) return left.score > right.score;
  if (left.isrc !== right.isrc) return left.isrc;
  if (left.verified !== right.verified) return left.verified;
  return left.credited && !right.credited;
}

function evaluate(track, candidate) {
  const wanted = normalizeTrack(track);
  if (!Number.isFinite(wanted.durationMs) || wanted.durationMs <= 0) return null;
  if (candidate?.live === true) return null;
  if (wanted.artists[0] === undefined || wanted.artists[0] === null) return null;
  const primaryName = primaryArtist(wanted);
  const wantedTitle = parseWanted(wanted.name);
  if (wantedTitle.words.size === 0) return null;
  const parsed = parseCandidate(candidate?.title ?? "", wantedTitle);
  if (parsed.words.size === 0) return null;
  const duration = typeof candidate?.duration === "number" && Number.isFinite(candidate.duration) && candidate.duration > 0
    ? candidate.duration
    : null;
  if (duration === null) return null;
  const difference = Math.abs(duration - wanted.durationMs / 1000);
  if (difference > 5) return null;
  const isrc = isrcOf(wanted, candidate);
  const identityExact = parsed.collapsed === wantedTitle.collapsed;
  const primary = set(tokensOf(primaryName));
  const credited = publisherCredit(primary, candidate);
  if (difference > 3 && !(isrc || (identityExact && (candidate?.verified === true || credited)))) return null;
  // The artist's own account spelling a title with its words inflected is the
  // same song: "Never Going Down" is "Never Go Down". The versions gate below
  // must still agree and the uploader itself must be the artist, so a remix, an
  // instrumental, a fan account or a credit alone cannot ride on this. The
  // penalty keeps the score just below the same title spelled exactly.
  const inflectedTitles = sameAccount(primaryName, candidate?.uploader ?? "")
    && difference <= 1.5
    && inflectedTitle(wantedTitle, parsed);
  const titleWords = set(tokensOf(candidate?.title ?? ""));
  const evidence = artistEvidence(candidate, titleWords);
  const allArtists = set(tokensOf(artistNames(wanted)));
  const expected = new Set([...wantedTitle.words, ...allArtists]);
  const meaningful = new Set(parsed.words);
  for (const word of BOILERPLATE) if (!expected.has(word)) meaningful.delete(word);
  const precision = identityExact || inflectedTitles ? 1 : coverage(meaningful, expected);
  const artist = coverage(allArtists, evidence);
  const durationPoints = difference <= 3 ? 1 - difference / 3 : 0;
  const points = 0.5 * precision + 0.35 * artist + 0.15 * durationPoints;
  const score = inflectedTitles && !identityExact ? Math.max(0, points - INFLECTION_PENALTY) : points;
  const verdict = { score, isrc, verified: candidate?.verified === true, credited };
  if (isrc) return verdict;
  if (primary.size === 0) {
    if (!identityExact || !symbolsOnlyArtist(primaryName, candidate)) return null;
  } else {
    const named = coverage(primary, evidence) >= 1
      || coverage(set(secondaryTokens(primaryName)), secondaryEvidence(candidate, candidate?.title ?? "")) >= 1
      || artistAccount(primaryName, candidate);
    if (!named) return null;
  }
  if (!sameSet(parsed.versions, wantedTitle.versions)) return null;
  if (!sameCounts(parsed.identity, wantedTitle.identity)) return null;
  for (const tag of parsed.soft) if (!wantedTitle.soft.has(tag)) return null;
  if (!identityExact && !inflectedTitles && coverage(wantedTitle.words, parsed.words) < 0.9) return null;
  if (precision < 0.75) return null;
  return verdict;
}

function sameSet(left, right) {
  if (left.size !== right.size) return false;
  for (const item of left) if (!right.has(item)) return false;
  return true;
}

/**
 * Returns a score in [0, 1] or null. Every rejection is a gate: a missing
 * duration, a difference over five seconds without the released recording's
 * evidence, a live stream, a missing artist, a different title, a different
 * version, or a title that carries too much extra wording. An artist's own
 * account may drop the name's spaces, and its own upload may inflect a title
 * word; both readings stay narrow and are checked by their own tests.
 */
export function score(track, candidate) {
  const verdict = evaluate(track, candidate);
  return verdict === null ? null : verdict.score;
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
 * because its public tracks are the more faithful copy when both fit. Then
 * the recording's evidence breaks the tie: ISRC, a verified account, a
 * publisher credit naming the artist. Exact ties keep the first one, as the
 * Rust `pick` and `best_match_index` helpers do.
 */
export function bestMatch(track, candidates) {
  const scored = [];
  for (const candidate of Array.isArray(candidates) ? candidates : []) {
    const verdict = evaluate(track, candidate);
    if (verdict !== null) scored.push({ candidate, verdict });
  }
  if (scored.length === 0) return null;
  let max = -Infinity;
  for (const entry of scored) max = Math.max(max, entry.verdict.score);
  const near = scored.filter((entry) => max - entry.verdict.score <= 0.02);
  const soundcloud = near.filter((entry) => providerOf(entry.candidate) === "soundcloud");
  const youtube = near.filter((entry) => providerOf(entry.candidate) === "youtube");
  // SoundCloud only breaks a tie between the two providers. Without both,
  // keep the highest score and let the evidence order what is left.
  const pool = soundcloud.length > 0 && youtube.length > 0
    ? soundcloud
    : scored.filter((entry) => entry.verdict.score === max);
  let best = null;
  for (const entry of pool) {
    if (best === null || better(entry.verdict, best.verdict)) best = entry;
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

/** The search URL the Rust helper builds, with the primary artist only. */
export function searchUrl(track) {
  const wanted = normalizeTrack(track);
  const query = `${primaryArtist(wanted)} - ${wanted.name}`;
  return `https://www.youtube.com/results?search_query=${encodeURIComponent(query)}`;
}
