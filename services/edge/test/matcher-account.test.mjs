// An artist's own account may spell the name without its spaces: the owner's
// real playlist has Playboi Carti's own upload on the account `playboicarti`.
// The reading is exact, and accounts near it stay rejected.
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

  const slayr = track();
  assert.notEqual(score(slayr, candidate("Never Go Down", "slayr", 120.0)), null);
  // A handle that merely contains a one-word artist is not that artist.
  for (const uploader of ["slayrfan", "notslayr", "slayrx"]) {
    assert.equal(score(slayr, candidate("Never Go Down", uploader, 120.0)), null, uploader);
  }
});
