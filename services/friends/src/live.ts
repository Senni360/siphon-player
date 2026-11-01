import { randomInt, randomUUID } from 'node:crypto';
import { setTimeout as delay } from 'node:timers/promises';
import { body, fail, https, json, text, validateUrl } from './http.js';
import type { Rights, Storage, User } from './storage.js';

// Live state is small, short-lived and written often, so it stays out of the
// group's document: every key expires by itself, and a read is one MGET.
export const PRESENCE_SECONDS = 200;
const CHANNELS_SECONDS = 2 * 86400;
const CHANNELS_KEY = 'channels';
export const MAX_CHANNELS = 10;
export const MAX_QUEUE = 200;
export const MAX_BACKGROUND = 1000;
const FIRST_GROUP = '00000000-0000-4000-8000-000000000001';
const UUID = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;

export interface Song { url: string; source: 'spotify' | 'soundcloud' | 'youtube'; title: string; artists: string[]; artwork_url: string | null; duration_ms: number; uri?: string }
export interface Presence { user_id: string; item: Song | null; position_ms: number; playing: boolean; at: number; following: string | null }
export interface Entry { id: string; item: Song; added_by: string }
/** The songs a party falls back on when its queue runs out. The songs themselves have their own key. */
export interface Background { name: string; by: string; total: number; next: number; upcoming: Song[] }
export interface Channel {
  id: string; name: string; host_id: string; created_at: number; members: string[]; current: Entry | null; position_ms: number; playing: boolean; at: number; queue: Entry[];
  /** A party: only the host and those they name move the music. Absent on channels from older clients. */
  locked?: boolean;
  /** Guests the host trusted with the controls too. */
  controllers?: string[];
  /** Who asked the host to skip the song that is playing. */
  skip_requests?: string[];
  background?: Background | null;
}
interface Channels { revision: number; channels: Channel[] }

const presenceKey = (id: string) => `presence:${id}`;
const STATS_PERIODS = ['month', 'all'];
const statsKey = (id: string, period: string) => `stats:${id}:${period}`;
const STATS_SECONDS: Record<string, number> = { month: 40 * 86400, all: 400 * 86400 };
function parsed<T>(value: string | null): T | null {
  if (value === null) return null;
  try { return JSON.parse(value) as T; } catch { return null; }
}
const backgroundKey = (id: string) => `background:${id}`;
const BACKGROUND_PREVIEW = 3;
function position(value: unknown): number {
  if (!Number.isSafeInteger(value) || Number(value) < 0 || Number(value) > 604800000) fail(400, 'Invalid position_ms');
  return Number(value);
}
/** One song: a share may be an album, but what plays now is always one song. */
export function song(value: unknown): Song {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return fail(400, 'Invalid item');
  const raw = value as Record<string, unknown>;
  const source = raw.source as Song['source'];
  const url = validateUrl(raw.url, source);
  const path = new URL(url).pathname.split('/').filter(Boolean);
  if (source === 'spotify' && !path.includes('track') || source === 'soundcloud' && path[1] === 'sets') fail(400, 'Only one song can play');
  if (!Array.isArray(raw.artists) || raw.artists.length > 20) fail(400, 'Invalid artists');
  const artists = (raw.artists as unknown[]).map(a => text(a, 'artist', 200));
  const artwork = raw.artwork_url == null || raw.artwork_url === '' ? null : https(raw.artwork_url).toString();
  const uri = raw.uri === undefined || raw.uri === null ? undefined : text(raw.uri, 'uri', 200);
  if (uri !== undefined && !/^(?:spotify:track|soundcloud:track|youtube:video):[\w.-]+$/.test(uri)) fail(400, 'Invalid uri');
  return { url, source, title: text(raw.title, 'title', 300), artists, artwork_url: artwork, duration_ms: position(raw.duration_ms), ...(uri ? { uri } : {}) };
}
function count(value: unknown, name: string): number {
  if (!Number.isSafeInteger(value) || Number(value) < 0) fail(400, `Invalid ${name}`);
  return Number(value);
}
function tops(value: unknown, max: number): unknown[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > max) return fail(400, 'Invalid list');
  return value.map(row => {
    const raw = (row ?? {}) as Record<string, unknown>;
    const art = raw.art == null || raw.art === '' ? null : https(raw.art).toString();
    return { name: text(raw.name, 'name', 300), detail: text(raw.detail, 'detail', 400, true), art, uri: text(raw.uri, 'uri', 300, true), plays: count(raw.plays, 'plays'), minutes: count(raw.minutes, 'minutes') };
  });
}
/** What a member shares of their listening: totals and the top of each list. */
function summary(value: unknown) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return fail(400, 'Invalid summary');
  const raw = value as Record<string, unknown>;
  const hours = raw.hours === undefined ? [] : raw.hours;
  if (!Array.isArray(hours) || hours.length > 24) return fail(400, 'Invalid hours');
  return {
    month: text(raw.month, 'month', 7, true), minutes: count(raw.minutes, 'minutes'), plays: count(raw.plays, 'plays'),
    songs: tops(raw.songs, 20), artists: tops(raw.artists, 20), genres: tops(raw.genres, 10), hours: hours.map(h => count(h, 'hours')),
  };
}
/** Who a listener's song comes from: a member's id, or nobody. */
function followed(value: unknown, reader: string): string | null {
  if (value === undefined || value === null) return null;
  if (typeof value !== 'string' || !UUID.test(value) || value === reader) return fail(400, 'Invalid following');
  return value;
}

export function createLive(storage: Storage, clock: () => number = Date.now) {
  const channelsKey = (group: string) => group === FIRST_GROUP ? CHANNELS_KEY : `${CHANNELS_KEY}:${group}`;
  /** A party whose host left is handed to whoever has been there longest. */
  const keepHost = (channel: Channel) => {
    if (channel.locked && !channel.members.includes(channel.host_id) && channel.members.length) channel.host_id = channel.members[0];
    if (channel.controllers) channel.controllers = channel.controllers.filter(id => channel.members.includes(id));
    if (channel.skip_requests) channel.skip_requests = channel.skip_requests.filter(id => channel.members.includes(id));
  };
  /** Everyone's presence and the channels, with channels narrowed to the members still here. */
  async function read(users: User[], signal: AbortSignal, key: string = CHANNELS_KEY) {
    const [stored, ...found] = await storage.liveRead([key, ...users.map(u => presenceKey(u.user_id))], signal);
    const present = found.map(value => parsed<Presence>(value)).filter((p): p is Presence => !!p);
    const here = new Set(present.map(p => p.user_id));
    const document = parsed<Channels>(stored) ?? { revision: 0, channels: [] };
    for (const channel of document.channels) {
      channel.members = channel.members.filter(id => here.has(id));
      keepHost(channel);
    }
    document.channels = document.channels.filter(c => c.members.length);
    return { document, present };
  }

  async function change<T>(users: User[], update: (document: Channels) => T, signal: AbortSignal, key: string = CHANNELS_KEY): Promise<T> {
    for (let i = 0; i < 10; i++) {
      signal.throwIfAborted();
      const { document } = await read(users, signal, key);
      const previous = document.revision;
      const result = update(document);
      for (const channel of document.channels) keepHost(channel);
      document.channels = document.channels.filter(c => c.members.length);
      document.revision++;
      if (await storage.liveCas(key, previous, JSON.stringify(document), CHANNELS_SECONDS, signal)) return result;
      await delay(randomInt(5, 80), undefined, { signal });
    }
    return fail(503, 'Channel busy, try again');
  }

  /** Someone in a channel counts as here from the moment they arrive. */
  async function arrive(user: User, signal: AbortSignal) {
    const [existing] = await storage.liveRead([presenceKey(user.user_id)], signal);
    if (existing !== null) return;
    const blank: Presence = { user_id: user.user_id, item: null, position_ms: 0, playing: false, at: clock(), following: null };
    await storage.livePut(presenceKey(user.user_id), JSON.stringify(blank), PRESENCE_SECONDS, signal);
  }

  const found = (document: Channels, id: string, reader: string): Channel => {
    const channel = document.channels.find(c => c.id === id);
    if (!channel) return fail(404, 'Channel not found');
    if (!channel.members.includes(reader)) return fail(403, 'Join the channel first');
    return channel;
  };
  const play = (channel: Channel, entry: Entry | null) => {
    channel.current = entry;
    channel.position_ms = 0;
    channel.playing = entry !== null;
    channel.at = clock();
  };
  const answer = (channel: Channel | null, status = 200) => json({ now: clock(), channel }, status);

  async function route(request: Request, path: string, user: User, users: User[], signal: AbortSignal, group: string = FIRST_GROUP, rights: Rights = { share: true, host: true, queue: true, invite: true }): Promise<Response | null> {
    const method = request.method;
    const me = user.user_id;
    const key = channelsKey(group);
    const changing = <T>(update: (document: Channels) => T) => change(users, update, signal, key);
    const controls = (channel: Channel) => !channel.locked || channel.host_id === me || !!channel.controllers?.includes(me);
    const mustControl = (channel: Channel) => { if (!controls(channel)) fail(403, 'Only the host controls the music'); };
    const mayQueue = (channel: Channel) => { if (channel.locked && !rights.queue && !controls(channel)) fail(403, 'An admin turned off adding songs for you in this group'); };
    if (path === '/live' && method === 'GET') {
      const { document, present } = await read(users, signal, key);
      return json({ now: clock(), presence: present, channels: document.channels });
    }
    if (path === '/live/presence' && method === 'PUT') {
      const input = await body(request, signal);
      const presence: Presence = {
        user_id: me,
        item: input.item === null || input.item === undefined ? null : song(input.item),
        position_ms: position(input.position_ms ?? 0),
        playing: input.playing === true,
        at: clock(),
        following: followed(input.following, me),
      };
      await storage.livePut(presenceKey(me), JSON.stringify(presence), PRESENCE_SECONDS, signal);
      return json({ now: presence.at });
    }
    if (path === '/live/presence' && method === 'DELETE') {
      await storage.liveDelete(presenceKey(me), signal);
      return json({ now: clock() });
    }
    const stats = /^\/live\/stats\/(month|all)$/.exec(path);
    if (stats && method === 'PUT') {
      const shared = summary((await body(request, signal, 64 * 1024)).summary);
      await storage.livePut(statsKey(me, stats[1]), JSON.stringify(shared), STATS_SECONDS[stats[1]], signal);
      return json({ now: clock() });
    }
    if (stats && method === 'GET') {
      const found = await storage.liveRead(users.map(u => statsKey(u.user_id, stats[1])), signal);
      return json({
        now: clock(),
        stats: users.flatMap((u, i) => {
          const value = parsed<ReturnType<typeof summary>>(found[i]);
          return value ? [{ user_id: u.user_id, summary: value }] : [];
        }),
      });
    }
    if (path === '/live/stats' && method === 'DELETE') {
      for (const period of STATS_PERIODS) await storage.liveDelete(statsKey(me, period), signal);
      return json({ now: clock() });
    }
    if (path === '/live/channels' && method === 'POST') {
      const input = await body(request, signal);
      const name = text(input.name, 'name', 60);
      if (!rights.host) return fail(403, 'An admin turned off hosting for you in this group');
      const item = input.item === null || input.item === undefined ? null : song(input.item);
      const start = position(input.position_ms ?? 0);
      const playing = input.playing !== false;
      await arrive(user, signal);
      const channel = await changing(document => {
        for (const other of document.channels) other.members = other.members.filter(id => id !== me);
        document.channels = document.channels.filter(c => c.members.length);
        if (document.channels.length >= MAX_CHANNELS) return fail(409, 'Too many channels');
        const created: Channel = {
          id: randomUUID(), name, host_id: me, created_at: clock(), members: [me], current: item && { id: randomUUID(), item, added_by: me },
          position_ms: item ? start : 0, playing: !!item && playing, at: clock(), queue: [],
          ...(input.locked === true ? { locked: true, controllers: [], skip_requests: [], background: null } : {}),
        };
        document.channels.push(created);
        return created;
      });
      return answer(channel, 201);
    }
    const match = /^\/live\/channels\/([a-f0-9-]{36})\/(join|leave|playback|next|queue|end|host|controllers|skip-request|background)(?:\/([a-f0-9-]{36})(\/move)?)?$/.exec(path);
    if (!match) return null;
    const [, id, verb, entryId, move] = match;
    if (verb === 'join' && method === 'POST' && !entryId) {
      await arrive(user, signal);
      return answer(await changing(document => {
        const channel = document.channels.find(c => c.id === id);
        if (!channel) return fail(404, 'Channel not found');
        for (const other of document.channels) if (other !== channel) other.members = other.members.filter(m => m !== me);
        document.channels = document.channels.filter(c => c.members.length);
        if (!channel.members.includes(me)) channel.members.push(me);
        return channel;
      }));
    }
    if (verb === 'leave' && method === 'POST' && !entryId) {
      await changing(document => {
        for (const channel of document.channels) channel.members = channel.members.filter(m => m !== me);
        document.channels = document.channels.filter(c => c.members.length);
      });
      return answer(null);
    }
    if (verb === 'playback' && method === 'POST' && !entryId) {
      const input = await body(request, signal);
      if (typeof input.playing !== 'boolean') return fail(400, 'Invalid playing');
      const at = position(input.position_ms);
      return answer(await changing(document => {
        const channel = found(document, id, me);
        mustControl(channel);
        channel.playing = input.playing as boolean && channel.current !== null;
        channel.position_ms = at;
        channel.at = clock();
        return channel;
      }));
    }
    if (verb === 'next' && method === 'POST' && !entryId) {
      const input = await body(request, signal);
      // Every listener may report the same song's end; only the first moves on.
      const from = input.from === undefined || input.from === null ? null : text(input.from, 'from', 36);
      const to = input.to === undefined || input.to === null ? null : text(input.to, 'to', 36);
      // Read outside the change: a retry must not read it again, and a list a moment stale is harmless.
      const [stored] = await storage.liveRead([backgroundKey(id)], signal);
      const songs = parsed<Song[]>(stored) ?? [];
      return answer(await changing(document => {
        const channel = found(document, id, me);
        if (from !== null && channel.current?.id !== from) return channel;
        // Guests may only say that the song has really run out, so that no
        // host has to be the one who keeps the music going.
        if (!controls(channel)) {
          const current = channel.current;
          const ended = from !== null && !!current && current.item.duration_ms > 0 && channel.playing && channel.position_ms + clock() - channel.at >= current.item.duration_ms;
          if (!ended) return fail(403, 'Only the host controls the music');
        }
        channel.skip_requests = channel.locked ? [] : channel.skip_requests;
        if (to !== null) {
          const index = channel.queue.findIndex(e => e.id === to);
          if (index < 0) return channel;
          play(channel, channel.queue[index]);
          channel.queue.splice(0, index + 1);
        } else {
          let next = channel.queue.shift() ?? null;
          const background = channel.background;
          if (!next && background && songs.length) {
            next = { id: randomUUID(), item: songs[background.next % songs.length], added_by: background.by };
            background.next = (background.next + 1) % songs.length;
            background.upcoming = Array.from({ length: Math.min(BACKGROUND_PREVIEW, songs.length) }, (_, i) => songs[(background.next + i) % songs.length]);
          }
          play(channel, next);
        }
        return channel;
      }));
    }
    if (verb === 'queue' && method === 'POST' && !entryId) {
      const input = await body(request, signal);
      const item = song(input.item);
      const where = input.where ?? 'end';
      if (where !== 'end' && where !== 'next' && where !== 'now') return fail(400, 'Invalid where');
      return answer(await changing(document => {
        const channel = found(document, id, me);
        mayQueue(channel);
        // In a party a guest's song waits its turn: it never cuts the current one off.
        if (where === 'now' && !controls(channel)) return fail(403, 'Only the host controls the music');
        if (channel.queue.length >= MAX_QUEUE) return fail(409, 'The queue is full');
        const entry: Entry = { id: randomUUID(), item, added_by: me };
        if (where === 'now' || !channel.current) play(channel, entry);
        else if (where === 'next') channel.queue.unshift(entry);
        else channel.queue.push(entry);
        return channel;
      }), 201);
    }
    if (verb === 'queue' && entryId && !move && method === 'DELETE') {
      return answer(await changing(document => {
        const channel = found(document, id, me);
        mayQueue(channel);
        channel.queue = channel.queue.filter(e => e.id !== entryId);
        return channel;
      }));
    }
    if (verb === 'queue' && entryId && move && method === 'POST') {
      const input = await body(request, signal);
      if (!Number.isSafeInteger(input.index) || Number(input.index) < 0) return fail(400, 'Invalid index');
      return answer(await changing(document => {
        const channel = found(document, id, me);
        mayQueue(channel);
        const from = channel.queue.findIndex(e => e.id === entryId);
        if (from >= 0) {
          const [entry] = channel.queue.splice(from, 1);
          channel.queue.splice(Math.min(Number(input.index), channel.queue.length), 0, entry);
        }
        return channel;
      }));
    }
    if (verb === 'end' && method === 'POST' && !entryId) {
      await changing(document => {
        const channel = found(document, id, me);
        if (channel.host_id !== me) return fail(403, 'Only the host can end the party');
        document.channels = document.channels.filter(c => c !== channel);
      });
      await storage.liveDelete(backgroundKey(id), signal);
      return answer(null);
    }
    if (verb === 'host' && method === 'POST' && !entryId) {
      const next = text((await body(request, signal)).user_id, 'user_id', 36);
      return answer(await changing(document => {
        const channel = found(document, id, me);
        if (channel.host_id !== me) return fail(403, 'Only the host can hand the party over');
        if (!channel.members.includes(next)) return fail(404, 'They are not in the party');
        channel.host_id = next;
        channel.controllers = (channel.controllers ?? []).filter(other => other !== next);
        return channel;
      }));
    }
    if (verb === 'controllers' && method === 'PUT' && !entryId) {
      const input = await body(request, signal);
      if (!Array.isArray(input.user_ids) || input.user_ids.length > 50 || !input.user_ids.every(other => typeof other === 'string' && UUID.test(other))) return fail(400, 'Invalid user_ids');
      return answer(await changing(document => {
        const channel = found(document, id, me);
        if (channel.host_id !== me) return fail(403, 'Only the host can share the controls');
        channel.controllers = (input.user_ids as string[]).filter(other => channel.members.includes(other) && other !== channel.host_id);
        return channel;
      }));
    }
    if (verb === 'skip-request' && !entryId && (method === 'PUT' || method === 'DELETE')) {
      return answer(await changing(document => {
        const channel = found(document, id, me);
        const others = (channel.skip_requests ?? []).filter(other => other !== me);
        channel.skip_requests = method === 'PUT' ? [...others, me] : others;
        return channel;
      }));
    }
    if (verb === 'background' && !entryId && method === 'PUT') {
      const input = await body(request, signal, 768 * 1024);
      const name = text(input.name, 'name', 100);
      if (!Array.isArray(input.items) || !input.items.length || input.items.length > MAX_BACKGROUND) return fail(400, 'Invalid items');
      const items = input.items.map(song);
      const answered = await changing(document => {
        const channel = found(document, id, me);
        mayQueue(channel);
        // Anyone may start one; only someone at the controls may replace another's.
        if (channel.background && channel.background.by !== me && !controls(channel)) return fail(403, 'Only the host can replace the background playlist');
        channel.background = { name, by: me, total: items.length, next: 0, upcoming: items.slice(0, BACKGROUND_PREVIEW) };
        return channel;
      });
      await storage.livePut(backgroundKey(id), JSON.stringify(items), CHANNELS_SECONDS, signal);
      return answer(answered);
    }
    if (verb === 'background' && !entryId && method === 'DELETE') {
      const answered = await changing(document => {
        const channel = found(document, id, me);
        if (channel.background && channel.background.by !== me && !controls(channel)) return fail(403, 'Only the host can remove the background playlist');
        channel.background = null;
        return channel;
      });
      await storage.liveDelete(backgroundKey(id), signal);
      return answer(answered);
    }
    return null;
  }

  /** Leaving the group leaves nothing live behind. */
  async function forget(user: User, users: User[], signal: AbortSignal) {
    await change(users, document => {
      for (const channel of document.channels) channel.members = channel.members.filter(m => m !== user.user_id);
      document.channels = document.channels.filter(c => c.members.length);
    }, signal);
    await storage.liveDelete(presenceKey(user.user_id), signal);
    for (const period of STATS_PERIODS) await storage.liveDelete(statsKey(user.user_id, period), signal);
  }

  return { route, forget };
}
