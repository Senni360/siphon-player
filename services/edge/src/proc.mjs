// Child processes are always spawned with an argument array and shell:false.
// Standard error is never logged or returned: a tool's stderr can contain a
// private link or a token, and none of that belongs in a log line or response.
//
// On POSIX a child is started in its own process group, so a cancelled job can
// take down helpers it spawned as well. Windows has no process groups to signal
// this way, so the direct child is terminated instead.
//
// Every child is supervised: a timeout, an abort signal, a cancelled waiter or
// a shutdown all reach the same kill path, and the returned promise always
// settles even when a grandchild keeps a pipe open.

import { spawn as nodeSpawn } from "node:child_process";

export class ProcessError extends Error {
  constructor(code) {
    super(code);
    this.name = "ProcessError";
    this.code = code;
  }
}

const DEFAULT_STDOUT_LIMIT = 4 * 1024 * 1024;

/** The environment a tool may see: never an EDGE_ variable. */
export function childEnvironment(env = process.env) {
  return Object.fromEntries(Object.entries(env).filter(([name]) => !name.startsWith("EDGE_")));
}

export function spawnOptions({ stdio = ["ignore", "pipe", "pipe"], detached = true } = {}) {
  return {
    shell: false,
    windowsHide: true,
    stdio,
    // A new process group lets cancellation reach grandchildren on POSIX.
    detached: detached && process.platform !== "win32",
    env: childEnvironment(),
  };
}

/** Terminates a child and, on POSIX, everything it started. */
export function killTree(child, { platform = process.platform, kill } = {}) {
  if (!child || typeof child.pid !== "number" || child.pid <= 0) return;
  if (child.exitCode !== null || child.signalCode) return;
  const signal = kill ?? process.kill.bind(process);
  if (platform !== "win32") {
    try {
      // The child leads its own group, so the negative pid is the whole tree.
      signal(-child.pid, "SIGKILL");
      return;
    } catch {
      // Fall through to the single process.
    }
  }
  try {
    child.kill("SIGKILL");
  } catch {
    // Already gone.
  }
}

function validateArgs(args) {
  return Array.isArray(args) && !args.some((arg) => typeof arg !== "string");
}

/**
 * Starts a child and returns `{ child, done }` immediately. `done` settles
 * exactly once: on exit, on `timeoutMs`, on `signal` abort, or on a spawn
 * error. On any failure the whole process tree is killed first, so a tool that
 * ignores its direct child cannot pin the caller forever.
 */
export function startProcess(command, args, {
  timeoutMs = 120_000,
  spawn = nodeSpawn,
  onSpawn,
  signal,
  stdio,
} = {}) {
  if (!validateArgs(args)) {
    return { child: null, done: Promise.reject(new ProcessError("invalid_arguments")) };
  }
  let child;
  try {
    child = spawn(command, args, spawnOptions({ stdio }));
  } catch {
    return { child: null, done: Promise.reject(new ProcessError("spawn_failed")) };
  }
  let settled = false;
  let timer = null;
  let onAbort = null;
  const done = new Promise((resolve, reject) => {
    const cleanup = () => {
      if (timer) {
        clearTimeout(timer);
        timer = null;
      }
      if (onAbort) signal?.removeEventListener("abort", onAbort);
    };
    const finish = (error) => {
      if (settled) return;
      settled = true;
      cleanup();
      if (error) reject(error);
      else resolve({ code: child.exitCode, signal: child.signalCode });
    };
    onAbort = () => {
      killTree(child);
      finish(new ProcessError("cancelled"));
    };
    if (signal) {
      if (signal.aborted) {
        onAbort();
        return;
      }
      signal.addEventListener("abort", onAbort, { once: true });
    }
    timer = setTimeout(() => {
      killTree(child);
      finish(new ProcessError("timeout"));
    }, timeoutMs);
    timer.unref?.();
    child.on("error", () => finish(new ProcessError("spawn_failed")));
    child.on("close", (code, closeSignal) => {
      if (closeSignal) finish(new ProcessError("terminated"));
      else if (code === 0) finish(null);
      else finish(new ProcessError(`exit_${code}`));
    });
  });
  try {
    onSpawn?.(child);
  } catch {
    // A registry callback must not change the outcome.
  }
  return { child, done };
}

/**
 * Runs a child to completion and collects at most `maxStdoutBytes` of stdout.
 * `maxStdoutBytes: 0` drains nothing into memory, which is what a job whose
 * output only goes to a file or another pipe wants.
 */
export function runProcess(command, args, {
  timeoutMs = 120_000,
  maxStdoutBytes = DEFAULT_STDOUT_LIMIT,
  spawn = nodeSpawn,
  onSpawn,
  signal,
} = {}) {
  const { child, done } = startProcess(command, args, { timeoutMs, spawn, onSpawn, signal });
  if (!child) return done;
  let stdout = "";
  let stdoutBytes = 0;
  child.stdout?.on("data", (chunk) => {
    if (stdoutBytes >= maxStdoutBytes) return;
    const remaining = maxStdoutBytes - stdoutBytes;
    const text = chunk.toString("utf8", 0, Math.min(chunk.length, remaining));
    stdout += text;
    stdoutBytes += Buffer.byteLength(text);
  });
  // Drain stderr so a noisy tool cannot block on a full pipe.
  child.stderr?.on("data", () => {});
  return done.then((value) => ({ ...value, stdout }));
}
