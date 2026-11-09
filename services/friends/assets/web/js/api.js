// Talking to the group service.
//
// One place holds the device secret and group header, the per-call timeout and
// the difference between "nobody answered" and "Encore said no". Nothing here
// decides what a call means: callers get the parsed answer or a typed error.
//
// Only GETs are retried, because asking again is safe for them and can repeat
// a POST's effect. A timeout is not a refusal, so a timed-out GET may be tried
// again a bounded number of times; a POST is never retried here.

/** The network never reached the service. */
export class Offline extends Error {
  constructor(message = "Encore couldn't be reached. Check your connection.", options) {
    super(message, options);
    this.name = 'Offline';
  }
}

/** The service will not accept this device's secret any more. */
export class SignedOut extends Error {
  constructor(message = 'This phone was signed out. Join the group again.') {
    super(message);
    this.name = 'SignedOut';
  }
}

/** The service did not answer within this call's patience. */
export class TimedOut extends Error {
  constructor(ms) {
    super(`Encore didn't answer within ${Math.round(ms / 1000)}s.`);
    this.name = 'TimedOut';
    this.timeout = ms;
  }
}

/** The service answered, and the answer was no. */
export class Refused extends Error {
  constructor(status, message = 'Something went wrong.', body = null) {
    super(message);
    this.name = 'Refused';
    this.status = status;
    this.body = body;
  }
}

/** The service asked us to slow down. `retryAfter` is in seconds, or null. */
export class RateLimited extends Refused {
  constructor(retryAfter = null, message = 'Too many requests. Wait a moment.', body = null) {
    super(429, message, body);
    this.name = 'RateLimited';
    this.retryAfter = retryAfter;
  }
}

const SIGN_IN = 'Sign in to the group';

/** Seconds from a Retry-After header, whether it is a count or a date. */
export function retryAfterSeconds(header, now = Date.now()) {
  if (header === null || header === undefined || header === '') return null;
  const seconds = Number(header);
  if (Number.isFinite(seconds) && seconds >= 0) return seconds;
  const when = Date.parse(header);
  return Number.isFinite(when) ? Math.max(0, (when - now) / 1000) : null;
}

const cancelled = signal => {
  const reason = signal?.reason;
  if (reason instanceof Error) return reason;
  const error = new Error('The request was cancelled.');
  error.name = 'AbortError';
  return error;
};

/**
 * A fetch wrapper for the group service.
 *
 * @param {object} [options]
 * @param {string} [options.base] Prefix for every path. Defaults to `/api`.
 * @param {() => {secret?: string, group?: string}} [options.credentials]
 *   Read fresh for every call, so a sign-in or group change needs no rebuild.
 * @param {typeof fetch} [options.fetch] The fetch to use.
 * @param {number} [options.timeout] Patience per attempt, in ms. 0 or null waits.
 * @param {number} [options.retries] Bounded GET retries. Defaults to 2.
 * @param {number} [options.retryBase] First retry wait, in ms.
 * @param {number} [options.retryMax] Longest retry wait, in ms.
 * @param {number} [options.maxRetryWait] Never sleep longer than this for a
 *   Retry-After; a longer request is refused now instead.
 * @param {() => number} [options.random] Jitter source, for tests.
 * @param {(ms: number) => Promise<void>} [options.sleep] Retry wait, for tests.
 * @param {(credentials: object) => Promise<boolean>} [options.confirmSignedOut]
 *   Ask the service once more whether this secret was really removed. The
 *   service's first 401 is never enough on its own: proxies and odd answers can
 *   produce one. When omitted, the service's own "Sign in to the group" body
 *   is taken as its word, exactly as the page did before.
 */
export function createApi({
  base = '/api',
  credentials = () => ({}),
  fetch = globalThis.fetch,
  timeout = 15000,
  retries = 2,
  retryBase = 300,
  retryMax = 5000,
  maxRetryWait = 10000,
  random = Math.random,
  sleep = ms => new Promise(resolve => setTimeout(resolve, ms)),
  confirmSignedOut = null,
} = {}) {
  if (typeof fetch !== 'function') throw new TypeError('createApi needs a fetch');

  const pause = attempt => {
    const step = Math.min(retryMax, retryBase * 2 ** attempt);
    return Math.max(0, Math.round(step * (0.5 + random() * 0.5)));
  };

  const readCredentials = () => {
    try {
      return credentials() ?? {};
    } catch {
      return {};  // a browser that hides storage still gets to ask politely
    }
  };

  /** One attempt: fetch, wait for the body, and turn the answer into a value or an error. */
  async function once(url, method, payload, headers, signal, ms, fetchOptions) {
    const controller = new AbortController();
    let timedOut = false;
    let timer = null;
    const stop = () => controller.abort(signal.reason);
    if (signal) {
      if (signal.aborted) throw cancelled(signal);
      signal.addEventListener('abort', stop, { once: true });
    }
    if (ms > 0) {
      timer = setTimeout(() => {
        timedOut = true;
        controller.abort(new TimedOut(ms));
      }, ms);
    }

    try {
      let answer;
      try {
        answer = await fetch(url, {
          ...fetchOptions,
          method,
          headers,
          body: payload,
          signal: controller.signal,
        });
      } catch (error) {
        if (timedOut) throw new TimedOut(ms);
        if (signal?.aborted) throw cancelled(signal);
        if (error instanceof Refused) throw error;
        throw new Offline(undefined, { cause: error });
      }
      if (timedOut) throw new TimedOut(ms);

      let text = '';
      try {
        text = await answer.text();
      } catch (error) {
        if (timedOut) throw new TimedOut(ms);
        if (signal?.aborted) throw cancelled(signal);
        throw new Offline(undefined, { cause: error });
      }

      let value;
      let readable = true;
      if (text) {
        try {
          value = JSON.parse(text);
        } catch {
          readable = false;
        }
      } else {
        value = null;
      }

      if (answer.ok) {
        if (!readable) throw new Refused(answer.status, 'Encore sent an answer this page could not read.', text);
        if (signal?.aborted) throw cancelled(signal);
        return value;
      }

      const said = readable && value && typeof value === 'object' ? value : null;
      const message = said?.error || `Encore said no (${answer.status}).`;

      if (answer.status === 401) {
        // Only the service's own wording counts. A proxy's login page, a 5xx or
        // any other refusal holds the sign-in and is reported as a refusal.
        if (said?.error !== SIGN_IN) throw new Refused(401, message, said ?? text);
        if (confirmSignedOut) {
          let gone = false;
          try {
            gone = await confirmSignedOut(readCredentials());
          } catch {
            gone = false;  // asking again said nothing for certain: hold the sign-in
          }
          if (!gone) throw new Refused(401, message, said ?? text);
        }
        throw new SignedOut();
      }
      if (answer.status === 429) {
        const after = retryAfterSeconds(answer.headers?.get?.('retry-after') ?? null);
        throw new RateLimited(after, message, said ?? text);
      }
      throw new Refused(answer.status, message, said ?? text);
    } finally {
      clearTimeout(timer);
      if (signal) signal.removeEventListener('abort', stop);
    }
  }

  /**
   * Make a request.
   *
   * @param {string} path Path below `base`, or an absolute URL.
   * @param {object} [options]
   * @param {string} [options.method] Defaults to GET.
   * @param {unknown} [options.body] Objects are sent as JSON.
   * @param {number} [options.timeout] Overrides the per-attempt patience.
   * @param {AbortSignal} [options.signal] Aborts the call (and never retries it).
   * @param {boolean|number|object} [options.retry] Retries for this call:
   *   false/number, or `{ retries, base, max, maxWait }`. Only GETs retry.
   * @param {Record<string,string>} [options.headers]
   * @param {object} [options.fetch] Extra fetch options for this call.
   */
  async function request(path, options = {}) {
    const method = String(options.method ?? 'GET').toUpperCase();
    const url = /^https?:\/\//i.test(path) ? path : `${base}${path}`;
    const given = readCredentials();
    const headers = { accept: 'application/json', ...(options.headers ?? {}) };
    if (given.secret) headers.authorization = `Bearer ${given.secret}`;
    if (given.group) headers['x-siphon-group'] = given.group;

    let payload;
    if (options.body !== undefined) {
      const raw = typeof options.body === 'string'
        || (typeof FormData !== 'undefined' && options.body instanceof FormData)
        || (typeof Blob !== 'undefined' && options.body instanceof Blob)
        || (typeof URLSearchParams !== 'undefined' && options.body instanceof URLSearchParams);
      if (raw) {
        payload = options.body;
      } else {
        payload = JSON.stringify(options.body);
        headers['content-type'] = 'application/json';
      }
    }

    const ms = options.timeout === undefined ? timeout : options.timeout;
    const retry = options.retry;
    const settings = typeof retry === 'object' && retry !== null ? retry : {};
    const maxRetries = method !== 'GET' || retry === false
      ? 0
      : Math.max(0, Number(settings.retries ?? (typeof retry === 'number' ? retry : retries)) || 0);
    const baseWait = settings.base ?? retryBase;
    const capWait = settings.max ?? retryMax;
    const retryCeiling = settings.maxWait ?? maxRetryWait;
    const fetchOptions = options.fetch ?? {};

    for (let attempt = 0; ; attempt++) {
      try {
        return await once(url, method, payload, headers, options.signal, ms, fetchOptions);
      } catch (error) {
        if (options.signal?.aborted) throw cancelled(options.signal);
        if (attempt >= maxRetries) throw error;
        const retryable = error instanceof Offline
          || error instanceof TimedOut
          || error instanceof RateLimited
          || (error instanceof Refused && error.status >= 500);
        if (!retryable) throw error;
        let wait = error instanceof RateLimited && error.retryAfter !== null
          ? error.retryAfter * 1000
          : Math.min(capWait, baseWait * 2 ** attempt) * (0.5 + random() * 0.5);
        wait = Math.round(wait);
        if (wait > retryCeiling) throw error;
        await sleep(wait);
      }
    }
  }

  return {
    request,
    get: (path, options = {}) => request(path, { ...options, method: 'GET' }),
    post: (path, body, options = {}) => request(path, { ...options, method: 'POST', body }),
    put: (path, body, options = {}) => request(path, { ...options, method: 'PUT', body }),
    patch: (path, body, options = {}) => request(path, { ...options, method: 'PATCH', body }),
    del: (path, options = {}) => request(path, { ...options, method: 'DELETE' }),
  };
}
