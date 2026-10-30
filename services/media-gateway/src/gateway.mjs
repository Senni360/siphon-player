import { Registry, configured, failure } from "./registry.mjs";

// Fetch-based DO entrypoint deliberately needs no SDK or npm dependencies.
export class MediaRegistry extends Registry {
  constructor(state, env) {
    super(state, env);
  }
}

function registry(env) {
  if (!configured(env) || !env.MEDIA_REGISTRY?.idFromName || !env.MEDIA_REGISTRY?.get) {
    throw new Error("unconfigured");
  }
  return env.MEDIA_REGISTRY.get(env.MEDIA_REGISTRY.idFromName("m1-pilot-v1"));
}

export default {
  async fetch(request, env) {
    try {
      return await registry(env).fetch(request);
    } catch {
      return failure(503, "Media service unavailable", request.method === "HEAD");
    }
  },
  async scheduled(_event, env) {
    // Alarms retry, but only a bounded number of times. Cron provides an
    // independent retry path for the single pilot registry after outages.
    const response = await registry(env).fetch(new Request(`${env.MEDIA_ORIGIN}/admin/cleanup`, {
      method: "POST",
      headers: { Authorization: `Bearer ${env.ADMIN_SECRET}` },
    }));
    if (!response.ok) throw new Error("Media cleanup unavailable");
  },
};
