import { test, expect, type Phone } from './fixtures.js';

/** Starts a song here without assuming an https address: the mock edge is local. */
async function play(phone: Phone, index: number): Promise<void> {
  const row = phone.row(index);
  const name = (await row.locator('.name').innerText()).trim();
  await row.locator('button.words').click();
  await expect(phone.page.locator('.mini .name')).toHaveText(name);
  await expect.poll(async () => (await phone.audio())?.paused, { timeout: 20_000 }).toBe(false);
}

const part = async (phone: Phone): Promise<string> => (await phone.audio())?.src ?? '';

test('a SoundCloud song plays from the edge, without asking the computer', async ({ phone, server }) => {
  server.enableEdge();
  await phone.open();
  await phone.openMix();

  const song = server.songAt(1);
  await play(phone, 1);
  await expect.poll(() => part(phone), { timeout: 20_000 }).toContain('/v1/media/');
  const audio = await phone.audio();
  expect(audio!.src).not.toContain('media.example.test');
  expect(server.edgeResolveCalls).toContain('soundcloud:encore/song-1');
  // This song's address did not come from the computer.
  expect(server.sent.filter(command => command.kind === 'stream' && command.uri === song.uri)).toEqual([]);
});

test('a Spotify song plays the edge\'s match', async ({ phone, server }) => {
  server.enableEdge();
  await phone.open();
  await phone.openMix();

  await play(phone, 0);
  await expect.poll(() => part(phone), { timeout: 20_000 }).toContain('/v1/media/');
  expect(server.edgeMatchCalls).toBeGreaterThan(0);
  expect(server.edgeResolveCalls).toContain('soundcloud:encore/song-1');
});

test('a saved match is used before searching again', async ({ phone, server }) => {
  const spotify = server.songAt(0);
  server.enableEdge({
    match: null,
    saved: { [spotify.uri]: { source: 'soundcloud', sourceUri: 'soundcloud:encore/saved', confidence: 1, manual: true } },
  });
  await phone.open();
  await phone.openMix();

  await play(phone, 0);
  await expect.poll(() => part(phone), { timeout: 20_000 }).toContain('/v1/media/');
  expect(server.edgeResolveCalls).toContain('soundcloud:encore/saved');
  expect(server.edgeMatchCalls).toBe(0);
});

test('with the edge down, the computer still answers', async ({ phone, server, observed }) => {
  // The real service answers 503 when EDGE_TOKEN_SECRET is unset.
  observed.allowConsole(/Failed to load resource: the server responded with a status of 503/);
  server.disableEdge();
  await phone.open();
  await phone.openMix();

  await play(phone, 1);
  await expect.poll(() => part(phone), { timeout: 20_000 }).toContain('media.example.test');
  expect(server.edgeTokenRequests).toBeGreaterThan(0);
  await expect.poll(() => server.sent.filter(command => command.kind === 'stream').length).toBeGreaterThan(0);
});

test('the edge refusing a track falls back to the computer', async ({ phone, server, observed }) => {
  observed.allowConsole(/Failed to load resource: the server responded with a status of 400/);
  server.enableEdge();
  server.edgeRefused.add('soundcloud:encore/song-1');
  await phone.open();
  await phone.openMix();

  await play(phone, 1);
  await expect.poll(() => part(phone), { timeout: 20_000 }).toContain('media.example.test');
  expect(server.edgeResolveCalls).toContain('soundcloud:encore/song-1');
  expect(server.sent.filter(command => command.kind === 'stream').length).toBeGreaterThan(0);
});

test('a token that ran out is refreshed once', async ({ phone, server, observed }) => {
  observed.allowConsole(/Failed to load resource: the server responded with a status of 401/);
  server.enableEdge({ expireFirst: true });
  await phone.open();
  await phone.openMix();

  await play(phone, 1);
  await expect.poll(() => part(phone), { timeout: 20_000 }).toContain('/v1/media/');
  expect(server.edgeResolveCalls).toContain('soundcloud:encore/song-1');
  expect(server.edgeTokenRequests).toBeGreaterThanOrEqual(2);
});

test('a computer on the edge takes commands, not the relay', async ({ phone, server }) => {
  server.enableEdge({ computer: true });
  await phone.open();
  await expect.poll(() => server.edgePeers).toBe(1);
  await phone.openMix();
  await play(phone, 1);
  await phone.openSheet();

  await phone.page.locator('.sheet .heart').click();
  await expect.poll(() => server.wsCommands.filter(command => command.kind === 'like').length).toBe(1);
  // Never down both paths.
  expect(server.sent.filter(command => command.kind === 'like')).toEqual([]);
});

test('without a computer on the edge, commands keep using the relay', async ({ phone, server }) => {
  server.enableEdge();
  await phone.open();
  await expect.poll(() => server.edgePeers).toBe(1);
  await phone.openMix();
  await play(phone, 1);
  await phone.openSheet();

  await phone.page.locator('.sheet .heart').click();
  await expect.poll(() => server.sent.filter(command => command.kind === 'like').length).toBe(1);
  expect(server.wsCommands.filter(command => command.kind === 'like')).toEqual([]);
});

test('the computer\'s state over the edge reaches the page', async ({ phone, server }) => {
  server.enableEdge({ computer: true });
  await phone.open();
  await expect.poll(() => server.edgePeers).toBe(1);

  const song = server.songAt(121);
  server.sendEdgeState({ song, position_ms: 12_000, playing: true });
  await expect(phone.page.locator('.mini .name')).toHaveText(song.name);
  expect(server.edgeStateSent).toBeGreaterThan(0);
});

test('the next songs are warmed on the edge', async ({ phone, server }) => {
  server.enableEdge();
  await phone.open();
  await phone.openMix();
  await play(phone, 0);

  await expect.poll(() => server.edgeWarmed.length, { timeout: 20_000 }).toBeGreaterThan(0);
  // Only public sources the edge accepts are sent.
  for (const source of server.edgeWarmed) {
    expect(source).toMatch(/^(youtube:video:[A-Za-z0-9_-]{11}|soundcloud:[A-Za-z0-9][A-Za-z0-9._-]*(?:\/[A-Za-z0-9][A-Za-z0-9._-]*)+)$/);
  }
});
