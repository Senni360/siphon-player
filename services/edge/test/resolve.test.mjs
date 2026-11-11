import assert from "node:assert/strict";
import fs from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { CacheStore } from "../src/cache.mjs";
import { loadConfig } from "../src/config.mjs";
import { buildFfmpegArgs, buildYtDlpArgs, parseSource, ResolveService } from "../src/resolve.mjs";
import { MatchService } from "../src/search.mjs";
import { UnplayableStore } from "../src/unplayable.mjs";
import { fakeSpawn } from "./helpers/fakes.mjs";
import { SECRET, request, startEdge, tempDir } from "./helpers/context.mjs";
import { mintToken } from "../src/tokens.mjs";

const SOURCE = "youtube:video:abcdefghijk";

test("accepts only the two documented source shapes", () => {
  assert.deepEqual(parseSource(SOURCE), {
    provider: "youtube",
    sourceUri: SOURCE,
    url: "https://www.youtube.com/watch?v=abcdefghijk",
  });
  assert.deepEqual(parseSource("soundcloud:artist/track"), {
    provider: "soundcloud",
    sourceUri: "soundcloud:artist/track",
    url: "https://soundcloud.com/artist/track",
  });
  assert.deepEqual(parseSource("https://soundcloud.com/artist/track"), {
    provider: "soundcloud",
    sourceUri: "soundcloud:artist/track",
    url: "https://soundcloud.com/artist/track",
  });
});

test("rejects leading dashes, metacharacters, traversal and arbitrary URLs", () => {
  for (const bad of [
    "-youtube:video:abcdefghijk",
    "youtube:video:short",
    "youtube:video:abcdefghijk$(id)",
    "youtube:video:abcdefghijk; rm -rf /",
    "soundcloud:artist/track;rm",
    "soundcloud:artist/../../etc/passwd",
    "soundcloud:artist/track%2F..",
    "soundcloud:artist/track?x=1",
    "https://evil.example/artist/track",
    "https://soundcloud.com.evil/artist/track",
    "http://soundcloud.com/artist/track",
    "https://user:pass@soundcloud.com/artist/track",
    "https://soundcloud.com/artist/track?secret=1",
    "https://soundcloud.com/artist/../track",
    "file:///etc/passwd",
    "spotify:track:abc",
    " youtube:video:abcdefghijk",
    "youtube:video:abcdefghijk\n",
    "",
    "-",
  ]) {
    assert.throws(() => parseSource(bad), /invalid_source/, JSON.stringify(bad));
  }
});

test("commands use argument arrays and end options before the URL", () => {
  const args = buildYtDlpArgs({ url: "https://www.youtube.com/watch?v=abcdefghijk" });
  assert.equal(args[args.length - 2], "--");
  assert.equal(args[args.length - 1], "https://www.youtube.com/watch?v=abcdefghijk");
  assert.equal(args.includes("bestaudio[ext=m4a]/140/bestaudio"), true);
  assert.equal(args[args.indexOf("--output") + 1], "-", "audio is streamed, not written twice");
  const ffmpeg = buildFfmpegArgs({ format: "m4a" });
  assert.deepEqual(ffmpeg.slice(ffmpeg.indexOf("-c:a"), ffmpeg.indexOf("-c:a") + 2), ["-c:a", "copy"]);
  assert.equal(ffmpeg[ffmpeg.indexOf("-i") + 1], "pipe:0");
  assert.equal(ffmpeg.at(-1), "pipe:1");
  assert.equal(ffmpeg.includes("frag_keyframe+empty_moov+default_base_moof"), true);
});

async function resolveService({ env = {}, unplayable = false } = {}) {
  const dir = await tempDir();
  const cache = new CacheStore({ dir, maxBytes: 10 * 1024 * 1024, now: Date.now });
  await cache.init();
  const config = loadConfig({
    EDGE_TOKEN_SECRET: SECRET,
    EDGE_DATA_DIR: dir,
    EDGE_YTDLP_BIN: "yt-dlp",
    EDGE_FFMPEG_BIN: "ffmpeg",
  });
  const { spawn, state } = fakeSpawn({ env });
  let store = null;
  if (unplayable) {
    store = new UnplayableStore({ dir, logger: { warn() {} } });
    await store.init();
  }
  const service = new ResolveService({ cache, config, logger: { warn() {} }, spawn, unplayable: store });
  return { dir, cache, config, service, state, spawn, unplayable: store };
}
test("resolves a source through yt-dlp and ffmpeg, then serves the cache", async (t) => {
  const setup = await resolveService();
  t.after(async () => {
    await setup.service.close();
    await fs.rm(setup.dir, { recursive: true, force: true });
  });
  const entry = await setup.service.ensure("user-1", SOURCE);
  assert.equal(entry.cached, false);
  assert.equal(entry.contentType, "audio/mp4");
  assert.ok(entry.size > 0);
  assert.equal(setup.state.calls.length, 2);
  const [ytdlp, ffmpeg] = setup.state.calls;
  assert.equal(ytdlp.command, "yt-dlp");
  assert.equal(ytdlp.shell, false);
  assert.equal(ytdlp.args[ytdlp.args.length - 2], "--");
  assert.equal(ytdlp.args.at(-1), "https://www.youtube.com/watch?v=abcdefghijk");
  assert.equal(ffmpeg.command, "ffmpeg");
  const hit = await setup.service.ensure("user-1", SOURCE);
  assert.equal(hit.cached, true);
  assert.equal(setup.state.calls.length, 2);
});

test("identical concurrent requests share one job, and at most two run at once", async (t) => {
  const setup = await resolveService({ env: { FAKE_DELAY_MS: "60" } });
  t.after(async () => {
    await setup.service.close();
    await fs.rm(setup.dir, { recursive: true, force: true });
  });
  const youtube = (index) => `youtube:video:abcdefghij${index}`;
  const shared = await Promise.all([
    setup.service.ensure("user-1", youtube(0)),
    setup.service.ensure("user-1", youtube(0)),
    setup.service.ensure("user-1", youtube(0)),
  ]);
  assert.equal(new Set(shared.map((entry) => entry.key)).size, 1);
  assert.equal(setup.state.calls.filter((call) => call.command === "yt-dlp").length, 1);

  const many = await Promise.all(Array.from({ length: 5 }, (_, index) => setup.service.ensure("user-2", youtube(index + 1))));
  assert.equal(many.length, 5);
  // Two jobs at a time, and each job runs yt-dlp and ffmpeg together.
  assert.ok(setup.state.maxActive <= 4, `max concurrent was ${setup.state.maxActive}`);
  assert.ok(setup.service.stats().running <= 2, `running jobs was ${setup.service.stats().running}`);
});

test("an MP3 source is remuxed as MP3, not transcoded", async (t) => {
  const setup = await resolveService({ env: { FAKE_AUDIO_EXT: "mp3" } });
  t.after(async () => {
    await setup.service.close();
    await fs.rm(setup.dir, { recursive: true, force: true });
  });
  const entry = await setup.service.ensure("user-1", "soundcloud:artist/track");
  assert.equal(entry.contentType, "audio/mpeg");
  assert.match(entry.key, /^c[0-9a-f]{56}$/);
  const cacheEntry = setup.cache.get(entry.key);
  assert.match(cacheEntry.file, /\.mp3$/);
  const ffmpeg = setup.state.calls.find((call) => call.command === "ffmpeg");
  assert.equal(ffmpeg.args[ffmpeg.args.length - 2], "mp3");
});

test("warm prepares at most two upcoming songs and rejects bad sources", async (t) => {
  const setup = await resolveService();
  t.after(async () => {
    await setup.service.close();
    await fs.rm(setup.dir, { recursive: true, force: true });
  });
  await setup.service.ensure("user-1", SOURCE);
  const result = setup.service.warm("user-1", [
    SOURCE,
    "not valid; rm -rf /",
    "youtube:video:bbbbbbbbbbb",
    "youtube:video:ccccccccccc",
    "youtube:video:ddddddddddd",
  ]);
  assert.equal(result.warmed.length, 2);
  assert.equal(result.warmed[0].state, "cached");
  assert.equal(result.warmed[1].state, "queued");
  assert.equal(result.rejected, 1);
  await new Promise((resolve) => setTimeout(resolve, 300));
  assert.equal(setup.state.calls.filter((call) => call.command === "yt-dlp").length, 2);
});

test("a tool failure is a resolve error, never a crash", async (t) => {
  const setup = await resolveService({ env: { FAKE_EXIT_CODE: "1" } });
  t.after(async () => {
    await setup.service.close();
    await fs.rm(setup.dir, { recursive: true, force: true });
  });
  await assert.rejects(setup.service.ensure("user-1", SOURCE), (error) => error.code === "resolve_failed" && error.status === 502);
});

test("a DRM protected source is classified, remembered and refused without a tool", async (t) => {
  const setup = await resolveService({
    env: { FAKE_STDERR: "ERROR: [soundcloud] 123: This video is DRM protected" },
    unplayable: true,
  });
  t.after(async () => {
    await setup.service.close();
    await fs.rm(setup.dir, { recursive: true, force: true });
  });
  await assert.rejects(
    setup.service.ensure("user-1", "soundcloud:artist/track"),
    (error) => error.code === "drm_protected" && error.status === 409,
  );
  const entry = setup.unplayable.get("soundcloud:artist/track");
  assert.equal(entry.reason, "drm_protected");
  assert.equal(typeof entry.at, "number");

  const calls = setup.state.calls.length;
  assert.throws(
    () => setup.service.ensure("user-1", "soundcloud:artist/track"),
    (error) => error.code === "drm_protected" && error.status === 409,
  );
  assert.equal(setup.state.calls.length, calls, "no tool starts for a known unplayable source");

  const warmed = setup.service.warm("user-1", ["soundcloud:artist/track", "soundcloud:artist/other"]);
  assert.deepEqual(warmed.warmed.map((item) => item.state), ["queued"]);
  assert.equal(warmed.rejected, 1);
});

test("a failure that is not DRM keeps the generic resolve classification", async (t) => {
  const setup = await resolveService({
    env: { FAKE_STDERR: "ERROR: Unsupported URL: https://soundcloud.com/artist/track", FAKE_EXIT_CODE: "1" },
    unplayable: true,
  });
  t.after(async () => {
    await setup.service.close();
    await fs.rm(setup.dir, { recursive: true, force: true });
  });
  await assert.rejects(
    setup.service.ensure("user-1", "soundcloud:artist/track"),
    (error) => error.code === "resolve_failed" && error.status === 502,
  );
  assert.equal(setup.unplayable.has("soundcloud:artist/track"), false);
});

test("a marked source is refused before yt-dlp starts", async (t) => {
  const setup = await resolveService({ unplayable: true });
  t.after(async () => {
    await setup.service.close();
    await fs.rm(setup.dir, { recursive: true, force: true });
  });
  await setup.unplayable.add(SOURCE, "drm_protected");
  assert.throws(
    () => setup.service.ensure("user-1", SOURCE),
    (error) => error.code === "drm_protected" && error.status === 409,
  );
  assert.equal(setup.state.calls.length, 0);
});

test("search finds a match, prefers a SoundCloud tie, and never guesses", async () => {
  const dir = await tempDir();
  const youtube = path.join(dir, "youtube.json");
  const soundcloud = path.join(dir, "soundcloud.json");
  await fs.writeFile(youtube, JSON.stringify({
    entries: [{
      webpage_url: "https://www.youtube.com/watch?v=abcdefghijk",
      title: "Aurora - Northern Lights",
      uploader: "Aurora",
      duration: 200,
    }],
  }));
  await fs.writeFile(soundcloud, JSON.stringify({
    entries: [{
      webpage_url: "https://soundcloud.com/aurora/northern-lights",
      title: "Aurora - Northern Lights",
      uploader: "Aurora",
      duration: 200.3,
    }],
  }));
  const { spawn } = fakeSpawn({ env: { FAKE_SEARCH_YOUTUBE: youtube, FAKE_SEARCH_SOUNDCLOUD: soundcloud } });
  const matcher = new MatchService({
    config: { ytdlpBin: "yt-dlp", searchTimeoutMs: 10_000 },
    logger: { warn() {} },
    spawn,
  });
  const result = await matcher.match("user-1", {
    uri: "spotify:track:abc",
    name: "Northern Lights",
    durationMs: 200_000,
    artists: [{ name: "Aurora" }],
  });
  assert.equal(result.match.provider, "soundcloud");
  assert.equal(result.match.source, "soundcloud:aurora/northern-lights");
  assert.ok(result.match.score > 0.98 && result.match.score < 1);

  const none = await matcher.match("user-1", {
    uri: "spotify:track:def",
    name: "A Different Song",
    durationMs: 200_000,
    artists: [{ name: "Aurora" }],
  });
  assert.equal(none.match, null);
  await fs.rm(dir, { recursive: true, force: true });
});

test("HTTP resolve returns a media URL that plays, and warm answers 202", async (t) => {
  const { spawn } = fakeSpawn();
  const ctx = await startEdge({ spawn });
  t.after(() => ctx.close());
  const token = mintToken({ sub: "phone-user", role: "phone", scope: "media", ttlSeconds: 120 }, SECRET);
  const resolved = await request(ctx.base, "/v1/resolve", { method: "POST", token, body: { source: SOURCE } });
  assert.equal(resolved.status, 200);
  const body = await resolved.json();
  assert.match(body.key, /^c[0-9a-f]{56}$/);
  const media = await fetch(`${ctx.base}${body.url}`);
  assert.equal(media.status, 200);
  assert.ok((await media.arrayBuffer()).byteLength > 0);
  assert.equal((await request(ctx.base, "/v1/resolve", { method: "POST", token, body: { source: "https://evil.example/x" } })).status, 400);
  const warm = await request(ctx.base, "/v1/warm", {
    method: "POST",
    token,
    body: { sources: ["youtube:video:bbbbbbbbbbb", "youtube:video:ccccccccccc", "youtube:video:ddddddddddd"] },
  });
  assert.equal(warm.status, 202);
  assert.equal((await warm.json()).warmed.length, 2);
});
