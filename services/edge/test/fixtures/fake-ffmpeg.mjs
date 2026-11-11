#!/usr/bin/env node
// Local stand-in for ffmpeg: copies the input to the output and never
// transcodes, which is exactly the contract the resolver relies on. With
// `pipe:0`/`pipe:1` it copies stdin to stdout as bytes arrive.

import fs from "node:fs";
import { once } from "node:events";

const args = process.argv.slice(2);
const sleep = Number(process.env.FAKE_DELAY_MS ?? 0);
const input = args[args.indexOf("-i") + 1];
const output = args[args.length - 1];

if (input === "pipe:0" || output === "pipe:1") {
  process.stdin.on("error", () => {});
  process.stdout.on("error", () => process.exit(0));
  for await (const chunk of process.stdin) {
    if (!process.stdout.write(chunk)) await once(process.stdout, "drain").catch(() => {});
  }
} else {
  if (sleep > 0) await new Promise((resolve) => setTimeout(resolve, sleep));
  if (!input || !output || !fs.existsSync(input)) {
    process.stderr.write("bad ffmpeg invocation\n");
    process.exit(2);
  }
  fs.copyFileSync(input, output);
}

const exit = Number(process.env.FAKE_EXIT_CODE ?? 0);
if (exit !== 0) process.exit(exit);
