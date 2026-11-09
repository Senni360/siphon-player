// Child processes are always spawned with an argument array and shell:false.
// Standard error is never logged or returned: a tool's stderr can contain a
// private link or a token, and none of that belongs in a log line or response.

import { spawn as nodeSpawn } from "node:child_process";

export class ProcessError extends Error {
  constructor(code) {
    super(code);
    this.name = "ProcessError";
    this.code = code;
  }
}

const DEFAULT_STDOUT_LIMIT = 4 * 1024 * 1024;

export function runProcess(command, args, {
  timeoutMs = 120_000,
  maxStdoutBytes = DEFAULT_STDOUT_LIMIT,
  spawn = nodeSpawn,
  onSpawn,
} = {}) {
  if (!Array.isArray(args) || args.some((arg) => typeof arg !== "string")) {
    return Promise.reject(new ProcessError("invalid_arguments"));
  }
  return new Promise((resolve, reject) => {
    let child;
    try {
      child = spawn(command, args, {
        shell: false,
        windowsHide: true,
        stdio: ["ignore", "pipe", "pipe"],
        env: Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("EDGE_"))),
      });
    } catch {
      reject(new ProcessError("spawn_failed"));
      return;
    }
    let stdout = "";
    let stdoutBytes = 0;
    let settled = false;
    try {
      onSpawn?.(child);
    } catch {
      // A registry callback must not change the outcome.
    }
    const finish = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (error) reject(error);
      else resolve(value);
    };
    const timer = setTimeout(() => {
      try {
        child.kill("SIGKILL");
      } catch {
        // Already gone.
      }
      finish(new ProcessError("timeout"));
    }, timeoutMs);
    timer.unref?.();
    child.stdout?.on("data", (chunk) => {
      if (stdoutBytes >= maxStdoutBytes) return;
      const remaining = maxStdoutBytes - stdoutBytes;
      const text = chunk.toString("utf8", 0, Math.min(chunk.length, remaining));
      stdout += text;
      stdoutBytes += Buffer.byteLength(text);
    });
    // Drain stderr so a noisy tool cannot block on a full pipe.
    child.stderr?.on("data", () => {});
    child.on("error", () => finish(new ProcessError("spawn_failed")));
    child.on("close", (code, signal) => {
      if (signal) finish(new ProcessError("terminated"));
      else if (code === 0) finish(null, { stdout, code });
      else finish(new ProcessError(`exit_${code}`));
    });
  });
}
