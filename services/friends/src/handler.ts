import type { IncomingMessage, ServerResponse } from 'node:http';
import { Readable } from 'node:stream';
import { configuredStorage } from './storage.ts';
import { createService } from './service.ts';

export function createHandler(env: NodeJS.ProcessEnv) {
  let service: ReturnType<typeof createService> | undefined;
  return async function handler(req: IncomingMessage, res: ServerResponse) {
    try {
      service ??= createService(configuredStorage(env), env.SIPHON_FRIENDS_INVITE_CODE ?? '');
      const headers = new Headers();
      for (const [key, value] of Object.entries(req.headers)) if (value) headers.set(key, Array.isArray(value) ? value.join(',') : value);
      // Vercel may have parsed the body before invoking the Node handler.
      const parsed = (req as IncomingMessage & { body?: unknown }).body;
      const requestBody = parsed === undefined ? Readable.toWeb(req) as ReadableStream :
        Buffer.isBuffer(parsed) ? new Uint8Array(parsed) :
        typeof parsed === 'string' ? parsed : JSON.stringify(parsed);
      const request = new Request(`http://localhost${req.url}`, {
        method: req.method, headers,
        ...(!['GET', 'HEAD'].includes(req.method ?? 'GET') ? { body: requestBody, duplex: 'half' } : {}),
      });
      const ip = env.VERCEL ? headers.get('x-vercel-forwarded-for') ?? 'unknown' : req.socket.remoteAddress ?? 'local';
      const result = await service(request, ip);
      res.writeHead(result.status, Object.fromEntries(result.headers));
      res.end(Buffer.from(await result.arrayBuffer()));
    } catch {
      res.writeHead(503, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
      res.end('{"error":"Service unavailable"}');
    }
  };
}

export default createHandler(process.env);
