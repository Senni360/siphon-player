#!/usr/bin/env node
// Local stand-in for ffmpeg: copies the input to the output and never
// transcodes, which is exactly the contract the resolver relies on.

import fs from "node:fs";

const args = process.argv.slice(2);
const sleep = Number(process.env.FAKE_DELAY_MS ?? 0);
if (sleep > 0) await new Promise((resolve) => setTimeout(resolve, sleep));

const input = args[args.indexOf("-i") + 1];
const output = args[args.length - 1];
if (!input || !output || !fs.existsSync(input)) {
  process.stderr.write("bad ffmpeg invocation\n");
  process.exit(2);
}
fs.copyFileSync(input, output);
const exit = Number(process.env.FAKE_EXIT_CODE ?? 0);
if (exit !== 0) process.exit(exit);
