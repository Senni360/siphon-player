import { test, expect } from './fixtures.js';

test('opens a playlist and plays a song on this phone', async ({ phone, server }) => {
  await phone.open();
  await phone.openMix();

  // Row 1 is a SoundCloud song: the mixed list plays it here, not through Spotify.
  const name = await phone.playRow(1);
  const audio = await phone.audio();
  expect(audio).not.toBeNull();
  expect(audio!.src).toMatch(/^https:\/\//);
  expect(audio!.paused).toBe(false);
  expect(audio!.ended).toBe(false);
  expect(audio!.error).toBeNull();

  await expect(phone.page.locator('.mini .name')).toHaveText(name);
  expect(await phone.page.locator('.mini .name').getAttribute('class')).toContain('soundcloud');

  // The computer is told what the phone plays.
  await expect.poll(() => server.phoneWrites, { timeout: 10_000 }).toBeGreaterThan(0);
  await expect.poll(async () => {
    const told = server.phone as { song?: { name?: string }; playing?: boolean } | null;
    return `${told?.song?.name}|${told?.playing}`;
  }).toBe(`${name}|true`);
});

test('next and previous move through the playlist', async ({ phone }) => {
  await phone.open();
  await phone.openMix();

  const first = await phone.playRow(1);
  const second = (await phone.row(2).locator('.name').innerText()).trim();
  const third = (await phone.row(3).locator('.name').innerText()).trim();

  await phone.openSheet();
  await expect(phone.page.locator('.sheet .big')).toHaveText(first);

  await phone.page.locator('.sheet button[aria-label="Next"]').click();
  await expect(phone.page.locator('.mini .name')).toHaveText(second);
  await expect(phone.page.locator('.sheet .big')).toHaveText(second);

  await phone.page.locator('.sheet button[aria-label="Previous"]').click();
  await expect(phone.page.locator('.mini .name')).toHaveText(first);
  await expect(phone.page.locator('.sheet .big')).toHaveText(first);

  // And the queue is the list around it.
  await expect(phone.page.locator('.sheet .upnext .row .name').first()).toHaveText(second);
  await expect(phone.page.locator('.sheet .upnext .row .name').nth(1)).toHaveText(third);
});

test('a 200-song playlist keeps going past the sixth song', async ({ phone }) => {
  await phone.open();
  await phone.openMix();

  // The list is longer than a screenful: 50 rows are drawn from the 200.
  await expect(phone.page.locator('main .row')).toHaveCount(50);
  const seventh = (await phone.row(6).locator('.name').innerText()).trim();
  const seventhUri = await phone.row(6).getAttribute('data-uri');
  const eighth = (await phone.row(7).locator('.name').innerText()).trim();

  await phone.playRow(6);
  const audio = await phone.audio();
  expect(audio!.src).toContain(encodeURIComponent(seventhUri!));

  // It carries on to the seventh, not back to the top.
  await phone.openSheet();
  await phone.page.locator('.sheet button[aria-label="Next"]').click();
  await expect(phone.page.locator('.mini .name')).toHaveText(eighth);
  expect((await phone.audio())!.src).not.toContain(encodeURIComponent(seventhUri!));

  // The list still holds its later rows too.
  await expect(phone.page.locator('main .row')).toHaveCount(50);
});
