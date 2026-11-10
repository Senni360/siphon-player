import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { bestMatch, score, searchUrl, youtubeUrl } from "../src/matcher.mjs";

function track(overrides = {}) {
  return {
    uri: "spotify:track:abc",
    name: "Northern Lights",
    durationMs: 200_000,
    artists: [{ name: "Aurora" }],
    ...overrides,
  };
}

function candidate(title, uploader, duration, overrides = {}) {
  return {
    title,
    uploader,
    duration,
    url: "https://www.youtube.com/watch?v=abcdefghijk",
    live: false,
    source: "youtube",
    ...overrides,
  };
}

// Fixtures below are the Rust module's own tests, kept word for word in spirit.

test("rust fixture: prefers metadata and duration and accepts topic uploads", () => {
  const candidates = [
    candidate("Aurora - Northern Lights (Official Audio)", "Aurora", 202.9),
    candidate("Northern Lights", "Aurora - Topic", 200.0),
    candidate("Northern Lights", "Someone else", 200.0),
  ];
  assert.equal(bestMatch(track(), candidates).uploader, "Aurora - Topic");
});

test("rust fixture: refuses missing duration, wrong artist, titles and alternate recordings", () => {
  for (const item of [
    candidate("Northern Lights", "Aurora", null),
    candidate("Northern Lights", "Aurora", 203.1),
    candidate("Northern Lights", "Unknown", 200.0),
    candidate("Southern Lights", "Aurora", 200.0),
    candidate("Aurora - Northern Lights (Live)", "Aurora", 200.0),
    candidate("Aurora - Northern Lights (Remix)", "Aurora", 200.0),
    candidate("Northern Lights (Cover)", "Aurora", 200.0),
    candidate("Northern Lights (Sped Up)", "Aurora", 200.0),
    candidate("Northern Lights Full Album Compilation", "Aurora", 200.0),
  ]) {
    assert.equal(score(track(), item), null, `${item.title} / ${item.uploader}`);
  }
  assert.equal(bestMatch(track(), []), null);
});

test("rust fixture: preserves explicit versions and unicode names", () => {
  const acoustic = track({ name: "Northern Lights (Acoustic)" });
  assert.equal(score(acoustic, candidate("Northern Lights", "Aurora", 200.0)), null);
  assert.notEqual(score(acoustic, candidate(acoustic.name, "Aurora", 200.0)), null);
  const unicode = track({ name: "Jóga", artists: [{ name: "Björk" }] });
  assert.notEqual(score(unicode, candidate("BJÖRK - JÓGA", "Björk", 197.0)), null);
});

test("rust fixture: search URL encoding and canonical YouTube URLs", () => {
  assert.equal(
    searchUrl(track()),
    "https://www.youtube.com/results?search_query=Aurora%20-%20Northern%20Lights",
  );
  assert.equal(youtubeUrl("https://youtu.be/abcdefghijk?t=4"), "https://www.youtube.com/watch?v=abcdefghijk");
  for (const url of [
    "http://youtube.com/watch?v=abcdefghijk",
    "https://youtube.com.evil/watch?v=abcdefghijk",
    "https://youtube.com/results?q=x",
    "https://youtube.com/watch?v=bad",
    "ytsearch5:test",
  ]) {
    assert.equal(youtubeUrl(url), null);
  }
});

test("the score is the Rust formula exactly", () => {
  assert.equal(score(track(), candidate("Northern Lights", "Aurora", 200.0)), 1);
  // A 1.5 s difference: 0.5 + 0.35 + 0.15 * 0.5.
  assert.ok(Math.abs(score(track(), candidate("Northern Lights", "Aurora", 201.5)) - 0.925) < 1e-12);
  // Official/audio boilerplate is removed before precision.
  assert.equal(score(track(), candidate("Aurora - Northern Lights (Official Audio)", "Aurora", 200.0)), 1);
  // An extra meaningful word fails precision.
  assert.equal(score(track(), candidate("Aurora - Northern Lights Tonight Again", "Aurora", 200.0)), null);
});

test("a SoundCloud tie within 0.02 is preferred, a clear loss is not", () => {
  const youtube = candidate("Northern Lights", "Aurora", 200.1, { source: "youtube" });
  const soundcloud = candidate("Northern Lights", "Aurora", 200.3, {
    source: "soundcloud",
    url: "https://soundcloud.com/aurora/northern-lights",
  });
  assert.equal(bestMatch(track(), [youtube, soundcloud]), soundcloud);
  const muchWorse = candidate("Northern Lights", "Aurora", 200.6, {
    source: "soundcloud",
    url: "https://soundcloud.com/aurora/northern-lights",
  });
  assert.equal(bestMatch(track(), [youtube, muchWorse]), youtube);
});

test("without a second provider the plain highest score still wins", () => {
  const exact = candidate("Northern Lights", "Aurora", 200.0);
  const close = candidate("Northern Lights", "Aurora", 200.1);
  assert.equal(bestMatch(track(), [exact, close]), exact);
  assert.equal(bestMatch(track(), [close]), close);
});

test("below the threshold returns nothing, never a guess", () => {
  for (const bad of [
    candidate("Northern Lights", "Aurora", 210),
    candidate("Northern Lights", "Aurora", 0),
    candidate("Northern Lights", "Aurora", Number.NaN),
    candidate("Northern Lights", "Aurora", Infinity),
    candidate("Northern Lights", "Aurora", 200, { live: true }),
    candidate("Something else entirely", "Aurora", 200),
  ]) {
    assert.equal(score(track(), bad), null);
  }
});

test("song with no artists cannot pass the primary-artist gate", () => {
  assert.equal(score(track({ artists: [] }), candidate("Northern Lights", "Aurora", 200)), null);
  assert.equal(score(track({ durationMs: Number.NaN }), candidate("Northern Lights", "Aurora", 200)), null);
});

// The same file the Rust test reads. A change to one implementation that the
// other does not make shows up as a failure in both test suites.
const cases = JSON.parse(
  readFileSync(new URL("../../../src/downloads/testdata/matching-cases.json", import.meta.url), "utf8"),
).cases;

test("the published matching cases hold", () => {
  assert.ok(cases.length >= 90, `${cases.length} cases`);
  for (const entry of cases) {
    const best = bestMatch(entry.track, entry.candidates);
    if (entry.expect.result === "match") {
      assert.ok(best, `${entry.id}: expected a match`);
      assert.notEqual(score(entry.track, best), null, `${entry.id}: chosen candidate scores`);
      if (entry.expect.index !== undefined) {
        assert.equal(entry.candidates.indexOf(best), entry.expect.index, `${entry.id}: chosen candidate`);
      }
    } else {
      assert.equal(best, null, `${entry.id}: expected no match, got ${best?.title}`);
    }
  }
});
