// A resolve must be interruptible: a queued job that nobody waits for never
// starts, a running job has its whole process tree killed, and a hung tool
// hits the timeout. These use a fake spawn so no real tool is needed.
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import fs from "node:fs/promises";
import { PassThrough } from "node:stream";
import test from "node:test";
import { CacheStore } from "../src/cache.mjs";
import { killTree } from "../src/proc.mjs";
import { ResolveService } from "../src/resolve.mjs";
import { SECRET, tempDir } from "./helpers/context.mjs";

const SOURCE = "youtube:video:abcdefghijk";
const YOUTUBE = (index) => `youtube:video:abcdefghij${index}`;
const SOURCE_N = (index) => `youtube:video:${String(index).padStart(11, "0")}`;

function fakeChild(pid, kills) {
  const child = new EventEmitter();
  child.pid = pid;
  child.exitCode = null;
  child.signalCode = null;
  child.stdout = new PassThrough();
  child.stderr = new PassThrough();
  child.stdin = new PassThrough();
  child.kill = (signal) => {
    kills.push(signal);
    child.exitCode = 0;
    child.signalCode = signal;
    child.emit("exit", 0, signal);
    child.emit("close", 0, signal);
    return true;
  };
  return child;
}

/** A spawn whose children never finish on their own. */
function hangingSpawn() {
  const kills = [];
  const calls = [];
  const spawn = (command, args) => {
    calls.push({ command, args });
    return fakeChild(10_000 + calls.length, kills);
  };
  return { spawn, kills, calls };
}

async function serviceFor({ concurrency = 1, timeout = 5_000, spawn, dir } = {}) {
  const dataDir = dir ?? await tempDir();
  const cache = new CacheStore({ dir: dataDir, maxBytes: 10 * 1024 * 1024, now: Date.now });
  await cache.init();
  const config = {
    dataDir,
    resolveConcurrency: concurrency,
    resolveTimeoutMs: timeout,
    ffmpegTimeoutMs: timeout,
    warmQueueMax: 4,
    ytdlpBin: "yt-dlp",
    ffmpegBin: "ffmpeg",
  };
  const service = new ResolveService({ cache, config, logger: { warn() {} }, spawn });
  await service.init();
  return { service, cache, dir: dataDir };
}

test("a queued job whose only caller leaves never starts", async (t) => {
  const { spawn, calls } = hangingSpawn();
  const setup = await serviceFor({ spawn });
  t.after(async () => {
    await setup.service.close();
    await fs.rm(setup.dir, { recursive: true, force: true });
  });
  const running = setup.service.ensure("user", SOURCE);
  running.catch(() => {});
  const controller = new AbortController();
  const queued = setup.service.ensure("user", YOUTUBE(1), { signal: controller.signal });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(setup.service.stats().queued, 1);
  controller.abort();
  await assert.rejects(queued, (error) => error.code === "cancelled");
  assert.equal(setup.service.stats().queued, 0);
  assert.equal(calls.length, 1, "only the running job ever spawned a tool");

  await setup.service.close();
  await assert.rejects(running, (error) => error.code === "shutting_down");
});

test("leaving a running job kills its process tree", async (t) => {
  const { spawn, kills } = hangingSpawn();
  const setup = await serviceFor({ spawn });
  t.after(async () => {
    await setup.service.close();
    await fs.rm(setup.dir, { recursive: true, force: true });
  });
  const controller = new AbortController();
  const promise = setup.service.ensure("user", SOURCE, { signal: controller.signal });
  await new Promise((resolve) => setImmediate(resolve));
  controller.abort();
  await assert.rejects(promise, (error) => error.code === "cancelled");
  await new Promise((resolve) => setImmediate(resolve));
  assert.ok(kills.includes("SIGKILL"), "the child was killed");
  assert.equal(setup.service.stats().running, 0);
});

test("a tool that never exits hits its timeout and is killed", async (t) => {
  const { spawn, kills } = hangingSpawn();
  const setup = await serviceFor({ spawn, timeout: 60 });
  t.after(async () => {
    await setup.service.close();
    await fs.rm(setup.dir, { recursive: true, force: true });
  });
  await assert.rejects(setup.service.ensure("user", SOURCE), (error) => {
    assert.equal(error.code, "resolve_failed");
    assert.equal(error.status, 502);
    return true;
  });
  assert.ok(kills.includes("SIGKILL"));
});

test("a user tap preempts a prefetch and the warm queue stays capped", async (t) => {
  const { spawn, kills, calls } = hangingSpawn();
  const setup = await serviceFor({ spawn });
  t.after(async () => {
    await setup.service.close();
    await fs.rm(setup.dir, { recursive: true, force: true });
  });
  setup.service.warm("user", [SOURCE_N(1)]);
  for (let index = 2; index <= 20; index += 1) setup.service.warm("user", [SOURCE_N(index)]);
  assert.ok(setup.service.stats().queued <= 4, `warm queue was ${setup.service.stats().queued}`);

  const controller = new AbortController();
  const tap = setup.service.ensure("user", SOURCE, { signal: controller.signal });
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.ok(kills.length >= 1, "the running prefetch was killed for the tap");
  assert.equal(calls.filter((call) => call.command === "yt-dlp").length >= 2, true);
  assert.equal(setup.service.stats().running, 1);
  controller.abort();
  await assert.rejects(tap, (error) => error.code === "cancelled");
});

test("killTree signals the whole group on POSIX and the child elsewhere", () => {
  const child = { pid: 4242, exitCode: null, signalCode: null, kill() {} };
  const posix = [];
  killTree(child, { platform: "linux", kill: (pid, signal) => posix.push([pid, signal]) });
  assert.deepEqual(posix, [[-4242, "SIGKILL"]]);

  let direct = 0;
  const windows = { pid: 4243, exitCode: null, signalCode: null, kill() { direct += 1; } };
  killTree(windows, { platform: "win32", kill: () => { throw new Error("no groups on win32"); } });
  assert.equal(direct, 1);
});
