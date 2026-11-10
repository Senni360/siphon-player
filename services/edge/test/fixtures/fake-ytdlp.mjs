#!/usr/bin/env node
// Local stand-in for yt-dlp. It never touches the network: it writes a small
// fixture instead of a download, or prints a prepared search JSON file. With
// `-o -` it streams those bytes to stdout in chunks, which is what the resolver
// asks for in production.

import fs from "node:fs";
import { once } from "node:events";

const args = process.argv.slice(2);
const sleep = Number(process.env.FAKE_DELAY_MS ?? 0);

function valueAfter(flag) {
  const index = args.indexOf(flag);
  return index >= 0 ? args[index + 1] : undefined;
}

function fixtureBytes(total) {
  const ext = process.env.FAKE_AUDIO_EXT ?? "m4a";
  const head = ext === "mp3" || ext === "ogg" || ext === "flac"
    ? fixtureHead(ext)
    : Buffer.concat([Buffer.from([0, 0, 0, 0x18]), Buffer.from("ftypM4A ")]);
  const body = Buffer.alloc(Math.max(0, total - head.length));
  for (let index = 0; index < body.length; index += 1) body[index] = index % 251;
  return Buffer.concat([head, body]);
}

function fixtureHead(ext) {
  if (ext === "mp3") return Buffer.concat([Buffer.from("ID3"), Buffer.alloc(9, 0x21)]);
  if (ext === "ogg") return Buffer.concat([Buffer.from("OggS"), Buffer.alloc(8, 0x11)]);
  if (ext === "flac") return Buffer.concat([Buffer.from("fLaC"), Buffer.alloc(8, 0x12)]);
  return Buffer.concat([Buffer.from([0, 0, 0, 0x18]), Buffer.from("ftypM4A ")]);
}

if (args.includes("--dump-single-json")) {
  const url = args[args.length - 1] ?? "";
  const key = (url.startsWith("scsearch") || url.includes("soundcloud.com")) ? "FAKE_SEARCH_SOUNDCLOUD" : "FAKE_SEARCH_YOUTUBE";
  const file = process.env[key];
  process.stdout.write(file && fs.existsSync(file) ? fs.readFileSync(file, "utf8") : "{\"entries\":[]}");
} else if (valueAfter("--output") === "-") {
  if (sleep > 0) await new Promise((resolve) => setTimeout(resolve, sleep));
  const total = Number(process.env.FAKE_STREAM_BYTES ?? 4096);
  const chunkSize = Math.max(1, Number(process.env.FAKE_STREAM_CHUNK ?? 1024));
  const delay = Number(process.env.FAKE_STREAM_DELAY_MS ?? 0);
  const bytes = fixtureBytes(total);
  process.stdout.on("error", () => process.exit(0));
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    if (!process.stdout.write(bytes.subarray(offset, offset + chunkSize))) {
      await once(process.stdout, "drain").catch(() => {});
    }
    if (delay > 0) await new Promise((resolve) => setTimeout(resolve, delay));
  }
} else {
  const output = valueAfter("--output");
  if (!output) {
    process.stderr.write("no output target\n");
    process.exit(2);
  }
  const ext = process.env.FAKE_AUDIO_EXT ?? "m4a";
  const file = output.replace("%(ext)s", ext);
  const head = ext === "mp3"
    ? Buffer.concat([Buffer.from("ID3"), Buffer.alloc(96, 0x21)])
    : Buffer.concat([Buffer.from([0, 0, 0, 0x18]), Buffer.from("ftypM4A "), Buffer.alloc(96, 0x41)]);
  fs.writeFileSync(file, head);
}

const exit = Number(process.env.FAKE_EXIT_CODE ?? 0);
if (exit !== 0) process.exit(exit);
