import { body, fail, json, text } from './http.js';
import { MAX_MESSAGE_BYTES, type Storage, type User } from './storage.js';

// A phone uses Encore on the listener's computer through here: it leaves commands,
// the computer picks them up, and answers or shows the result in its state. Every
// key belongs to one account and expires by itself, so nothing here is a record of
// what anyone did. Answers can name private links, so they're never logged.
//
// While a phone is open, each side also listens on an event stream, so a command,
// a new state or a ready answer reaches the other side at once instead of at the
// next look. The looks carry on behind it, slower, so nothing is lost if a word is.

const INBOX_SECONDS = 60;
const STATE_SECONDS = 30;
const REPLY_SECONDS = 60;
/** How long a phone counts as watching, which has the computer stay close by. */
export const WANTED_SECONDS = 30;
const MAX_INBOX = 50;
// Enough for playing a whole list: the phone sends at most 500 songs at a time.
const MAX_COMMAND_BYTES = 64 * 1024;
const MAX_STATE_BYTES = 16 * 1024;
const MAX_REPLY_BYTES = 900 * 1024;
const ID = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;
/** How long the phone's own playback stays known without word from it. */
const PHONE_SECONDS = 120;
/**
 * An event stream closes after this long, under the function's time limit, and the
 * listener opens the next. Streams only run while a phone is open, so the service
 * isn't held open for everyone all day.
 */
export const STREAM_SECONDS = 240;
const STREAM_PING_MS = 15_000;
/** What the computer may ask of the phone's own player. */
const CONTROLS = new Set(['toggle', 'pause', 'play', 'next', 'previous', 'seek']);

const inboxKey = (user: string) => `remote:${user}:inbox`;
const wantedKey = (user: string) => `remote:${user}:wanted`;
const stateKey = (user: string) => `remote:${user}:state`;
const replyKey = (user: string, id: string) => `remote:${user}:reply:${id}`;
const phoneKey = (user: string) => `remote:${user}:phone`;
/** Where each side hears about the other at once, while it's listening. */
const toComputer = (user: string) => `remote:${user}:to-computer`;
const toPhone = (user: string) => `remote:${user}:to-phone`;

interface Inbox { revision: number; items: unknown[] }

const parsed = (stored: string | null) => stored ? JSON.parse(stored) as unknown : null;

export function createRemote(storage: Storage, clock: () => number = Date.now) {
  async function read(user: string, signal: AbortSignal): Promise<Inbox> {
    const [stored] = await storage.liveRead([inboxKey(user)], signal);
    return stored ? JSON.parse(stored) as Inbox : { revision: 0, items: [] };
  }

  async function write(user: string, change: (inbox: Inbox) => void, signal: AbortSignal): Promise<Inbox> {
    for (let attempt = 0; attempt < 8; attempt++) {
      const inbox = await read(user, signal);
      const previous = inbox.revision;
      change(inbox);
      inbox.revision = previous + 1;
      if (await storage.liveCas(inboxKey(user), previous, JSON.stringify(inbox), INBOX_SECONDS, signal)) return inbox;
    }
    return fail(503, 'Busy, try again');
  }

  const watching = (user: string, signal: AbortSignal) =>
    storage.livePut(wantedKey(user), String(clock()), WANTED_SECONDS, signal);

  /** A word to whoever is listening. The looks still find everything, so a lost one only costs time. */
  const tell = (channel: string, message: unknown, signal: AbortSignal) => {
    const encoded = JSON.stringify(message);
    if (Buffer.byteLength(encoded) > MAX_MESSAGE_BYTES) return Promise.resolve();
    return storage.livePublish(channel, encoded, signal).catch(() => {});
  };

  /**
   * Everything said to one side, as server-sent events, opening with where things
   * stand. A phone listening counts as watching, which keeps the computer close by.
   */
  function events(me: string, side: 'computer' | 'phone', signal: AbortSignal): Response {
    const stop = new AbortController();
    const ended = AbortSignal.any([signal, stop.signal, AbortSignal.timeout(STREAM_SECONDS * 1000)]);
    const encoder = new TextEncoder();
    const stream = new ReadableStream<Uint8Array>({
      async start(controller) {
        const send = (line: string) => {
          try { controller.enqueue(encoder.encode(line)); } catch { stop.abort(); }
        };
        const ping = setInterval(() => {
          send(': ping\n\n');
          if (side === 'phone') void watching(me, ended).catch(() => {});
        }, STREAM_PING_MS);
        try {
          // Listening starts before the opening state is read, so nothing falls between.
          const listening = storage.liveListen(side === 'phone' ? toPhone(me) : toComputer(me), ended)[Symbol.asyncIterator]();
          const first = listening.next();
          if (side === 'phone') {
            await watching(me, ended);
            const [state] = await storage.liveRead([stateKey(me)], ended);
            send(`data: ${JSON.stringify({ t: 'state', state: parsed(state), now: clock() })}\n\n`);
          } else {
            const [phone] = await storage.liveRead([phoneKey(me)], ended);
            send(`data: ${JSON.stringify({ t: 'phone', phone: parsed(phone), now: clock() })}\n\n`);
          }
          for (let next = await first; !next.done; next = await listening.next()) send(`data: ${next.value}\n\n`);
        } catch { /* the listener opens another */ } finally {
          clearInterval(ping);
          stop.abort();
          try { controller.close(); } catch { /* already gone */ }
        }
      },
      cancel() { stop.abort(); },
    });
    return new Response(stream, {
      headers: { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store', 'X-Accel-Buffering': 'no' },
    });
  }

  return async function route(request: Request, path: string, user: User, signal: AbortSignal): Promise<Response | null> {
    const me = user.user_id;
    const method = request.method;

    // The computer asks what's waiting, whether a phone is there to hurry for, and
    // what the phone itself is playing.
    if (path === '/live/remote/poll' && method === 'GET') {
      const [stored, wanted, phone] = await storage.liveRead([inboxKey(me), wantedKey(me), phoneKey(me)], signal);
      const held = stored ? (JSON.parse(stored) as Inbox).items : [];
      let commands: unknown[] = [];
      if (held.length) {
        await write(me, inbox => { commands = inbox.items; inbox.items = []; }, signal);
      }
      return json({ wanted: wanted !== null, commands, phone: parsed(phone), now: clock() });
    }

    if (path === '/live/remote/events' && method === 'GET') {
      const side = new URL(request.url).searchParams.get('for');
      if (side !== 'computer' && side !== 'phone') return fail(400, 'Invalid side');
      return events(me, side, signal);
    }

    if (path === '/live/remote/send' && method === 'POST') {
      const input = await body(request, signal, MAX_COMMAND_BYTES);
      if (typeof input.id !== 'string' || !ID.test(input.id)) return fail(400, 'Invalid id');
      const kind = text(input.kind, 'kind', 40);
      if (!/^[a-z_]+$/.test(kind)) return fail(400, 'Invalid kind');
      await write(me, inbox => {
        inbox.items.push(input);
        inbox.items = inbox.items.slice(-MAX_INBOX);
      }, signal);
      await watching(me, signal);
      await tell(toComputer(me), { t: 'command' }, signal);
      return json({ ok: true }, 202);
    }

    if (path === '/live/remote/state' && method === 'PUT') {
      const input = await body(request, signal, MAX_STATE_BYTES);
      const state = { ...input, at: clock() };
      await storage.livePut(stateKey(me), JSON.stringify(state), STATE_SECONDS, signal);
      await tell(toPhone(me), { t: 'state', state, now: clock() }, signal);
      return json({ ok: true });
    }

    if (path === '/live/remote/state' && method === 'GET') {
      const [state, wanted] = await storage.liveRead([stateKey(me), wantedKey(me)], signal);
      // Written less often than read: it only has to outlast the next look.
      if (wanted === null || clock() - Number(wanted) > WANTED_SECONDS * 500) await watching(me, signal);
      return json({ state: parsed(state), now: clock() });
    }

    // The phone's own player, so the computer can show what plays there.
    if (path === '/live/remote/phone' && method === 'PUT') {
      const input = await body(request, signal, MAX_STATE_BYTES);
      const phone = { ...input, at: clock() };
      await storage.livePut(phoneKey(me), JSON.stringify(phone), PHONE_SECONDS, signal);
      await tell(toComputer(me), { t: 'phone', phone, now: clock() }, signal);
      return json({ ok: true });
    }
    if (path === '/live/remote/phone' && method === 'DELETE') {
      await storage.liveDelete(phoneKey(me), signal);
      await tell(toComputer(me), { t: 'phone', phone: null, now: clock() }, signal);
      return json({ ok: true });
    }

    // The computer working the phone's own player. Only a phone listening hears it.
    if (path === '/live/remote/control' && method === 'POST') {
      const input = await body(request, signal, 1024);
      const kind = text(input.kind, 'kind', 20);
      if (!CONTROLS.has(kind)) return fail(400, 'Invalid kind');
      const ms = input.ms === undefined ? undefined : Number(input.ms);
      if (ms !== undefined && (!Number.isFinite(ms) || ms < 0)) return fail(400, 'Invalid ms');
      await tell(toPhone(me), { t: 'control', kind, ...(ms === undefined ? {} : { ms: Math.round(ms) }) }, signal);
      return json({ ok: true }, 202);
    }

    const reply = /^\/live\/remote\/reply\/([a-f0-9-]{36})$/.exec(path);
    if (reply && ID.test(reply[1])) {
      if (method === 'PUT') {
        const input = await body(request, signal, MAX_REPLY_BYTES);
        await storage.livePut(replyKey(me, reply[1]), JSON.stringify(input), REPLY_SECONDS, signal);
        await tell(toPhone(me), { t: 'reply', id: reply[1] }, signal);
        return json({ ok: true });
      }
      if (method === 'GET') {
        const [stored] = await storage.liveRead([replyKey(me, reply[1])], signal);
        return json(stored ? { ready: true, reply: JSON.parse(stored) as unknown } : { ready: false });
      }
    }
    return null;
  };
}
