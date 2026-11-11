// The edge's own library, for a phone whose computer is not open.
//
// These are the same questions `ask` puts to the computer, answered from the
// edge's read-only copy of the listener's Spotify and SoundCloud: home, the
// library, playlists, liked songs, albums, artists and search. Each answer is
// mapped here into the exact shape its screen already reads, so nothing above
// this module knows which one answered.
//
// All of it is best effort. An absent edge, a 503, a 401, a timeout or a body
// this module does not recognise answers null, never an error, and `ask` goes
// on exactly as it did before.

import { edgeCall } from './edge.js';

/** The edge's largest page. */
const PAGE = 50;
/** A long list is read whole up to here; past it, the first songs do. */
const MOST_SONGS = 500;
/** A library is read whole up to here. */
const MOST_PLAYLISTS = 200;

let browsing = false;
/** Whether the edge answered this page's library the last time it was asked. */
export const edgeBrowsing = () => browsing;

const text = value => (typeof value === 'string' ? value : '');

/** One artist's name, from the edge's `{ id, name }` or a plain name. */
const artistName = artist => (typeof artist === 'string' ? artist : text(artist?.name));

/**
 * A song as the screens read one: `{ uri, name, artists, artist_id, image,
 * duration_ms }`, plus `upload` when the edge holds a mixed playlist's file.
 * A row without a uri is not a song the phone can play, so it is dropped.
 */
export function edgeSong(row) {
  if (!row || typeof row !== 'object') return null;
  const uri = text(row.uri);
  if (!uri) return null;
  const people = Array.isArray(row.artists) ? row.artists : [];
  // Only a Spotify artist opens the artist page: a SoundCloud user has a
  // numeric id, which that page cannot ask for.
  const first = uri.startsWith('spotify:')
    ? people.find(artist => typeof artist?.id === 'string' && /^[A-Za-z0-9]{1,64}$/.test(artist.id))
    : null;
  const song = {
    uri,
    name: text(row.name),
    artists: people.map(artistName).filter(Boolean),
    image: typeof row.image === 'string' ? row.image : null,
    duration_ms: Number.isFinite(row.duration_ms) ? row.duration_ms : 0,
  };
  if (first) song.artist_id = first.id;
  if (text(row.upload)) song.upload = row.upload;
  // A SoundCloud song's own page, which is what the edge plays it from.
  if (uri.startsWith('soundcloud:') && text(row.link).startsWith('https://soundcloud.com/')) song.link = row.link;
  return song;
}

export const edgeSongs = rows => (Array.isArray(rows) ? rows : []).map(edgeSong).filter(Boolean);

/** A playlist or mixed playlist as the Library screen reads one. */
export function edgeList(row) {
  if (!row || typeof row !== 'object' || typeof row.id !== 'string' || !row.id) return null;
  const list = {
    id: row.id,
    uri: text(row.uri),
    name: text(row.name),
    image: typeof row.image === 'string' ? row.image : null,
  };
  if (Number.isFinite(row.track_count)) list.count = row.track_count;
  return list;
}

/** The Library screen's two lists, from the edge's one page of playlists. */
export function edgeLibrary(items) {
  const playlists = [];
  const mixed = [];
  for (const row of Array.isArray(items) ? items : []) {
    const list = edgeList(row);
    if (!list) continue;
    (row.source === 'mixed' ? mixed : playlists).push(list);
  }
  return { playlists, mixed };
}

/** A playlist, album, likes or mixed list as the list screen reads one. */
export function edgeListSongs(head, rows) {
  if (!head || typeof head !== 'object') return null;
  return {
    name: text(head.name) || 'Songs',
    uri: text(head.uri),
    image: typeof head.image === 'string' ? head.image : null,
    songs: edgeSongs(rows),
  };
}

/** An artist's row in search results. */
export function edgeArtistRow(row) {
  if (!row || typeof row !== 'object' || !text(row.id)) return null;
  return { id: row.id, name: text(row.name), image: typeof row.image === 'string' ? row.image : null };
}

/** Search's artists, Spotify songs and SoundCloud songs. */
export function edgeSearch(answer) {
  if (!answer || typeof answer !== 'object') return null;
  const spotify = answer.spotify && typeof answer.spotify === 'object' ? answer.spotify : {};
  const soundcloud = answer.soundcloud && typeof answer.soundcloud === 'object' ? answer.soundcloud : {};
  if (!spotify.tracks && !soundcloud.tracks && !spotify.artists) return null;
  return {
    artists: (Array.isArray(spotify.artists) ? spotify.artists : []).map(edgeArtistRow).filter(Boolean),
    spotify: edgeSongs(spotify.tracks),
    soundcloud: edgeSongs(soundcloud.tracks),
  };
}

/** An artist page, with the songs the edge knows are popular. */
export function edgeArtist(answer) {
  if (!answer || typeof answer !== 'object' || !answer.artist || typeof answer.artist !== 'object') return null;
  const artist = answer.artist;
  return {
    name: text(artist.name),
    uri: text(artist.uri),
    image: typeof artist.image === 'string' ? artist.image : null,
    followers: Number.isFinite(artist.followers) ? artist.followers : null,
    popular: edgeSongs(answer.top_tracks),
    // The edge has no other sources to look at, so the page need not ask again.
    looking: false,
  };
}

/**
 * Home, from the edge's recent plays and new releases. The edge has no ranked
 * picks and no followed-artist news, so the most recent plays stand in as the
 * picks, and the plays after them fill "Jump back in". New releases are albums,
 * which no shelf this screen draws takes, so they are left out.
 */
export function edgeHome(answer) {
  if (!answer || typeof answer !== 'object') return null;
  if (!Array.isArray(answer.recently_played) && !Array.isArray(answer.new_releases)) return null;
  const played = edgeSongs(answer.recently_played);
  const releases = (Array.isArray(answer.new_releases) ? answer.new_releases : []).map(edgeRelease).filter(Boolean);
  return { picks: played.slice(0, 11), recent: played.slice(11, 23), releases };
}

/** A new album, EP or single from the edge, as Home's New releases shows one. */
export function edgeRelease(album) {
  if (!album || typeof album !== 'object' || !text(album.id)) return null;
  const kind = album.album_type === 'single' ? (album.total_tracks > 1 ? 'ep' : 'single') : 'album';
  return {
    uri: text(album.uri) || `spotify:album:${album.id}`,
    id: album.id,
    name: text(album.name),
    artists: (Array.isArray(album.artists) ? album.artists : []).map(artistName).filter(Boolean),
    image: typeof album.image === 'string' ? album.image : null,
    kind,
    songs: Number.isFinite(album.total_tracks) ? album.total_tracks : null,
  };
}

/**
 * Every page of one edge list, up to `most` rows, with the first page's own
 * answer for a playlist or album's name. Null the moment a page fails.
 */
async function edgePages(path, most) {
  const rows = [];
  let head = null;
  let cursor = null;
  for (let round = 0; round < 40 && rows.length < most; round++) {
    const tail = cursor ? `&cursor=${encodeURIComponent(cursor)}` : '';
    const answer = await edgeCall(`${path}?limit=${PAGE}${tail}`, 'sync', { method: 'GET' });
    if (!answer || !Array.isArray(answer.items)) return null;
    if (head === null) head = answer;
    rows.push(...answer.items);
    const next = text(answer.next);
    if (!next || next === cursor) break;
    cursor = next;
  }
  return { head, rows: rows.slice(0, most) };
}

/** One answer from the edge: its value, or null with browsing switched off. */
async function edgeOnly(work) {
  let found = null;
  try { found = await work(); } catch { found = null; }
  browsing = found !== null;
  return found;
}

/** A song list's own header, from the page the edge answered with. */
const headOf = (page, field) => (page.head && typeof page.head[field] === 'object' ? page.head[field] : null);

/** The questions the edge can answer, under the names the screens ask. */
export const EDGE_DIRECT = {
  home: () => edgeOnly(async () => edgeHome(await edgeCall('/v1/home', 'sync', { method: 'GET' }))),

  library: () => edgeOnly(async () => {
    const page = await edgePages('/v1/library/playlists', MOST_PLAYLISTS);
    return page ? edgeLibrary(page.rows) : null;
  }),

  liked: () => edgeOnly(async () => {
    const page = await edgePages('/v1/library/liked', MOST_SONGS);
    return page ? { name: 'Liked Songs', songs: edgeSongs(page.rows) } : null;
  }),

  playlist: ({ target } = {}) => edgeOnly(async () => {
    if (!target) return null;
    const page = await edgePages(`/v1/library/playlists/${encodeURIComponent(target)}`, MOST_SONGS);
    return page ? edgeListSongs(headOf(page, 'playlist'), page.rows) : null;
  }),

  mixed: ({ target } = {}) => edgeOnly(async () => {
    if (!target) return null;
    const page = await edgePages(`/v1/library/playlists/${encodeURIComponent(target)}`, MOST_SONGS);
    return page ? edgeListSongs(headOf(page, 'playlist'), page.rows) : null;
  }),

  album: ({ target } = {}) => edgeOnly(async () => {
    if (!target) return null;
    const page = await edgePages(`/v1/album/${encodeURIComponent(target)}`, MOST_SONGS);
    return page ? edgeListSongs(headOf(page, 'album'), page.rows) : null;
  }),

  soundcloud_likes: () => edgeOnly(async () => {
    const page = await edgePages('/v1/library/soundcloud/likes', MOST_SONGS);
    return page ? { name: 'SoundCloud likes', songs: edgeSongs(page.rows) } : null;
  }),

  artist: ({ target } = {}) => edgeOnly(async () => {
    if (!target) return null;
    return edgeArtist(await edgeCall(`/v1/artist/${encodeURIComponent(target)}`, 'sync', { method: 'GET' }));
  }),

  search: ({ query } = {}) => edgeOnly(async () => {
    const words = text(query).trim();
    if (!words) return null;
    return edgeSearch(await edgeCall(`/v1/search?q=${encodeURIComponent(words)}`, 'sync', { method: 'GET' }));
  }),
};
