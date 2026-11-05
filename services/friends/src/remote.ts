import { body, fail, json, text } from './http.js';
import type { Storage, User } from './storage.js';

// A phone uses Encore on the listener's computer through here: it leaves commands,
// the computer picks them up, and answers or shows the result in its state. Every
// key belongs to one account and expires by itself, so nothing here is a record of
// what anyone did. Answers can name private links, so they're never logged.

const INBOX_SECONDS = 60;
const STATE_SECONDS = 30;
const REPLY_SECONDS = 60;
/** How long a phone counts as watching, which has the computer check every second. */
export const WANTED_SECONDS = 30;
const MAX_INBOX = 50;
const MAX_COMMAND_BYTES = 8 * 1024;
const MAX_STATE_BYTES = 16 * 1024;
const MAX_REPLY_BYTES = 900 * 1024;
const ID = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;

const inboxKey = (user: string) => `remote:${user}:inbox`;
const wantedKey = (user: string) => `remote:${user}:wanted`;
const stateKey = (user: string) => `remote:${user}:state`;
const replyKey = (user: string, id: string) => `remote:${user}:reply:${id}`;

interface Inbox { revision: number; items: unknown[] }

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

  return async function route(request: Request, path: string, user: User, signal: AbortSignal): Promise<Response | null> {
    const me = user.user_id;
    const method = request.method;

    // The computer asks what's waiting, and whether a phone is there to hurry for.
    if (path === '/live/remote/poll' && method === 'GET') {
      const [stored, wanted] = await storage.liveRead([inboxKey(me), wantedKey(me)], signal);
      const held = stored ? (JSON.parse(stored) as Inbox).items : [];
      let commands: unknown[] = [];
      if (held.length) {
        await write(me, inbox => { commands = inbox.items; inbox.items = []; }, signal);
      }
      return json({ wanted: wanted !== null, commands });
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
      return json({ ok: true }, 202);
    }

    if (path === '/live/remote/state' && method === 'PUT') {
      const input = await body(request, signal, MAX_STATE_BYTES);
      await storage.livePut(stateKey(me), JSON.stringify({ ...input, at: clock() }), STATE_SECONDS, signal);
      return json({ ok: true });
    }

    if (path === '/live/remote/state' && method === 'GET') {
      const [state, wanted] = await storage.liveRead([stateKey(me), wantedKey(me)], signal);
      // Written less often than read: it only has to outlast the next look.
      if (wanted === null || clock() - Number(wanted) > WANTED_SECONDS * 500) await watching(me, signal);
      return json({ state: state ? JSON.parse(state) as unknown : null, now: clock() });
    }

    const reply = /^\/live\/remote\/reply\/([a-f0-9-]{36})$/.exec(path);
    if (reply && ID.test(reply[1])) {
      if (method === 'PUT') {
        const input = await body(request, signal, MAX_REPLY_BYTES);
        await storage.livePut(replyKey(me, reply[1]), JSON.stringify(input), REPLY_SECONDS, signal);
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
