// An artist's own account may spell the name without its spaces: the owner's
// real playlist has Playboi Carti's own upload on the account `playboicarti`.
// That account's own upload may also inflect a title word: "Never Going Down"
// is slayr's "Never Go Down". Both readings are exact, and accounts or titles
// near them stay rejected.
import assert from "node:assert/strict";
import test from "node:test";
import { bestMatch, score } from "../src/matcher.mjs";

function track(overrides = {}) {
  return {
    uri: "spotify:track:abc",
    name: "Never Go Down",
    durationMs: 120_000,
    artists: [{ name: "slayr" }],
    ...overrides,
  };
}

function candidate(title, uploader, duration, overrides = {}) {
  return {
    title,
    uploader,
    credits: [uploader],
    duration,
    source: "soundcloud",
    url: "https://soundcloud.com/slayr/never-going-down",
    live: false,
    ...overrides,
  };
}

test("an artist's own account spelled without spaces is the artist", () => {
  const carti = track({
    uri: "spotify:track:differentday",
    name: "DIFFERENT DAY",
    durationMs: 166_700,
    artists: [{ name: "Playboi Carti" }],
  });
  const own = candidate("DIFFERENT DAY", "playboicarti", 166.726, {
    url: "https://soundcloud.com/playboicarti/different-day-1",
  });
  assert.notEqual(score(carti, own), null);
  assert.equal(bestMatch(carti, [own]), own);

  // The rest of that search is still not this recording.
  for (const [title, uploader, duration] of [
    ["PLAYBOI CARTI - UR THE MOON // DIFFERENT DAY", "carti meatrider", 226.705],
    ["ur the moon / different day - playboi carti (slowed + reverb)", "eth7n2", 204.013],
    ["DIFFERENT DAY X Better Off Alone/Say Yeah - Playboi Carti", "IcyH3av3n", 172.534],
    ["Playboi Carti Different day (Remix) ID", "Brady (US)", 149.182],
    ["Playboi Carti \u2013 Different Day [PITCH,SPED UP]", "sahara2mil", 154.775],
    ["different day (remix) - playboi carti", "yevixx", 96.571],
  ]) {
    assert.equal(score(carti, candidate(title, uploader, duration)), null, `${title} / ${uploader}`);
  }
});

test("a fan account is not the artist, and a one-word artist matches exactly", () => {
  const carti = track({
    name: "DIFFERENT DAY",
    durationMs: 166_700,
    artists: [{ name: "Playboi Carti" }],
  });
  // Same title and duration, so only the account can decide.
  for (const uploader of ["carti meatrider", "playboicartifan", "cartifan"]) {
    assert.equal(score(carti, candidate("DIFFERENT DAY", uploader, 166.7)), null, uploader);
  }
  // A handle that contains the name as words was already readable evidence;
  // the equality rule never widens that, so an inflected title on a fan
  // account is still not the song.
  for (const uploader of ["Playboi Carti Fan", "Playboi Carti 2"]) {
    assert.equal(score(carti, candidate("DIFFERENT DAYS", uploader, 166.7)), null, uploader);
  }

  const slayr = track();
  assert.notEqual(score(slayr, candidate("Never Go Down", "slayr", 120.0)), null);
  // A handle that merely contains a one-word artist is not that artist.
  for (const uploader of ["slayrfan", "notslayr", "slayrx"]) {
    assert.equal(score(slayr, candidate("Never Go Down", uploader, 120.0)), null, uploader);
  }
});

test("the artist's own upload may inflect one word of the title", () => {
  const slayr = track();
  const inflected = candidate("Never Going Down", "slayr", 120.238, {
    url: "https://soundcloud.com/slayr/never-going-down",
  });
  assert.notEqual(score(slayr, inflected), null);
  // An exact title always wins when the rest is equal.
  const exact = candidate("Never Go Down", "slayr", 120.238);
  assert.ok(score(slayr, exact) > score(slayr, inflected));
  assert.equal(bestMatch(slayr, [inflected, exact]), exact);
});

test("the inflection rule stops at a different account, a slower upload or a different title", () => {
  const slayr = track();
  const title = "Never Going Down";

  // The uploader itself must be the artist, not a credit on someone else's upload.
  assert.equal(score(slayr, candidate(title, "NOVOX", 120.238)), null);
  assert.equal(score(slayr, candidate(title, "NOVOX", 120.238, { credits: ["NOVOX", "slayr"] })), null);

  // Within 1.5 seconds only.
  assert.notEqual(score(slayr, candidate(title, "slayr", 121.5)), null);
  assert.equal(score(slayr, candidate(title, "slayr", 121.6)), null);
  assert.equal(score(slayr, candidate(title, "slayr", 123.2)), null);

  // An extra or a missing word is a different title.
  assert.equal(score(slayr, candidate("Never Going Down Again", "slayr", 120.238)), null);
  assert.equal(score(slayr, candidate("Going Down", "slayr", 120.238)), null);
  assert.equal(score(slayr, candidate("Never Going", "slayr", 120.238)), null);

  // A stem is at most three letters longer and at least two letters long.
  assert.equal(score(slayr, candidate("Never Goinggg Down", "slayr", 120.238)), null);
  assert.equal(score(slayr, candidate("Never G Down", "slayr", 120.238)), null);

  // A version qualifier still makes it another recording, whoever uploaded it.
  assert.equal(score(slayr, candidate("slayr - Never Go Down (Instrumental)", "NOVOX", 120.328, {
    credits: ["NOVOX", "slayr"],
  })), null);
  assert.equal(score(slayr, candidate("Never Going Down (Instrumental)", "slayr", 120.238)), null);
  assert.equal(score(slayr, candidate("never go down remix [slayr] prod. wa", "scxba", 181.305)), null);
});
