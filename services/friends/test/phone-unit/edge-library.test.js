import assert from 'node:assert/strict';
import test from 'node:test';

// The module itself belongs to the page, and its neighbours expect a document.
// Only the pure mapping is under test here, so a stub is all it needs.
globalThis.document = { getElementById: () => null };
const {
  EDGE_DIRECT, edgeArtist, edgeArtistRow, edgeBrowsing, edgeHome, edgeLibrary, edgeList, edgeListSongs,
  edgeSearch, edgeSong, edgeSongs,
} = await import('../../assets/web/js/edge-library.js');

/** A Spotify track as `library.mjs` maps one. */
const spotifyTrack = (change = {}) => ({
  source: 'spotify',
  uri: 'spotify:track:4uLU6hMCjMI75M1A2tKUQC',
  name: 'Midnight City',
  artists: [{ id: '0OdUWJ0sBjDrqHygGUXeCF', name: 'M83' }],
  album: { id: '6Xl5B8c3eQxHV0R5d1m4zT', name: 'Hurry Up, We Are Dreaming' },
  image: 'https://i.scdn.co/image/ab67616d0000b273abc',
  duration_ms: 244_000,
  ...change,
});

/** A SoundCloud track as `mapSoundcloudTrack` builds one. */
const soundcloudTrack = (change = {}) => ({
  source: 'soundcloud',
  uri: 'soundcloud:encore/coastline',
  name: 'Coastline',
  artists: [{ id: '12345', name: 'Neon Harbour' }],
  album: null,
  image: null,
  duration_ms: null,
  permalink: 'https://soundcloud.com/encore/coastline',
  ...change,
});

/** A mixed playlist row, with the file the edge keeps for it. */
const mixedRow = (change = {}) => ({
  source: 'mixed',
  uri: 'spotify:local:Sam:Kitchen:Paper%20Boats:214',
  name: 'Paper Boats',
  artists: [{ id: null, name: 'The Paper Kites' }],
  album: null,
  image: null,
  duration_ms: 214_000,
  upload: '4f2a9c',
  ...change,
});

const playlistRow = (change = {}) => ({
  source: 'spotify',
  id: '37i9dQZF1DXcBWIGoYBM5M',
  uri: 'spotify:playlist:37i9dQZF1DXcBWIGoYBM5M',
  name: 'Deep Focus',
  image: 'https://i.scdn.co/image/ab67706f000000027b2d1f7d4b9a3f1e0c2a5d6e',
  owner: { id: 'user-1', name: 'Sam' },
  track_count: 60,
  collaborative: false,
  ...change,
});

test('a song is mapped into what the screens read', () => {
  assert.deepEqual(edgeSong(spotifyTrack()), {
    uri: 'spotify:track:4uLU6hMCjMI75M1A2tKUQC',
    name: 'Midnight City',
    artists: ['M83'],
    artist_id: '0OdUWJ0sBjDrqHygGUXeCF',
    image: 'https://i.scdn.co/image/ab67616d0000b273abc',
    duration_ms: 244_000,
  });
});

test('a song without an image, a length or a uri is still shaped like one', () => {
  const bare = spotifyTrack({ image: null, duration_ms: null, artists: [] });
  assert.deepEqual(edgeSong(bare), {
    uri: 'spotify:track:4uLU6hMCjMI75M1A2tKUQC',
    name: 'Midnight City',
    artists: [],
    image: null,
    duration_ms: 0,
  });
  assert.equal(edgeSong(spotifyTrack({ uri: '' })), null);
  assert.equal(edgeSong(spotifyTrack({ uri: undefined })), null);
  assert.equal(edgeSong({ name: 'No uri' }), null);
  assert.equal(edgeSong(null), null);
  assert.equal(edgeSong('spotify:track:x'), null);
});

test('only a Spotify artist id opens the artist page', () => {
  assert.equal(edgeSong(soundcloudTrack()).artist_id, undefined);
  // A SoundCloud user's id is a number of its own: no such page to open.
  assert.equal(edgeSong(soundcloudTrack({ uri: 'soundcloud:encore/coastline' })).artists[0], 'Neon Harbour');
  assert.equal(edgeSong(spotifyTrack({ artists: [{ id: 'not an id', name: 'M83' }] })).artist_id, undefined);
});

test('a mixed playlist row keeps the edge upload key', () => {
  assert.equal(edgeSong(mixedRow()).upload, '4f2a9c');
  assert.equal(edgeSong(spotifyTrack()).upload, undefined);
  // A row the edge holds a file for but no uri cannot be played or placed in a list.
  assert.equal(edgeSong(mixedRow({ uri: null })), null);
});

test('a list of rows drops the ones that are not songs', () => {
  assert.deepEqual(edgeSongs([spotifyTrack(), null, { name: 'x' }, soundcloudTrack()]).map(song => song.uri),
    ['spotify:track:4uLU6hMCjMI75M1A2tKUQC', 'soundcloud:encore/coastline']);
  assert.deepEqual(edgeSongs(undefined), []);
  assert.deepEqual(edgeSongs([]), []);
});

test('a playlist row becomes the Library screen\'s card, count included', () => {
  assert.deepEqual(edgeList(playlistRow()), {
    id: '37i9dQZF1DXcBWIGoYBM5M',
    uri: 'spotify:playlist:37i9dQZF1DXcBWIGoYBM5M',
    name: 'Deep Focus',
    image: 'https://i.scdn.co/image/ab67706f000000027b2d1f7d4b9a3f1e0c2a5d6e',
    count: 60,
  });
  const uncounted = { ...playlistRow(), track_count: null };
  assert.equal('count' in edgeList(uncounted), false);
  assert.equal(edgeList(playlistRow({ id: undefined })), null);
  assert.equal(edgeList(null), null);
});

test('the library splits Spotify playlists from mixed ones', () => {
  const mixed = playlistRow({
    source: 'mixed', id: '0123456789abcdef', uri: 'spotifast:playlist:0123456789abcdef',
    name: 'Everything, 200 Songs', track_count: 60,
  });
  assert.deepEqual(edgeLibrary([playlistRow(), mixed, playlistRow({ id: 'pl2' }), null]).playlists.map(list => list.id),
    ['37i9dQZF1DXcBWIGoYBM5M', 'pl2']);
  assert.deepEqual(edgeLibrary([playlistRow(), mixed]).mixed.map(list => list.name), ['Everything, 200 Songs']);
});

test('an empty library is both lists, empty', () => {
  assert.deepEqual(edgeLibrary([]), { playlists: [], mixed: [] });
  assert.deepEqual(edgeLibrary(undefined), { playlists: [], mixed: [] });
});

test('a playlist\'s own page is its name, cover and songs', () => {
  const found = edgeListSongs(playlistRow(), [spotifyTrack(), mixedRow()]);
  assert.equal(found.name, 'Deep Focus');
  assert.equal(found.uri, 'spotify:playlist:37i9dQZF1DXcBWIGoYBM5M');
  assert.equal(found.image, playlistRow().image);
  assert.deepEqual(found.songs.map(song => song.name), ['Midnight City', 'Paper Boats']);
  assert.equal(found.songs[1].upload, '4f2a9c');
  assert.deepEqual(edgeListSongs(playlistRow(), []).songs, []);
  assert.equal(edgeListSongs(null, [spotifyTrack()]), null);
});

test('search gives the artists, Spotify songs and SoundCloud songs its screen reads', () => {
  const found = edgeSearch({
    v: 1,
    query: 'neon',
    spotify: { tracks: [spotifyTrack()], artists: [{ source: 'spotify', id: 'artistid', name: 'Neon Harbour', image: 'https://i.scdn.co/image/x', followers: 1 }] },
    soundcloud: { tracks: [soundcloudTrack()] },
  });
  assert.deepEqual(found.artists, [{ id: 'artistid', name: 'Neon Harbour', image: 'https://i.scdn.co/image/x' }]);
  assert.deepEqual(found.spotify.map(song => song.uri), ['spotify:track:4uLU6hMCjMI75M1A2tKUQC']);
  assert.deepEqual(found.soundcloud.map(song => song.uri), ['soundcloud:encore/coastline']);
  assert.equal(edgeSearch({ v: 1, query: 'x', spotify: { tracks: [] } }).soundcloud.length, 0);
  assert.equal(edgeSearch({ v: 1, query: 'x' }), null);
  assert.equal(edgeSearch(null), null);
});

test('an artist row needs an id to be openable', () => {
  assert.deepEqual(edgeArtistRow({ id: 'abc', name: 'Neon Harbour', image: null }), { id: 'abc', name: 'Neon Harbour', image: null });
  assert.equal(edgeArtistRow({ name: 'No id' }), null);
});

test('an artist page carries its popular songs and stops looking', () => {
  const found = edgeArtist({
    v: 1,
    artist: { source: 'spotify', id: 'abc', uri: 'spotify:artist:abc', name: 'Neon Harbour', image: 'https://i.scdn.co/image/x', genres: [], followers: 184_000 },
    top_tracks: [spotifyTrack(), spotifyTrack({ uri: 'spotify:track:second' })],
  });
  assert.equal(found.name, 'Neon Harbour');
  assert.equal(found.uri, 'spotify:artist:abc');
  assert.equal(found.image, 'https://i.scdn.co/image/x');
  assert.equal(found.followers, 184_000);
  assert.equal(found.looking, false);
  assert.deepEqual(found.popular.map(song => song.uri), ['spotify:track:4uLU6hMCjMI75M1A2tKUQC', 'spotify:track:second']);
  assert.equal(edgeArtist({ v: 1, artists: [] }), null);
  assert.equal(edgeArtist(null), null);
  assert.equal(edgeArtist({ v: 1, artist: { name: 'No numbers' }, top_tracks: [] }).followers, null);
});

test('home turns the recent plays into picks and the rest of jump back in', () => {
  const played = Array.from({ length: 14 }, (_, at) => ({ ...spotifyTrack({ uri: `spotify:track:${at}` }), played_at: '2026-11-10T10:00:00.000Z' }));
  const found = edgeHome({ v: 1, recently_played: played, new_releases: [] });
  assert.equal(found.picks.length, 11);
  assert.equal(found.recent.length, 3);
  assert.equal(found.picks[0].uri, 'spotify:track:0');
  assert.equal(found.recent[0].uri, 'spotify:track:11');
  // New releases are albums, which no shelf on this screen takes.
  assert.equal('news' in found, false);
  assert.equal('following' in found, false);
});

test('home with nothing played is still an answer', () => {
  assert.deepEqual(edgeHome({ v: 1, recently_played: [], new_releases: [] }), { picks: [], recent: [], releases: [] });
  assert.deepEqual(edgeHome({ v: 1, recently_played: Array.from({ length: 5 }, (_, at) => spotifyTrack({ uri: `spotify:track:${at}` })) }).recent, []);
  assert.equal(edgeHome({ v: 1 }), null);
  assert.equal(edgeHome(null), null);
});

test('every question answers null without an edge, and never throws', async () => {
  const asked = {
    home: {}, library: {}, liked: {}, playlist: { target: '37i9dQZF1DXcBWIGoYBM5M' },
    mixed: { target: '0123456789abcdef' }, album: { target: 'album0' }, soundcloud_likes: {},
    artist: { target: '4dpOu9hZz1pZ0F9vB6mZ0v' }, search: { query: 'm83' },
  };
  for (const [kind, args] of Object.entries(asked)) {
    assert.equal(await EDGE_DIRECT[kind](args), null, kind);
    assert.equal(edgeBrowsing(), false, kind);
  }
  // A kind the edge cannot answer is simply absent.
  assert.equal(EDGE_DIRECT.youtube_likes, undefined);
  assert.equal(EDGE_DIRECT.source, undefined);
  assert.equal(EDGE_DIRECT.stream, undefined);
  assert.equal(EDGE_DIRECT.lyrics, undefined);
  assert.equal(await EDGE_DIRECT.playlist({}), null);
  assert.equal(await EDGE_DIRECT.search({ query: '  ' }), null);
});

test("a SoundCloud song keeps its own public page, which the edge plays it from", () => {
  const row = { uri: 'soundcloud:track:7', name: 'Public', artists: [{ name: 'Artist' }], link: 'https://soundcloud.com/artist/public-song' };
  assert.equal(edgeSong(row).link, 'https://soundcloud.com/artist/public-song');
  assert.equal(edgeSong({ ...row, link: 'https://evil.example/x' }).link, undefined);
  assert.equal(edgeSong({ ...row, uri: 'spotify:track:abc' }).link, undefined);
});

test('a new release from the edge says what it is', async () => {
  const { edgeRelease } = await import('../../assets/web/js/edge-library.js');
  const album = { id: 'abc', uri: 'spotify:album:abc', name: 'Fragments', image: 'https://i.example/a.jpg', album_type: 'album', total_tracks: 12, artists: [{ id: 'x', name: 'Bonobo' }] };
  assert.deepEqual(edgeRelease(album), { uri: 'spotify:album:abc', id: 'abc', name: 'Fragments', artists: ['Bonobo'], image: 'https://i.example/a.jpg', kind: 'album', songs: 12 });
  assert.equal(edgeRelease({ ...album, album_type: 'single', total_tracks: 4 }).kind, 'ep');
  assert.equal(edgeRelease({ ...album, album_type: 'single', total_tracks: 1 }).kind, 'single');
  assert.equal(edgeRelease({ name: 'No id' }), null);
});
