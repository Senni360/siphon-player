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
import type { Socket } from 'node:net';
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { TestSocket } from './websocket.js';

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
  /** What this computer can do for a phone beyond playing: older ones say nothing. */
  can?: string[];
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

// ---------------------------------------------------------------- the mock edge's library
// The shapes `services/edge/src/library.mjs` answers with: Spotify ids, artists
// as `{ id, name }`, and mixed rows that may name a file the edge holds.
const EDGE_ARTIST_ID = '4dpOu9hZz1pZ0F9vB6mZ0v';

function edgeTrack(song: Song): Record<string, unknown> {
  const spotify = song.uri.startsWith('spotify:');
  return {
    source: spotify ? 'spotify' : song.uri.split(':')[0],
    uri: song.uri,
    name: song.name,
    artists: song.artists.map(name => ({ id: spotify ? EDGE_ARTIST_ID : null, name })),
    album: null,
    image: song.image,
    duration_ms: song.duration_ms,
  };
}

function edgeArtist(): Record<string, unknown> {
  return {
    source: 'spotify',
    id: EDGE_ARTIST_ID,
    uri: `spotify:artist:${EDGE_ARTIST_ID}`,
    name: 'Neon Harbour',
    image: imageFor(6, 'spotify'),
    genres: ['dream pop'],
    followers: 184_000,
  };
}

function edgeAlbum(index: number): Record<string, unknown> {
  const list = PLAYLISTS[index % PLAYLISTS.length]!;
  return {
    source: 'spotify',
    id: `album${index}`,
    uri: `spotify:album:album${index}`,
    name: list.name,
    image: imageFor(index, 'spotify'),
    artists: [{ id: EDGE_ARTIST_ID, name: 'Neon Harbour' }],
    release_date: `202${1 + index}-05-01`,
    total_tracks: 10,
    album_type: 'album',
  };
}

const EDGE_PLAYLIST_TRACKS = [spotifySongs.slice(0, 60), spotifySongs.slice(0, 12), spotifySongs.slice(12, 24)];
/** The edge's own ids have no dashes, as a Spotify id never does. */
const EDGE_PLAYLISTS = PLAYLISTS.map((list, index) => ({
  source: 'spotify',
  id: list.id.replaceAll('-', ''),
  uri: `spotify:playlist:${list.id.replaceAll('-', '')}`,
  name: list.name,
  image: list.image,
  owner: { id: 'user-1', name: 'Sam' },
  track_count: EDGE_PLAYLIST_TRACKS[index]!.length,
  collaborative: false,
}));
const EDGE_PLAYLIST_ROWS = EDGE_PLAYLIST_TRACKS.map(tracks => tracks.map(edgeTrack));

/** A mixed playlist, with its first song's file held by the edge. */
const EDGE_MIXED = {
  source: 'mixed',
  id: '0123456789abcdef',
  uri: 'spotifast:playlist:0123456789abcdef',
  name: 'Everything, 200 Songs',
  image: SONGS[0]!.image,
  track_count: 60,
};
const EDGE_MIXED_ROWS = SONGS.slice(0, 60).map((song, index) => ({
  source: 'mixed',
  uri: index === 0 ? 'spotify:local:Sam:Kitchen:Midnight%20City:214' : song.uri,
  name: song.name,
  artists: song.artists.map(name => ({ id: null, name })),
  album: null,
  image: song.image,
  duration_ms: song.duration_ms,
  ...(index === 0 ? { upload: 'edgeupload01' } : {}),
}));

// ---------------------------------------------------------------- the page's headers
// Copied from src/web.ts (HEADERS). Keep the two in step: the page is only meant
// to load itself, Spotify's own sign-in and API, the edge, and images from
// wherever art is. The local mock edge is this server, on 127.0.0.1.
const HEADERS: Record<string, string> = {
  'Content-Security-Policy': "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' https: data:; media-src https: http://127.0.0.1:*; font-src 'self'; connect-src 'self' https://accounts.spotify.com https://api.spotify.com https://lrclib.net https://edge.encore.senni.dev wss://edge.encore.senni.dev http://127.0.0.1:* ws://127.0.0.1:*; manifest-src 'self'; worker-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
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

// The same shell caching as src/web.ts: versioned asset URLs may be kept for
// good, while the page and manifest revalidate on every open. The app and its
// modules share one build version, stamped into each relative import.
const hashOf = (body: Buffer | string): string => createHash('sha256').update(body).digest('hex').slice(0, 12);
const bytesOf = (name: string): Buffer => readFileSync(join(WEB, name));
const JS_FILES = readdirSync(join(WEB, 'js'), { recursive: true, encoding: 'utf8' })
  .filter(name => name.endsWith('.js'))
  .map(name => name.replaceAll('\\', '/'))
  .sort()
  .map(name => `js/${name}`);
const MODULES = new Set(JS_FILES);
let build: string | null = null;
const buildVersion = (): string => (build ??= hashOf([bytesOf('app.js'), ...JS_FILES.map(name => bytesOf(name))].map(body => body.toString('utf8')).join('\n')));
const versionOf = (path: string): string => (path === '/app.js' || MODULES.has(path.slice(1)) ? buildVersion() : hashOf(bytesOf(path.slice(1))));
const asset = (path: string): string => {
  if (path === '/manifest.webmanifest') return `${path}?v=${hashOf(bytesOf('manifest.webmanifest'))}`;
  return `${path}?v=${versionOf(path)}`;
};
const MODULE_IMPORT = /(\bfrom\s*|\bimport\s*)(['"])(\.{1,2}\/[^'"]+?\.js)\2/g;
const scriptBody = (name: string): Buffer => Buffer.from(bytesOf(name).toString('utf8').replace(MODULE_IMPORT,
  (_match: string, lead: string, quote: string, specifier: string) => `${lead}${quote}${specifier}?v=${buildVersion()}${quote}`));
const bodyFor = (name: string): Buffer => (name === 'app.js' || MODULES.has(name) ? scriptBody(name) : bytesOf(name));
function versionedIndex(): Buffer {
  const source = readFileSync(join(WEB, 'index.html'), 'utf8');
  return Buffer.from(source
    .replaceAll('href="/app.css"', `href="${asset('/app.css')}"`)
    .replaceAll('src="/app.js"', `src="${asset('/app.js')}"`)
    .replaceAll('href="/manifest.webmanifest"', `href="${asset('/manifest.webmanifest')}"`)
    .replaceAll('href="/icon.svg"', `href="${asset('/icon.svg')}"`)
    .replaceAll('href="/icon-180.png"', `href="${asset('/icon-180.png')}"`));
}
const shellUrls = (): string[] => ['/', asset('/app.css'), asset('/app.js'), ...JS_FILES.map(name => asset(`/${name}`)),
  asset('/manifest.webmanifest'), asset('/icon.svg'), asset('/icon-180.png'), asset('/icon-192.png'), asset('/icon-512.png')];
function serviceWorker(): Buffer {
  const urls = shellUrls();
  return Buffer.from(readFileSync(join(WEB, 'sw.js'), 'utf8')
    .replace('__VERSION__', hashOf(urls.join('\n') + versionedIndex().toString()))
    .replace('__SHELL__', JSON.stringify(urls)));
}

export interface Failure {
  status: number;
  error: string;
}

const uuid = (number: number): string => `4d544a6f-8d43-4f1a-9e2c-${String(number).padStart(12, '0')}`;

/** Whether the edge answers this path from its read-only library. */
const isEdgeLibrary = (pathname: string): boolean => pathname === '/v1/home'
  || pathname === '/v1/search'
  || pathname.startsWith('/v1/library/')
  || pathname.startsWith('/v1/artist/')
  || pathname.startsWith('/v1/album/');

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
    can: ['download'],
    at: Date.now(),
  };

  /** What the phone last said it is playing, if anything. */
  phone: unknown = null;

  // ------------------------------------------------------------ the mock edge
  /** Whether `/api/edge/token` answers tokens at all. Off by default, as today. */
  edgeOn = false;
  /** The token route answers 503, as a service without the shared secret does. */
  edgeDown = false;
  /** The edge has no library vault: its library routes answer 503, as a fresh edge does. */
  edgeNoVault = false;
  /** Whether a computer is on the edge's `/v1/live` room. */
  edgeComputerPresent = false;
  /** Sources `/v1/resolve` refuses. */
  readonly edgeRefused = new Set<string>();
  /** Songs the computer has no audio for, as a Spotify-only match would be. */
  readonly noAudio = new Set<string>();
  /** Songs the phone asked the computer to download, in the order asked. */
  readonly downloads: Array<Record<string, unknown>> = [];
  /** The next `/api/edge/token` names a media token that is already expired. */
  edgeExpireNext = false;
  /** What `/v1/match` answers. Null means no match. */
  edgeMatchSource: string | null = 'soundcloud:encore/song-1';
  /** The saved matches `/v1/sync/matches` reports. */
  edgeSaved: Record<string, unknown> = {};

  edgeTokenRequests = 0;
  edgeResolveCalls: string[] = [];
  edgeWarmed: string[] = [];
  edgeMatchCalls = 0;
  edgeStateSent = 0;
  wsCommands: Array<Record<string, unknown>> = [];

  port = 0;
  requests: string[] = [];
  sent: Array<Record<string, unknown>> = [];
  claims: Array<Record<string, unknown>> = [];
  eventRequests = 0;
  eventConnections = 0;
  droppedStreams = 0;
  phoneWrites = 0;
  computerOffline = false;
  revoked = false;
  private readonly allowedSecret = 'a'.repeat(64);
  private readonly edgeMedia = 'edge-media-token';
  private readonly edgeSync = 'edge-sync-token';
  private readonly edgeLive = 'edge-live-token';
  private readonly peers: TestSocket[] = [];
  private wsSeq = 0;

  /** The service stops accepting the phone's secret, as after a removal. */
  revoke(): void {
    this.revoked = true;
  }

  get url(): string {
    return `http://127.0.0.1:${this.port}`;
  }

  get edgePeers(): number {
    return this.peers.length;
  }

  songAt(index: number): Song {
    return SONGS[index]!;
  }

  countRequests(entry: string): number {
    return this.requests.filter(request => request === entry).length;
  }

  /** The edge is reachable: tokens, resolve, match, warm and a live room. */
  enableEdge(options: { computer?: boolean; expireFirst?: boolean; match?: string | null; saved?: Record<string, unknown> } = {}): void {
    this.edgeOn = true;
    this.edgeDown = false;
    this.edgeComputerPresent = !!options.computer;
    this.edgeExpireNext = !!options.expireFirst;
    if (options.match !== undefined) this.edgeMatchSource = options.match;
    if (options.saved !== undefined) this.edgeSaved = options.saved;
  }

  /** The edge is away: the token route answers 503 and no socket is offered. */
  disableEdge(): void {
    this.edgeOn = false;
    this.edgeDown = true;
    this.edgeComputerPresent = false;
  }

  /** The edge is up but has no library vault, so its library routes answer 503. */
  setEdgeNoVault(noVault: boolean): void {
    this.edgeNoVault = noVault;
  }

  /** A computer joins or leaves the live room, and the page hears either. */
  setEdgeComputerPresent(present: boolean): void {
    this.edgeComputerPresent = present;
    if (!present) return;
    for (const peer of this.peers) this.sendEdgeState({}, peer);
  }

  /** The computer's player changes, but only the edge socket hears it. */
  sendEdgeState(change: Partial<PlayerState> = {}, only?: TestSocket): void {
    this.state = { ...this.state, ...change, at: Date.now() };
    this.edgeStateSent++;
    const payload = {
      v: 1, t: 'state', id: `pc-${++this.wsSeq}`, from: 'computer', seq: this.wsSeq,
      body: { v: 1, t: 'state', id: `pc-state-${this.wsSeq}`, state: { ...this.state }, at: Date.now() },
    };
    for (const peer of only ? [only] : this.peers) peer.send(payload);
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
    this.http.on('upgrade', (request, socket) => this.upgrade(request, socket as Socket));
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
    for (const peer of this.peers) peer.close();
    this.peers.length = 0;
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
    if (url.pathname.startsWith('/v1/')) {
      await this.handleEdge(request, response, url);
      return;
    }
    if (url.pathname === '/api' || url.pathname.startsWith('/api/')) {
      await this.handleApi(request, response, url);
      return;
    }
    this.serveStatic(request, response, url);
  }

  // ------------------------------------------------------------ the mock edge's socket
  private upgrade(request: IncomingMessage, socket: Socket): void {
    const url = new URL(request.url ?? '/', 'http://127.0.0.1');
    const allowed = this.edgeOn && !this.edgeDown && url.pathname === '/v1/live' && url.searchParams.get('token') === this.edgeLive;
    if (!allowed) {
      try {
        socket.write('HTTP/1.1 401 Unauthorized\r\nConnection: close\r\nContent-Length: 0\r\n\r\n');
      } catch { /* gone already */ }
      socket.destroy();
      return;
    }
    let peer: TestSocket | null = null;
    peer = TestSocket.accept(request, socket, text => { if (peer) this.edgeMessage(peer, text); });
    if (!peer) return;
    this.peers.push(peer);
    this.sendHello(peer);
  }

  private sendHello(peer: TestSocket): void {
    const state = this.edgeComputerPresent
      ? {
          t: 'state',
          seq: ++this.wsSeq,
          body: { v: 1, t: 'state', id: `pc-state-${this.wsSeq}`, state: { ...this.liveState() }, at: Date.now() },
        }
      : null;
    peer.send({
      v: 1, t: 'hello', id: 'srv-test', ok: true, role: 'phone', sub: 'user-1',
      seq: this.wsSeq, epoch: 'test', reset: false, replayed: 0, state,
      heartbeatMs: 15000, bufferMs: 120000,
    });
  }

  private edgeMessage(peer: TestSocket, text: string): void {
    let message: Record<string, unknown>;
    try { message = JSON.parse(text) as Record<string, unknown>; } catch { return; }
    if (!message || typeof message !== 'object') return;
    this.wsCommands.push(message);
    if (message.t === 'command') {
      const answer = this.answer(String(message.kind ?? ''), message) as Record<string, unknown>;
      peer.send({ v: 1, t: 'reply', id: message.id, from: 'computer', seq: ++this.wsSeq, ...answer });
    } else if (message.t === 'phone') {
      this.phone = message.phone ?? null;
    }
  }

  // ------------------------------------------------------------ the mock edge's HTTP API
  private async handleEdge(request: IncomingMessage, response: ServerResponse, url: URL): Promise<void> {
    const auth = (request.headers.authorization ?? '').replace(/^Bearer /, '');
    const method = request.method ?? 'GET';

    if (url.pathname === '/v1/sync/matches' && method === 'GET') {
      if (auth !== this.edgeSync) return this.edgeError(response, 401, 'unauthorized');
      return this.json(response, 200, { v: 1, doc: 'matches', revision: 1, updatedAt: Date.now(), data: this.edgeSaved });
    }
    // The library is read with the sync token, and a 503 without a vault.
    if (isEdgeLibrary(url.pathname)) {
      if (method !== 'GET' && method !== 'HEAD') return this.edgeError(response, 405, 'method_not_allowed');
      if (auth !== this.edgeSync) return this.edgeError(response, 401, 'unauthorized');
      if (this.edgeNoVault) return this.edgeError(response, 503, 'vault_not_configured');
      return this.edgeLibrary(response, url);
    }
    // The expired token is the first one the token route handed out.
    if (auth !== this.edgeMedia) return this.edgeError(response, 401, 'unauthorized');

    if (url.pathname === '/v1/media-token' && method === 'POST') {
      const input = await this.readJson(request) as { key?: unknown } | null;
      if (typeof input?.key !== 'string' || !input.key) return this.edgeError(response, 400, 'invalid_key');
      // This mock's media route serves every key it is given.
      return this.json(response, 200, {
        v: 1,
        key: input.key,
        url: `/v1/media/${encodeURIComponent(input.key)}?token=${encodeURIComponent(auth)}`,
        expiresIn: 300,
      });
    }

    if (url.pathname === '/v1/resolve' && method === 'POST') {
      const input = await this.readJson(request) as { source?: unknown } | null;
      const source = typeof input?.source === 'string' ? input.source : '';
      this.edgeResolveCalls.push(source);
      if (!source || this.edgeRefused.has(source)) return this.edgeError(response, 400, 'invalid_source');
      return this.json(response, 200, { v: 1, key: 'edge-key', contentType: 'audio/mp4', size: 1, cached: false, url: `/v1/media/edge-key?token=${encodeURIComponent(auth)}` });
    }
    if (url.pathname === '/v1/match' && method === 'POST') {
      const input = await this.readJson(request) as Record<string, unknown> | null;
      this.edgeMatchCalls++;
      if (!input || typeof input.name !== 'string' || !input.name || !Array.isArray(input.artists) || !input.artists.length) {
        return this.edgeError(response, 400, 'invalid_track');
      }
      const match = this.edgeMatchSource
        ? { source: this.edgeMatchSource, provider: 'soundcloud', score: 0.98, title: input.name, uploader: 'Encore', duration: Number(input.durationMs ?? 0) / 1000 }
        : null;
      return this.json(response, 200, { v: 1, match });
    }
    if (url.pathname === '/v1/warm' && method === 'POST') {
      const input = await this.readJson(request) as { sources?: unknown } | null;
      const sources = Array.isArray(input?.sources) ? input.sources.map(String) : [];
      this.edgeWarmed.push(...sources);
      return this.json(response, 202, { v: 1, warmed: sources.map(() => ({ key: 'warm', state: 'queued' })), rejected: 0 });
    }
    if (url.pathname.startsWith('/v1/media/') && (method === 'GET' || method === 'HEAD')) {
      response.writeHead(200, { 'Content-Type': 'audio/mp4', 'Content-Length': 1, 'Cache-Control': 'private, max-age=3600', 'Accept-Ranges': 'bytes' });
      response.end(method === 'HEAD' ? undefined : Buffer.from([0]));
      return;
    }
    return this.edgeError(response, 404, 'not_found');
  }

  private edgeError(response: ServerResponse, status: number, code: string): void {
    this.json(response, status, { v: 1, error: code });
  }

  // ------------------------------------------------------------ the mock edge's library
  /** One page of an edge list, with the base64url cursor the edge itself hands back. */
  private edgeSlice<T>(rows: T[], url: URL): { offset: number; items: T[]; next: string | null } {
    const raw = url.searchParams.get('limit');
    const asked = raw === null ? 50 : Number(raw);
    const limit = Number.isSafeInteger(asked) ? Math.min(50, Math.max(1, asked)) : 50;
    let offset = 0;
    const cursor = url.searchParams.get('cursor');
    if (cursor) {
      try {
        const value = JSON.parse(Buffer.from(cursor, 'base64url').toString('utf8')) as { o?: unknown };
        if (Number.isSafeInteger(value?.o) && (value.o as number) >= 0) offset = value.o as number;
      } catch { /* an unreadable cursor starts at the top */ }
    }
    const after = offset + limit;
    return {
      offset,
      items: rows.slice(offset, after),
      next: after < rows.length ? Buffer.from(JSON.stringify({ o: after }), 'utf8').toString('base64url') : null,
    };
  }

  /** The routes `LibraryService` serves a phone, in its own shapes. */
  private edgeLibrary(response: ServerResponse, url: URL): void {
    const path = url.pathname;
    const id = (prefix: string): string => decodeURIComponent(path.slice(prefix.length));

    if (path === '/v1/home') {
      return this.json(response, 200, {
        v: 1,
        recently_played: SONGS.slice(0, 14).map((song, at) => ({
          ...edgeTrack(song),
          played_at: new Date(1_760_000_000_000 + at * 60_000).toISOString(),
        })),
        new_releases: [edgeAlbum(0), edgeAlbum(1)],
        generated_at: new Date().toISOString(),
      });
    }
    if (path === '/v1/search') {
      const query = (url.searchParams.get('q') ?? '').trim();
      if (!query) return this.edgeError(response, 400, 'invalid_query');
      return this.json(response, 200, {
        v: 1,
        query,
        spotify: {
          tracks: spotifySongs.slice(0, 6).map(edgeTrack),
          artists: [edgeArtist()],
          albums: [edgeAlbum(0)],
          playlists: [EDGE_PLAYLISTS[0]!],
        },
        soundcloud: { tracks: soundcloudSongs.slice(0, 6).map(edgeTrack), playlists: [] },
        next: null,
      });
    }
    if (path === '/v1/library/playlists') {
      const page = this.edgeSlice(EDGE_PLAYLISTS, url);
      const items: unknown[] = [...page.items];
      if (page.offset === 0) items.push(EDGE_MIXED);
      return this.json(response, 200, { v: 1, items, next: page.next });
    }
    if (path.startsWith('/v1/library/playlists/')) {
      const playlistId = id('/v1/library/playlists/');
      const at = EDGE_PLAYLISTS.findIndex(list => list.id === playlistId);
      if (at >= 0) {
        const page = this.edgeSlice(EDGE_PLAYLIST_ROWS[at]!, url);
        return this.json(response, 200, { v: 1, playlist: EDGE_PLAYLISTS[at], items: page.items, next: page.next });
      }
      if (playlistId === EDGE_MIXED.id) {
        return this.json(response, 200, { v: 1, source: 'mixed', playlist: EDGE_MIXED, items: EDGE_MIXED_ROWS, next: null });
      }
      return this.edgeError(response, 404, 'not_found');
    }
    if (path === '/v1/library/liked') {
      const page = this.edgeSlice(LIKED.map(edgeTrack), url);
      return this.json(response, 200, { v: 1, items: page.items, next: page.next });
    }
    if (path === '/v1/library/soundcloud/likes') {
      const page = this.edgeSlice(soundcloudSongs.slice(0, 24).map(edgeTrack), url);
      return this.json(response, 200, { v: 1, items: page.items, next: page.next });
    }
    if (path === '/v1/library/albums') {
      const page = this.edgeSlice([edgeAlbum(0), edgeAlbum(1)], url);
      return this.json(response, 200, { v: 1, items: page.items, next: page.next });
    }
    if (path === '/v1/library/artists') {
      const page = this.edgeSlice([edgeArtist()], url);
      return this.json(response, 200, { v: 1, items: page.items, next: page.next });
    }
    if (path.startsWith('/v1/artist/')) {
      if (id('/v1/artist/') !== EDGE_ARTIST_ID) return this.edgeError(response, 404, 'not_found');
      return this.json(response, 200, { v: 1, artist: edgeArtist(), top_tracks: SONGS.slice(0, 10).map(edgeTrack) });
    }
    if (path.startsWith('/v1/album/')) {
      const found = /^album(\d+)$/.exec(id('/v1/album/'));
      if (!found) return this.edgeError(response, 404, 'not_found');
      const album = edgeAlbum(Number(found[1]));
      const page = this.edgeSlice(EDGE_PLAYLIST_TRACKS[1]!.map(edgeTrack), url);
      return this.json(response, 200, { v: 1, album, items: page.items, next: page.next });
    }
    return this.edgeError(response, 404, 'not_found');
  }

  private serveStatic(request: IncomingMessage, response: ServerResponse, url: URL): void {
    const pathname = url.pathname;
    const name = pathname.slice(1);
    const worker = pathname === '/sw.js';
    const script = name === 'app.js' || MODULES.has(name);
    const type = worker || script ? 'text/javascript; charset=utf-8'
      : CONTENT[pathname] ?? (/^\/icon-\d+\.png$/.test(pathname) ? 'image/png' : 'text/html; charset=utf-8');
    const body = worker ? serviceWorker()
      : script ? bodyFor(name)
      : CONTENT[pathname] || /^\/icon-\d+\.png$/.test(pathname) ? bytesOf(name) : versionedIndex();
    const immutable = !worker && pathname !== '/manifest.webmanifest'
      && !type.startsWith('text/html')
      && url.searchParams.get('v') === (script ? buildVersion() : hashOf(body));
    const etag = `"${hashOf(body)}"`;
    const headers = {
      ...HEADERS,
      'Content-Type': type,
      'Cache-Control': immutable ? 'public, max-age=31536000, immutable' : 'no-cache',
      ETag: etag,
    };
    if (request.headers['if-none-match'] === etag) {
      response.writeHead(304, headers);
      response.end();
      return;
    }
    response.writeHead(200, { ...headers, 'Content-Length': body.length });
    if (request.method === 'HEAD') { response.end(); return; }
    response.end(body);
  }

  private authorized(request: IncomingMessage): boolean {
    if (this.revoked) return false;
    const header = request.headers.authorization ?? '';
    return header.startsWith('Bearer ') && /^[a-f0-9]{64}$/.test(header.slice(7));
  }

  private async handleApi(request: IncomingMessage, response: ServerResponse, url: URL): Promise<void> {
    const path = url.pathname.slice(4);
    const method = request.method ?? 'GET';

    if (path === '/link/claim' || path === '/auth/spotify') {
      const input = await this.readJson(request).catch(() => null);
      if (path === '/link/claim') this.claims.push((input ?? {}) as Record<string, unknown>);
      return this.json(response, 200, { secret: this.allowedSecret, display_name: 'Sam' });
    }
    if (!this.authorized(request)) return this.json(response, 401, { error: 'Sign in to the group' });

    if (path === '/edge/token' && method === 'POST') {
      this.edgeTokenRequests++;
      if (this.edgeDown) return this.json(response, 503, { error: 'Edge is not configured' });
      const input = await this.readJson(request) as { role?: unknown } | null;
      if (input?.role !== 'phone' && input?.role !== 'computer') return this.json(response, 400, { error: 'Invalid role' });
      // Without an edge to offer, this older-shaped answer names none: the phone
      // falls back without a failed request in its console.
      if (!this.edgeOn) return this.json(response, 200, { url: null });
      let media = this.edgeMedia;
      if (this.edgeExpireNext) { this.edgeExpireNext = false; media = 'edge-media-expired'; }
      return this.json(response, 200, { url: this.url, live: this.edgeLive, media, sync: this.edgeSync });
    }

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
          releases: [
            { uri: 'spotify:album:release1', id: 'release1', name: 'Fragments', artists: ['Bonobo'], image: SONGS[3]!.image, kind: 'album', songs: 12 },
            { uri: 'spotify:album:release2', id: 'release2', name: 'Low Hours', artists: ['Tycho'], image: SONGS[8]!.image, kind: 'ep', songs: 5 },
            { uri: SONGS[12]!.uri, name: SONGS[12]!.name, artists: SONGS[12]!.artists, image: SONGS[12]!.image, kind: 'single', songs: 1, song: SONGS[12] },
          ],
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
        // A song the computer has nothing for answers with a reason instead:
        // the phone treats that as final, not as something to retry.
        if (this.noAudio.has(String(input.uri))) return { error: 'No audio for this song.' };
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
      case 'download':
        this.downloads.push(input);
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
