// Response-like objects and a recording fetch for tests. Nothing here touches
// the network.

export function reply(status, body, headers = {}) {
  const values = new Map(Object.entries(headers).map(([key, value]) => [key.toLowerCase(), String(value)]));
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: {
      get(name) {
        return values.get(String(name).toLowerCase()) ?? null;
      },
    },
    async json() {
      if (typeof body === "string") return JSON.parse(body);
      return body;
    },
    async text() {
      return typeof body === "string" ? body : JSON.stringify(body);
    },
  };
}

export function fakeFetch(handler) {
  const calls = [];
  const impl = async (input, init = {}) => {
    const url = typeof input === "string" ? input : input?.url ?? String(input);
    const call = {
      url,
      path: new URL(url).pathname,
      method: init.method ?? "GET",
      headers: init.headers ?? {},
      body: init.body,
    };
    calls.push(call);
    return handler(call, calls);
  };
  impl.calls = calls;
  impl.count = (predicate) => calls.filter(predicate).length;
  return impl;
}

export function header(call, name) {
  for (const [key, value] of Object.entries(call.headers ?? {})) {
    if (key.toLowerCase() === name.toLowerCase()) return value;
  }
  return undefined;
}

export const TOKEN_URL = "https://accounts.spotify.com/api/token";

export function isSpotifyToken(call) {
  return call.url.startsWith(TOKEN_URL);
}

export function isSpotifyApi(call, path) {
  return call.path === path && call.url.startsWith("https://api.spotify.com/v1/");
}
