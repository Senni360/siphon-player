import type { IncomingMessage, ServerResponse } from 'node:http';
import { Readable } from 'node:stream';
import { configuredStorage } from './storage.js';
import { createService } from './service.js';
import { createJev } from './jev.js';
import { configuredAudioStore } from './audio.js';
import { SITE_HEADERS, downloadUrl, sitePage } from './site.js';
import { WEB_HOST, webAnswer } from './web.js';

export function createHandler(env: NodeJS.ProcessEnv) {
  let service: ReturnType<typeof createService> | undefined;
  let gateLimits: ReturnType<typeof configuredStorage> | undefined;
  return async function handler(req: IncomingMessage, res: ServerResponse) {
    const disconnected = new AbortController();
    const abort = () => disconnected.abort();
    const closed = () => { if (!res.writableEnded) abort(); };
    req.once('aborted', abort);
    res.once('close', closed);
    try {
      if (req.headers.host?.split(':')[0] === (env.ENCORE_WEB_HOST ?? WEB_HOST)) {
        gateLimits ??= configuredStorage(env);
        const answer = await webAnswer(req.method ?? 'GET', req.url ?? '/', req.headers.cookie, async () => {
          const parsed = (req as IncomingMessage & { body?: unknown }).body;
          if (parsed !== undefined) return Buffer.isBuffer(parsed) ? JSON.parse(parsed.toString('utf8')) : typeof parsed === 'string' ? JSON.parse(parsed) : parsed;
          const parts: Buffer[] = [];
          let size = 0;
          for await (const part of req) { size += (part as Buffer).length; if (size > 1024) throw new Error('Too large'); parts.push(Buffer.from(part as Buffer)); }
          return JSON.parse(Buffer.concat(parts).toString('utf8'));
        }, env.ENCORE_WEB_CODE ?? '', gateLimits, env.VERCEL ? String(req.headers['x-vercel-forwarded-for'] ?? 'unknown') : req.socket.remoteAddress ?? 'local');
        if (answer) {
          res.writeHead(answer.status, answer.headers);
          res.end(req.method === 'HEAD' ? undefined : answer.body);
          return;
        }
      }
      if ((req.method === 'GET' || req.method === 'HEAD') && new URL(req.url ?? '/', 'http://localhost').pathname === '/download') {
        res.writeHead(302, { Location: await downloadUrl(), 'Cache-Control': 'no-store' });
        res.end();
        return;
      }
      const page = sitePage(req.method, req.url);
      if (page) {
        res.writeHead(page.status, SITE_HEADERS);
        res.end(req.method === 'HEAD' ? undefined : page.html);
        return;
      }
      if (req.aborted || res.destroyed) abort();
      service ??= createService(configuredStorage(env), env.SIPHON_FRIENDS_INVITE_CODE ?? '', createJev(env), undefined, undefined, env.SIPHON_FRIENDS_OWNER_SPOTIFY_ID ?? '', configuredAudioStore(env));
      const headers = new Headers();
      for (const [key, value] of Object.entries(req.headers)) if (value) headers.set(key, Array.isArray(value) ? value.join(',') : value);
      // Vercel may have parsed the body before invoking the Node handler.
      const parsed = (req as IncomingMessage & { body?: unknown }).body;
      const requestBody = parsed === undefined ? Readable.toWeb(req) as ReadableStream :
        Buffer.isBuffer(parsed) ? new Uint8Array(parsed) :
        typeof parsed === 'string' ? parsed : JSON.stringify(parsed);
      const request = new Request(`http://localhost${req.url}`, {
        method: req.method, headers, signal: disconnected.signal,
        ...(!['GET', 'HEAD'].includes(req.method ?? 'GET') ? { body: requestBody, duplex: 'half' } : {}),
      });
      const ip = env.VERCEL ? headers.get('x-vercel-forwarded-for') ?? 'unknown' : req.socket.remoteAddress ?? 'local';
      const result = await service(request, ip);
      res.writeHead(result.status, Object.fromEntries(result.headers));
      res.end(Buffer.from(await result.arrayBuffer()));
    } catch {
      res.writeHead(503, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' });
      res.end('{"error":"Service unavailable"}');
    } finally { req.off('aborted', abort); res.off('close', closed); }
  };
}

export default createHandler(process.env);
