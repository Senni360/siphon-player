import { spawn as nodeSpawn } from "node:child_process";
import { fileURLToPath } from "node:url";

export const FAKE_YTDLP = fileURLToPath(new URL("../fixtures/fake-ytdlp.mjs", import.meta.url));
export const FAKE_FFMPEG = fileURLToPath(new URL("../fixtures/fake-ffmpeg.mjs", import.meta.url));

/**
 * Routes the two tool names to small local Node scripts and records how many
 * run at once. No shell is involved, and nothing leaves the machine.
 */
export function fakeSpawn({ env = {} } = {}) {
  const state = { calls: [], active: 0, maxActive: 0 };
  function spawn(command, args, options) {
    state.calls.push({ command, args, shell: options?.shell });
    let script = null;
    if (command === "yt-dlp") script = FAKE_YTDLP;
    else if (command === "ffmpeg") script = FAKE_FFMPEG;
    const child = script
      ? nodeSpawn(process.execPath, [script, ...args], {
          ...options,
          env: { ...process.env, ...env },
        })
      : nodeSpawn(command, args, options);
    if (script) {
      state.active += 1;
      state.maxActive = Math.max(state.maxActive, state.active);
      child.once("exit", () => {
        state.active -= 1;
      });
    }
    return child;
  }
  return { spawn, state };
}
