#!/usr/bin/env node
// Local stand-in for yt-dlp. It never touches the network: it writes a small
// fixture instead of a download, or prints a prepared search JSON file.

import fs from "node:fs";

const args = process.argv.slice(2);
const sleep = Number(process.env.FAKE_DELAY_MS ?? 0);
if (sleep > 0) await new Promise((resolve) => setTimeout(resolve, sleep));

function valueAfter(flag) {
  const index = args.indexOf(flag);
  return index >= 0 ? args[index + 1] : undefined;
}

function writeFixture(target) {
  const ext = process.env.FAKE_AUDIO_EXT ?? "m4a";
  const file = target.replace("%(ext)s", ext);
  const head = ext === "mp3"
    ? Buffer.concat([Buffer.from("ID3"), Buffer.alloc(96, 0x21)])
    : Buffer.concat([Buffer.from([0, 0, 0, 0x18]), Buffer.from("ftypM4A "), Buffer.alloc(96, 0x41)]);
  fs.writeFileSync(file, head);
}

if (args.includes("--dump-single-json")) {
  const url = args[args.length - 1] ?? "";
  const key = (url.startsWith("scsearch") || url.includes("soundcloud.com")) ? "FAKE_SEARCH_SOUNDCLOUD" : "FAKE_SEARCH_YOUTUBE";
  const file = process.env[key];
  process.stdout.write(file && fs.existsSync(file) ? fs.readFileSync(file, "utf8") : "{\"entries\":[]}");
} else {
  const output = valueAfter("--output");
  if (!output) {
    process.stderr.write("no output target\n");
    process.exit(2);
  }
  writeFixture(output);
}

const exit = Number(process.env.FAKE_EXIT_CODE ?? 0);
if (exit !== 0) process.exit(exit);
