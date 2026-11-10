// The matcher is a port of src/downloads/matching.rs. These tests press on the
// places where text handling and the three-second gate are easiest to get
// subtly wrong: unicode, feature credits, remasters and the exact threshold.
import assert from "node:assert/strict";
import test from "node:test";
import { bestMatch, normalizeTrack, providerOf, score, searchUrl, words, youtubeUrl } from "../src/matcher.mjs";

function track(overrides = {}) {
  return { uri: "spotify:track:abc", name: "Northern Lights", durationMs: 200_000, artists: [{ name: "Aurora" }], ...overrides };
}

function candidate(title, uploader, duration, overrides = {}) {
  return { title, uploader, duration, url: "https://www.youtube.com/watch?v=abcdefghijk", live: false, source: "youtube", ...overrides };
}

test("a duration exactly three seconds away still matches; one millisecond more does not", () => {
  assert.notEqual(score(track(), candidate("Northern Lights", "Aurora", 203)), null);
  assert.notEqual(score(track(), candidate("Northern Lights", "Aurora", 197)), null);
  assert.equal(score(track(), candidate("Northern Lights", "Aurora", 203.001)), null);
  assert.equal(score(track(), candidate("Northern Lights", "Aurora", 196.999)), null);
  // The same edge expressed in the track's milliseconds.
  assert.notEqual(score(track({ durationMs: 200_000 }), candidate("Northern Lights", "Aurora", 203.000)), null);
  assert.equal(score(track({ durationMs: 200_000 }), candidate("Northern Lights", "Aurora", 203.0001)), null);
  assert.notEqual(score(track({ durationMs: 0.5 }), candidate("Northern Lights", "Aurora", 2.5)), null);
  assert.equal(score(track({ durationMs: 0.5 }), candidate("Northern Lights", "Aurora", 3.5001)), null);
});

test("remasters are compared in both directions", () => {
  const remastered = track({ name: "Northern Lights (Remastered 2011)" });
  assert.notEqual(score(remastered, candidate("Aurora - Northern Lights (Remastered 2011)", "Aurora", 200)), null);
  assert.equal(score(track(), candidate("Aurora - Northern Lights (2011 Remaster)", "Aurora", 200)), null);
  assert.equal(score(track(), candidate("Aurora - Northern Lights (Remastered)", "Aurora", 200)), null);
  // remaster and remastered are different qualifiers, so they do not stand in for each other.
  assert.equal(score(track({ name: "Northern Lights (Remaster)" }), candidate("Aurora - Northern Lights (Remastered)", "Aurora", 200)), null);
  // Release tags are metadata, so the same recording still matches.
  assert.notEqual(score(remastered, candidate("Aurora - Northern Lights (Remastered 2011 Deluxe Edition)", "Aurora", 200)), null);
});

test("feature credits never decide identity", () => {
  const withFeature = track({ name: "Song", artists: [{ name: "A" }, { name: "B" }] });
  assert.notEqual(score(withFeature, candidate("A - Song (feat. B)", "A", 200)), null);
  // A credit for an artist the track does not carry is metadata, not wording.
  assert.notEqual(score(track({ name: "Song", artists: [{ name: "A" }] }), candidate("A - Song (feat. B)", "A", 200)), null);
  // The bare title form still names the artist through the uploader.
  assert.notEqual(score(withFeature, candidate("Song (feat. B)", "A", 200)), null);
  // Extra credits are dropped before precision, whatever their count.
  assert.notEqual(score(withFeature, candidate("A - Song ft. B", "A", 200)), null);
  assert.notEqual(score(withFeature, candidate("A - Song (feat. B & C)", "A", 200)), null);
  // A feature does not replace the primary artist.
  assert.equal(score(withFeature, candidate("Song (feat. B)", "B", 200)), null);
});

test("empty or wordless artists cannot pass the primary-artist gate", () => {
  for (const artists of [[], [{}], [{ name: "" }], [{ name: "!!!" }], [null]]) {
    assert.equal(score(track({ artists }), candidate("Northern Lights", "Aurora", 200)), null, JSON.stringify(artists));
  }
  assert.notEqual(score(track({ artists: ["Aurora"] }), candidate("Northern Lights", "Aurora", 200)), null);
  assert.equal(score(track({ artists: [{ name: "Aurora" }], name: "" }), candidate("Northern Lights", "Aurora", 200)), null);
});

test("unicode names fold case, accents and width", () => {
  assert.notEqual(score(track({ name: "Jóga", artists: [{ name: "Björk" }] }), candidate("BJÖRK - JÓGA", "Björk", 197)), null);
  assert.notEqual(score(track({ name: "Песня", artists: [{ name: "Артист" }] }), candidate("АРТИСТ - ПЕСНЯ", "Артист", 200)), null);
  assert.notEqual(score(track({ name: "夜に駆ける", artists: [{ name: "YOASOBI" }] }), candidate("YOASOBI - 夜に駆ける", "YOASOBI", 200)), null);
  // An upload that drops the accent is still the same title.
  assert.notEqual(score(track({ name: "Café", artists: [{ name: "Aurora" }] }), candidate("Aurora - Cafe", "Aurora", 200)), null);
  // Folding spelling never rescues a different title.
  assert.equal(score(track({ name: "Café", artists: [{ name: "Aurora" }] }), candidate("Aurora - Caff", "Aurora", 200)), null);
});

test("combining marks are part of a word, as Rust's is_alphanumeric treats them", {
  todo: "matcher.mjs words() splits on [^\\p{L}\\p{N}], so Unicode combining marks (Mc/Mn, e.g. Devanagari matras) are dropped; Rust's char::is_alphanumeric keeps them",
}, () => {
  // Two different Devanagari words that differ only in the vowel sign.
  assert.notDeepEqual([...words("कि")], [...words("की")]);
  assert.equal(score(track({ name: "कि", artists: [{ name: "क" }] }), candidate("की", "क", 200)), null);
  assert.notEqual(score(track({ name: "कि", artists: [{ name: "क" }] }), candidate("कि", "क", 200)), null);
});

test("boilerplate is dropped but a meaningful extra word is not", () => {
  for (const boilerplate of ["Official Audio", "Lyrics", "Visualizer", "HD", "4K", "Music Video"]) {
    assert.equal(
      score(track(), candidate(`Aurora - Northern Lights (${boilerplate})`, "Aurora", 200)),
      1,
      boilerplate,
    );
  }
  assert.equal(score(track(), candidate("Aurora - Northern Lights (Official Audio) Tonight Again", "Aurora", 200)), null);
  // A boilerplate word that the title itself carries is not stripped.
  assert.notEqual(score(track({ name: "Northern Lights (Live)" }), candidate("Aurora - Northern Lights (Live)", "Aurora", 200)), null);
  assert.equal(score(track({ name: "Northern Lights (Live)" }), candidate("Aurora - Northern Lights", "Aurora", 200)), null);
});

test("bestMatch keeps the documented tie rules and the SoundCloud window", () => {
  const exact = candidate("Northern Lights", "Aurora", 200.0);
  const equal = candidate("Northern Lights", "Aurora", 200.0, { url: "https://www.youtube.com/watch?v=bbbbbbbbbbb" });
  assert.equal(bestMatch(track(), [exact, equal]), equal, "the last equal score wins");

  const youtube = candidate("Northern Lights", "Aurora", 200.0, { source: "youtube" });
  const soundcloud = candidate("Northern Lights", "Aurora", 200.3, { source: "soundcloud", url: "https://soundcloud.com/a/b" });
  assert.equal(bestMatch(track(), [youtube, soundcloud]), soundcloud, "inside the window SoundCloud is preferred");
  const outside = candidate("Northern Lights", "Aurora", 200.5, { source: "soundcloud", url: "https://soundcloud.com/a/b" });
  assert.equal(bestMatch(track(), [youtube, outside]), youtube, "outside the window the score wins");

  assert.equal(bestMatch(track(), [candidate("Southern Lights", "Aurora", 200)]), null);
  assert.equal(bestMatch(track(), []), null);
  assert.equal(bestMatch(track(), undefined), null);
});

test("providerOf reads the listing's own source and the permalink host", () => {
  assert.equal(providerOf({ source: "soundcloud" }), "soundcloud");
  assert.equal(providerOf({ source: "youtube" }), "youtube");
  assert.equal(providerOf({ url: "https://m.soundcloud.com/a/b" }), "soundcloud");
  assert.equal(providerOf({ url: "https://www.soundcloud.com/a/b" }), "soundcloud");
  assert.equal(providerOf({ url: "https://soundcloud.com.evil/a/b" }), "youtube");
  assert.equal(providerOf({ url: "not a url" }), "youtube");
  assert.equal(providerOf(null), "youtube");
});

test("normalizeTrack accepts the phone's camelCase and the Rust snake_case", () => {
  assert.deepEqual(normalizeTrack({ name: "N", duration_ms: 1, artists: ["A"] }), { uri: "", name: "N", durationMs: 1, artists: ["A"], isrc: null });
  assert.equal(normalizeTrack({ duration_ms: "200000" }).durationMs, 200_000);
  assert.throws(() => normalizeTrack(null), /invalid_track/);
  assert.equal(normalizeTrack({ artists: "Aurora" }).artists.length, 0);
});

test("YouTube URLs are canonical only, and search uses the same artist list", () => {
  assert.equal(youtubeUrl("https://music.youtube.com/watch?v=abcdefghijk&list=x"), "https://www.youtube.com/watch?v=abcdefghijk");
  assert.equal(youtubeUrl("https://youtu.be/abc-def_ghi"), "https://www.youtube.com/watch?v=abc-def_ghi");
  assert.equal(youtubeUrl("https://www.youtube.com/watch?v=abcdefghijk&v=zzzzzzzzzzz"), "https://www.youtube.com/watch?v=abcdefghijk");
  for (const url of [
    "https://youtu.be/abcdefghij", "https://youtu.be/abcdefghijkl", "https://user:pass@youtu.be/abcdefghijk",
    "https://youtube.com/watch?v=", "https://youtube.com/watch?V=abcdefghijk", "https://youtube.com/watch",
  ]) {
    assert.equal(youtubeUrl(url), null, url);
  }
  assert.equal(searchUrl(track({ artists: [{ name: "A" }, { name: "B" }] })), "https://www.youtube.com/results?search_query=A%20-%20Northern%20Lights");
  assert.equal(searchUrl(track({ artists: [] })), "https://www.youtube.com/results?search_query=%20-%20Northern%20Lights");
});
