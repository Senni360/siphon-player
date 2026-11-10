import assert from "node:assert/strict";
import test from "node:test";
import { buildSoundCloudSearchUrl, candidatesFromProbe } from "../src/search.mjs";

test("SoundCloud search asks for thirty rows in one request", () => {
  assert.equal(buildSoundCloudSearchUrl("Playboi Carti - Sky"), "scsearch30:Playboi Carti - Sky");
  assert.equal(buildSoundCloudSearchUrl("A\nB"), "scsearch30:A B");
});

test("a probe row carries publisher credits, verification and ISRC", () => {
  const [candidate] = candidatesFromProbe(
    {
      entries: [
        {
          title: "Sky",
          webpage_url: "https://soundcloud.com/opium/sky",
          uploader: "OPIUM",
          artists: ["Playboi Carti"],
          duration: 195.9,
          channel_is_verified: true,
          isrc: "USUM72100001",
        },
      ],
    },
    "soundcloud",
  );
  assert.equal(candidate.title, "Sky");
  assert.equal(candidate.uploader, "OPIUM");
  assert.deepEqual(candidate.credits, ["OPIUM", "Playboi Carti"]);
  assert.equal(candidate.verified, true);
  assert.equal(candidate.isrc, "USUM72100001");
  assert.equal(candidate.duration, 195.9);
});

test("a probe row without credits stays an uploader-only candidate", () => {
  const [candidate] = candidatesFromProbe(
    [{ title: "Sky", webpage_url: "https://soundcloud.com/carti/sky", uploader: "Playboi Carti" }],
    "soundcloud",
  );
  assert.deepEqual(candidate.credits, ["Playboi Carti"]);
  assert.equal(candidate.verified, false);
  assert.equal(candidate.isrc, null);
});
