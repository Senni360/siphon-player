// A local stand-in for the group service, for the phone browser tests.
//
// It serves `assets/web` with the same headers as `src/web.ts` and answers the
// JSON the app asks for. Nothing here reaches the network: the "computer" that
// normally answers `/api/live/remote/*` imitates a paused Encore, so the app
// boots straight into a signed-in, connected state.
//
// The tests drive it in-process (the Playwright fixtures own one per test):
//   server.failNext('search', { status: 500, error: 'Search is down.' });
//   server.slowNext('home', 3_000);
//   server.setComputerOffline(true);
//   server.dropEventStream(1); server.endEventStreams();
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const WEB = join(here, '..', '..', 'assets', 'web');

// ---------------------------------------------------------------- the app's songs
export interface Song {
  uri: string;
  name: string;
  artists: string[];
  image: string | null;
  duration_ms: number;
  artist_id?: string;
  plays?: number;
  link?: string;
}

export interface PlayerState {
  song: Song | null;
  position_ms: number;
  playing: boolean;
  liked: boolean;
  shuffle: boolean;
  repeat: 'off' | 'all' | 'one';
  volume: number;
  device: string;
  next: Song[];
  at: number;
}

const TITLES = [
  'Midnight City', 'Neon Harbour', 'Paper Boats', 'Slow Static', 'Golden Hour',
  'Satellite Hearts', 'After the Rain', 'Velvet Morning', 'Coastline', 'Half Light',
  'Little Fires', 'Blue Hour', 'Static Bloom', 'Open Water', 'Evening Sun',
  'Hollow Pines', 'Northern Line', 'Quiet Machines', 'Glasshouse', 'New Weather',
  'Sleep Talk', 'Low Tide', 'Soft Focus', 'Night Bus', 'First Light',
];

const ARTISTS = [
  'M83', 'Neon Harbour', 'The Paper Kites', 'Bonobo', 'Beach House',
  'Tycho', 'Little Dragon', 'Four Tet',
];

const imageFor = (index: number, source: string): string => {
  if (source === 'spotify') return `https://i.scdn.co/image/ab67616d0000b273${String(index).padStart(4, '0')}`;
  if (source === 'soundcloud') return `https://i1.sndcdn.com/artworks-${index}-t500x500.jpg`;
  return `https://i.ytimg.com/vi/seed${String(index).padStart(4, '0')}/hqdefault.jpg`;
};

/** 200 songs, mixed across the three sources, each (title, artist) pair unique. */
export const SONGS: Song[] = Array.from({ length: 200 }, (_, index) => {
  const source = index % 3 === 0 ? 'spotify' : index % 3 === 1 ? 'soundcloud' : 'youtube';
  const uri = source === 'spotify'
    ? `spotify:track:2${String(index).padStart(21, '0')}`
    : source === 'soundcloud'
      ? `soundcloud:tracks:${index + 1}`
      : `youtube:video:seed${index + 1}`;
  return {
    uri,
    name: TITLES[index % TITLES.length]!,
    artists: [ARTISTS[index % ARTISTS.length]!],
    image: imageFor(index, source),
    duration_ms: 150_000 + (index % 90) * 1_000,
    ...(source === 'spotify' ? { artist_id: `artist-${index % ARTISTS.length}` } : {}),
    plays: (index * 37) % 5_000,
    link: source === 'soundcloud'
      ? `https://soundcloud.com/encore/song-${index}`
      : source === 'youtube'
        ? `https://www.youtube.com/watch?v=seed${index}`
        : `https://open.spotify.com/track/seed${index}`,
  };
});

const spotifySongs = SONGS.filter(song => song.uri.startsWith('spotify:'));
const soundcloudSongs = SONGS.filter(song => song.uri.startsWith('soundcloud:'));
const youtubeSongs = SONGS.filter(song => song.uri.startsWith('youtube:'));

const MIXED = { id: 'mix-everything', name: 'Everything, 200 Songs', image: SONGS[0]!.image, count: 200 };
const LIKED = SONGS.filter((_, index) => index % 8 === 1).slice(0, 24);
const PLAYLISTS = [
  { id: 'pl-deep-focus', name: 'Deep Focus', image: SONGS[12]!.image },
  { id: 'pl-night-drive', name: 'Night Drive', image: SONGS[15]!.image },
  { id: 'pl-sunday', name: 'Sunday Kitchen', image: SONGS[18]!.image },
];

// ---------------------------------------------------------------- the page's headers
// Copied from src/web.ts (HEADERS). Keep the two in step: the page is only meant
// to load itself, Spotify's own sign-in and API, and images from wherever art is.
const HEADERS: Record<string, string> = {
  'Content-Security-Policy': "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' https: data:; media-src https:; font-src 'self'; connect-src 'self' https://accounts.spotify.com https://api.spotify.com; manifest-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
  'X-Content-Type-Options': 'nosniff',
  'Referrer-Policy': 'no-referrer',
};

const CONTENT: Record<string, string> = {
  '/app.js': 'text/javascript; charset=utf-8',
  '/app.css': 'text/css; charset=utf-8',
  '/manifest.webmanifest': 'application/manifest+json',
  '/icon.svg': 'image/svg+xml',
  '/icon-180.png': 'image/png',
  '/icon-192.png': 'image/png',
  '/icon-512.png': 'image/png',
};

export interface Failure {
  status: number;
  error: string;
}

const uuid = (number: number): string => `4d544a6f-8d43-4f1a-9e2c-${String(number).padStart(12, '0')}`;

/** An account in one group, as `/api/account` reports it. */
const account = {
  user_id: 'user-1',
  display_name: 'Sam',
  spotify_id: null,
  soundcloud: null,
  devices: [{ id: 'device-1', name: 'Phone', created_at: 1_700_000_000_000, current: true }],
  groups: [{
    id: 'group-1',
    name: 'The Group',
    owner_id: 'user-1',
    role: 'owner',
    rights: { share: true, queue: true },
    members: [
      { user_id: 'user-1', display_name: 'Sam', role: 'owner', rights: { share: true, queue: true }, spotify_linked: false, soundcloud: null },
      { user_id: 'user-2', display_name: 'Robin', role: 'member', rights: { share: true, queue: true }, spotify_linked: true, soundcloud: 'robin' },
    ],
    invite: null,
    pending: [],
  }],
};

interface PostRow {
  id: string;
  author_id: string;
  created_at: number;
  url: string;
  source: string;
  title: string;
  artists: string[];
  artwork_url: string | null;
  duration_ms: number;
  note: string;
  author_name: string;
  like_count: number;
  liked: boolean;
  to: string[];
  affinity: null;
  replies: Array<{ id: string; author_id: string; author_name: string; created_at: number; text: string }>;
}

const post = (id: string, author: string, name: string, title: string, source: string): PostRow => ({
  id,
  author_id: author,
  created_at: 1_760_000_000_000,
  url: `https://soundcloud.com/encore/${id}`,
  source,
  title,
  artists: ['Neon Harbour'],
  artwork_url: imageFor(1, 'soundcloud'),
  duration_ms: 214_000,
  note: '',
  author_name: name,
  like_count: 1,
  liked: false,
  to: [],
  affinity: null,
  replies: [],
});

/** One local instance of the service, with the computer behind it. */
export class PhoneServer {
  private http: Server | null = null;
  private keepAlive: NodeJS.Timeout | null = null;
  private closing = false;

  private readonly replies = new Map<string, unknown>();
  private readonly events = new Set<ServerResponse>();
  private readonly failures = new Map<string, Failure[]>();
  private readonly delays = new Map<string, number[]>();
  private pendingDrops = 0;

  readonly posts: PostRow[] = [
    post(uuid(1), 'user-2', 'Robin', 'Midnight City', 'soundcloud'),
    post(uuid(2), 'user-2', 'Robin', 'Golden Hour', 'youtube'),
  ];

  /** The rows `/api/playlists/:id` serves. */
  readonly sharedPlaylist = {
    id: uuid(3),
    name: 'Road Trip',
    items: SONGS.slice(100, 112).map(song => ({
      url: song.link ?? 'https://soundcloud.com/encore/road-trip',
      source: song.uri.split(':')[0],
      title: song.name,
      artists: song.artists,
      artwork_url: song.image,
      duration_ms: song.duration_ms,
    })),
  };

  /** The computer's own player, as `/api/live/remote/state` reports it. */
  state: PlayerState = {
    song: SONGS[0]!,
    position_ms: 42_000,
    playing: false,
    liked: false,
    shuffle: false,
    repeat: 'off',
    volume: 62,
    device: 'Studio',
    next: SONGS.slice(1, 6),
    at: Date.now(),
  };

  /** What the phone last said it is playing, if anything. */
  phone: unknown = null;

  port = 0;
  requests: string[] = [];
  sent: Array<Record<string, unknown>> = [];
  eventRequests = 0;
  eventConnections = 0;
  droppedStreams = 0;
  phoneWrites = 0;
  computerOffline = false;
  private readonly allowedSecret = 'a'.repeat(64);

  get url(): string {
    return `http://127.0.0.1:${this.port}`;
  }

  songAt(index: number): Song {
    return SONGS[index]!;
  }

  countRequests(entry: string): number {
    return this.requests.filter(request => request === entry).length;
  }

  async start(): Promise<void> {
    this.http = createServer((request, response) => {
      this.handle(request, response).catch(error => {
        try {
          this.json(response, 500, { error: `mock failure: ${error instanceof Error ? error.message : String(error)}` });
        } catch { /* the answer was already sent */ }
      });
    });
    await new Promise<void>((resolve, reject) => {
      this.http!.once('error', reject);
      this.http!.listen(0, '127.0.0.1', () => resolve());
    });
    const address = this.http.address();
    this.port = typeof address === 'object' && address ? address.port : 0;
    this.keepAlive = setInterval(() => {
      for (const response of this.events) {
        try { response.write(': ping\n\n'); } catch { this.events.delete(response); }
      }
    }, 10_000);
    this.keepAlive.unref();
  }

  async close(): Promise<void> {
    this.closing = true;
    if (this.keepAlive) { clearInterval(this.keepAlive); this.keepAlive = null; }
    for (const response of [...this.events]) {
      try { response.end(); } catch { /* gone already */ }
    }
    this.events.clear();
    const http = this.http;
    this.http = null;
    if (!http) return;
    await new Promise<void>(resolve => {
      http.close(() => resolve());
      http.closeAllConnections?.();
    });
  }

  // ------------------------------------------------------------ scripted trouble
  /** The next command of this kind fails at the HTTP layer, as the service would. */
  failNext(kind: string, failure: Partial<Failure> = {}): void {
    const queued = this.failures.get(kind) ?? [];
    queued.push({ status: failure.status ?? 500, error: failure.error ?? 'Something went wrong.' });
    this.failures.set(kind, queued);
  }

  /** The next command of this kind takes this long to answer. */
  slowNext(kind: string, milliseconds: number): void {
    const queued = this.delays.get(kind) ?? [];
    queued.push(milliseconds);
    this.delays.set(kind, queued);
  }

  /** The computer stops answering: no fresh state, so the page shows it's away. */
  setComputerOffline(offline: boolean): void {
    this.computerOffline = offline;
    this.broadcast({ t: 'state', state: offline ? null : this.liveState(), now: Date.now() });
  }

  /** The computer's player changes, and listening pages hear about it at once. */
  setComputerState(change: Partial<PlayerState>): void {
    this.state = { ...this.state, ...change, at: Date.now() };
    this.broadcast({ t: 'state', state: this.liveState(), now: Date.now() });
  }

  /** The next this many event streams end the moment they open. */
  dropEventStream(times = 1): void {
    this.pendingDrops += times;
  }

  /** Hangs up on the pages listening now, so they open the stream again. */
  endEventStreams(): void {
    for (const response of [...this.events]) {
      this.events.delete(response);
      try { response.end(); } catch { /* gone already */ }
    }
  }

  // ------------------------------------------------------------ routing
  private async handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const url = new URL(request.url ?? '/', 'http://127.0.0.1');
    this.requests.push(`${request.method ?? 'GET'} ${url.pathname}`);
    if (url.pathname === '/api' || url.pathname.startsWith('/api/')) {
      await this.handleApi(request, response, url);
      return;
    }
    this.serveStatic(request, response, url.pathname);
  }

  private serveStatic(request: IncomingMessage, response: ServerResponse, pathname: string): void {
    const type = CONTENT[pathname] ?? (/^\/icon-\d+\.png$/.test(pathname) ? 'image/png' : null);
    const name = type ? pathname.slice(1) : 'index.html';
    const body = readFileSync(join(WEB, name));
    response.writeHead(200, {
      ...HEADERS,
      'Content-Type': type ?? 'text/html; charset=utf-8',
      'Cache-Control': type ? 'no-cache' : 'no-store',
      'Content-Length': body.length,
    });
    if (request.method === 'HEAD') { response.end(); return; }
    response.end(body);
  }

  private authorized(request: IncomingMessage): boolean {
    const header = request.headers.authorization ?? '';
    return header.startsWith('Bearer ') && /^[a-f0-9]{64}$/.test(header.slice(7));
  }

  private async handleApi(request: IncomingMessage, response: ServerResponse, url: URL): Promise<void> {
    const path = url.pathname.slice(4);
    const method = request.method ?? 'GET';

    if (path === '/link/claim' || path === '/auth/spotify') {
      return this.json(response, 200, { secret: this.allowedSecret, display_name: 'Sam' });
    }
    if (!this.authorized(request)) return this.json(response, 401, { error: 'Sign in to the group' });

    if (path === '/me' && method === 'GET') {
      return this.json(response, 200, { user_id: account.user_id, display_name: account.display_name });
    }
    if (path === '/account' && method === 'GET') return this.json(response, 200, account);
    if (path === '/members' && method === 'GET') {
      return this.json(response, 200, { members: account.groups[0]!.members.map(member => ({ user_id: member.user_id, display_name: member.display_name })) });
    }
    if (path === '/posts' && method === 'GET') return this.json(response, 200, { posts: this.posts, next_cursor: null });
    if (path === '/playlists' && method === 'GET') {
      return this.json(response, 200, {
        playlists: [{
          id: this.sharedPlaylist.id,
          name: this.sharedPlaylist.name,
          by: 'user-2',
          by_name: 'Robin',
          created_at: 1_760_000_000_000,
          count: this.sharedPlaylist.items.length,
          artwork_url: this.sharedPlaylist.items[0]!.artwork_url,
          to: [],
        }],
      });
    }
    if (path === '/live' && method === 'GET') {
      return this.json(response, 200, {
        now: Date.now(),
        presence: [{
          user_id: 'user-2',
          item: {
            url: 'https://soundcloud.com/encore/robin-now',
            source: 'soundcloud',
            title: 'Coastline',
            artists: ['Neon Harbour'],
            artwork_url: imageFor(9, 'soundcloud'),
            duration_ms: 214_000,
          },
          position_ms: 30_000,
          playing: true,
          at: Date.now(),
          following: [],
        }],
        channels: [],
      });
    }

    if (path === '/live/remote/events' && method === 'GET') {
      this.eventRequests++;
      response.writeHead(200, {
        'Content-Type': 'text/event-stream; charset=utf-8',
        'Cache-Control': 'no-store',
        'Connection': 'keep-alive',
        'X-Accel-Buffering': 'no',
      });
      response.write(': connected\n\n');
      if (this.pendingDrops > 0) {
        this.pendingDrops--;
        this.droppedStreams++;
        response.end();
        return;
      }
      this.eventConnections++;
      this.events.add(response);
      response.on('close', () => this.events.delete(response));
      this.send(response, { t: 'state', state: this.computerOffline ? null : this.liveState(), now: Date.now() });
      return;
    }

    if (path === '/live/remote/state' && method === 'GET') {
      return this.json(response, 200, { state: this.computerOffline ? null : this.liveState(), now: Date.now() });
    }

    if (path === '/live/remote/send' && method === 'POST') {
      const input = await this.readJson(request) as Record<string, unknown>;
      const kind = typeof input.kind === 'string' ? input.kind : '';
      this.sent.push(input);
      const failure = this.failures.get(kind)?.shift();
      if (failure) return this.json(response, failure.status, { error: failure.error });
      this.json(response, 202, { ok: true });
      const delay = this.delays.get(kind)?.shift() ?? 0;
      const timer = setTimeout(() => {
        const answer = this.answer(kind, input);
        this.replies.set(String(input.id), answer);
        this.broadcast({ t: 'reply', id: input.id });
      }, delay);
      timer.unref();
      return;
    }

    const reply = /^\/live\/remote\/reply\/([\w-]+)$/.exec(path);
    if (reply && method === 'GET') {
      const stored = this.replies.get(reply[1]!);
      return this.json(response, 200, stored === undefined ? { ready: false } : { ready: true, reply: stored });
    }

    if (path === '/live/remote/phone' && method === 'PUT') {
      this.phoneWrites++;
      this.phone = { ...await this.readJson(request) as Record<string, unknown>, at: Date.now() };
      return this.json(response, 200, { ok: true });
    }
    if (path === '/live/remote/phone' && method === 'DELETE') {
      this.phone = null;
      return this.json(response, 200, { ok: true });
    }

    const like = /^\/posts\/([\w-]+)\/like$/.exec(path);
    if (like && (method === 'PUT' || method === 'DELETE')) {
      const found = this.posts.find(row => row.id === like[1]);
      if (found) {
        found.liked = method === 'PUT';
        found.like_count = Math.max(0, found.like_count + (found.liked ? 1 : -1));
      }
      return this.json(response, 200, { ok: true });
    }
    const replies = /^\/posts\/([\w-]+)\/replies$/.exec(path);
    if (replies && method === 'POST') {
      const body = await this.readJson(request) as { text?: string };
      const found = this.posts.find(row => row.id === replies[1]);
      const reply = { id: uuid(9), author_id: 'user-1', author_name: 'Sam', created_at: Date.now(), text: body.text ?? '' };
      if (found) found.replies.push(reply);
      return this.json(response, 201, reply);
    }
    if (path === '/posts' && method === 'POST') {
      const body = await this.readJson(request) as Record<string, unknown>;
      return this.json(response, 201, { id: uuid(8), ...body, author_name: 'Sam' });
    }

    const shared = /^\/playlists\/([\w-]+)$/.exec(path);
    if (shared && method === 'GET') {
      if (shared[1] !== this.sharedPlaylist.id) return this.json(response, 404, { error: 'Playlist not found' });
      return this.json(response, 200, { id: this.sharedPlaylist.id, name: this.sharedPlaylist.name, items: this.sharedPlaylist.items });
    }

    return this.json(response, 404, { error: 'Not found' });
  }

  // ------------------------------------------------------------ the computer answers
  private answer(kind: string, input: Record<string, unknown>): unknown {
    switch (kind) {
      case 'home':
        return {
          picks: SONGS.slice(0, 10),
          recent: SONGS.slice(20, 28),
          made_for_you: PLAYLISTS.map(list => ({ ...list, count: 60 })),
          news: soundcloudSongs.slice(0, 5),
          following: SONGS.slice(40, 45),
        };
      case 'library':
        return {
          direct: false,
          playlists: PLAYLISTS.map(list => ({ id: list.id, name: list.name, image: list.image, uri: `spotify:playlist:${list.id}` })),
          mixed: [{ ...MIXED }, { id: 'mix-sunday', name: 'Sunday Kitchen', image: SONGS[50]!.image, count: 37 }],
        };
      case 'liked':
        return { name: 'Liked Songs', songs: LIKED };
      case 'playlist': {
        const found = PLAYLISTS.find(list => list.id === String(input.target)) ?? PLAYLISTS[0]!;
        return { name: found.name, uri: `spotify:playlist:${found.id}`, songs: spotifySongs.slice(0, 12) };
      }
      case 'mixed': {
        if (String(input.target) === 'mix-sunday') return { name: 'Sunday Kitchen', songs: SONGS.slice(50, 87) };
        return { name: MIXED.name, songs: SONGS };
      }
      case 'source':
        return { name: 'Neon Harbour', image: imageFor(3, 'soundcloud'), songs: soundcloudSongs.slice(0, 20) };
      case 'album':
        return { name: 'City Lights', uri: 'spotify:album:city-lights', songs: spotifySongs.slice(0, 10) };
      case 'artist':
        return {
          name: 'Neon Harbour',
          uri: 'soundcloud:user:neon-harbour',
          image: imageFor(6, 'soundcloud'),
          followers: 184_000,
          popular: SONGS.slice(0, 10),
          albums: PLAYLISTS.map((list, index) => ({ id: `album-${index}`, name: list.name, year: String(2021 + index), image: list.image })),
          looking: false,
        };
      case 'search':
        return {
          artists: [{ id: 'artist-1', name: 'Neon Harbour', image: imageFor(2, 'spotify') }],
          accounts: [{ source: 'soundcloud', uri: 'soundcloud:user:neon-harbour', name: 'Neon Harbour', image: imageFor(2, 'soundcloud'), followers: 12_400 }],
          spotify: spotifySongs.slice(0, 6),
          soundcloud: soundcloudSongs.slice(0, 6),
          youtube: youtubeSongs.slice(0, 6),
        };
      case 'lyrics':
        return { uri: this.state.song?.uri ?? null, lines: [], synced: false, instrumental: true };
      case 'stream':
        return { url: `https://media.example.test/${encodeURIComponent(String(input.uri))}.mp3`, hls: false };
      case 'play': {
        const uris = Array.isArray(input.uris) ? input.uris.map(String) : [];
        const index = Math.max(0, Number(input.index ?? 0));
        const queue = uris.map(uri => SONGS.find(song => song.uri === uri) ?? this.minimal(uri));
        const song = queue[index] ?? queue[0] ?? SONGS[0]!;
        this.setState({ song, position_ms: 0, playing: true, next: queue.slice(index + 1, index + 6) });
        return { ok: true };
      }
      case 'toggle':
        this.setState({ playing: !this.state.playing });
        return { ok: true };
      case 'next': {
        const next = this.state.next[0] ?? this.state.song;
        this.setState({ song: next, position_ms: 0, playing: true, next: this.state.next.slice(1) });
        return { ok: true };
      }
      case 'previous':
        this.setState({ position_ms: 0 });
        return { ok: true };
      case 'seek':
        this.setState({ position_ms: Math.max(0, Number(input.ms ?? 0)) });
        return { ok: true };
      case 'volume':
        this.setState({ volume: Math.min(100, Math.max(0, Number(input.percent ?? 0))) });
        return { ok: true };
      case 'shuffle':
        this.setState({ shuffle: !this.state.shuffle });
        return { ok: true };
      case 'repeat':
        this.setState({ repeat: this.state.repeat === 'off' ? 'all' : 'off' });
        return { ok: true };
      case 'like':
        this.setState({ liked: !this.state.liked });
        return { ok: true };
      case 'queue':
      case 'play_link':
        return { ok: true };
      default:
        return { error: `The computer does not answer "${kind}" here.` };
    }
  }

  private minimal(uri: string): Song {
    return { uri, name: 'Unknown song', artists: ['Unknown'], image: null, duration_ms: 180_000 };
  }

  private setState(change: Partial<PlayerState>): void {
    this.state = { ...this.state, ...change, at: Date.now() };
    this.broadcast({ t: 'state', state: this.liveState(), now: Date.now() });
  }

  private liveState(): PlayerState {
    this.state.at = Date.now();
    return this.state;
  }

  private broadcast(message: unknown): void {
    if (this.closing) return;
    for (const response of [...this.events]) this.send(response, message);
  }

  private send(response: ServerResponse, message: unknown): void {
    try {
      response.write(`data: ${JSON.stringify(message)}\n\n`);
    } catch {
      this.events.delete(response);
    }
  }

  private json(response: ServerResponse, status: number, value: unknown): void {
    if (response.writableEnded) return;
    const body = JSON.stringify(value);
    response.writeHead(status, {
      'Content-Type': 'application/json; charset=utf-8',
      'Cache-Control': 'no-store',
      'Content-Length': Buffer.byteLength(body),
    });
    response.end(body);
  }

  private readJson(request: IncomingMessage): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const chunks: Buffer[] = [];
      let size = 0;
      request.on('data', chunk => {
        size += chunk.length;
        if (size > 2 * 1024 * 1024) { reject(new Error('body too large')); request.destroy(); return; }
        chunks.push(Buffer.from(chunk));
      });
      request.on('end', () => {
        const text = Buffer.concat(chunks).toString('utf8');
        if (!text) { resolve(null); return; }
        try { resolve(JSON.parse(text)); } catch (error) { reject(error); }
      });
      request.on('error', reject);
    });
  }
}
