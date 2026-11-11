import { test, expect } from './fixtures.js';

const lyricsButton = '.sheet .tools button:has-text("Lyrics")';

test("a song playing on this phone shows its own words, and a line taps to its place", async ({ phone, page }) => {
  const asked: string[] = [];
  page.on('request', request => { if (request.url().startsWith('https://lrclib.net/')) asked.push(request.url()); });
  await phone.open();
  await phone.openMix();
  const name = await phone.playRow(3);
  await phone.openSheet();
  await page.locator(lyricsButton).click();

  // The phone looked the words up itself, for the song it plays, not the computer's.
  const lines = page.locator('.sheet .lyrics button');
  await expect(lines).toHaveText(['First line', 'Second line', 'Third line']);
  expect(new URL(asked[0]!).searchParams.get('track_name')).toBe(name);

  await phone.advance(21);
  await expect(lines.nth(1)).toHaveClass(/now/);

  await lines.nth(2).click();
  await expect.poll(async () => Math.round((await phone.audio())?.currentTime ?? 0)).toBe(70);
});

test('a song nobody has written down says so, and the next song asks again', async ({ phone, page }) => {
  await phone.open();
  await phone.openMix();
  await phone.playRow(2);
  await phone.openSheet();
  await page.locator(lyricsButton).click();
  await expect(page.locator('.sheet .lyrics .quiet-note')).toHaveText('No lyrics for this song.');

  await page.locator('.sheet .transport button[aria-label="Next"]').click();
  await expect(page.locator('.sheet .lyrics button').first()).toHaveText('First line');
});

test("the computer's own song keeps the computer's words", async ({ phone, page }) => {
  const asked: string[] = [];
  page.on('request', request => { if (request.url().startsWith('https://lrclib.net/')) asked.push(request.url()); });
  await page.addInitScript(() => { localStorage.setItem('encore.target', 'computer'); });
  await phone.open();
  await phone.openSheet();
  await page.locator(lyricsButton).click();
  await expect(page.locator('.sheet .lyrics .quiet-note')).toHaveText('This one has no words.');
  expect(asked).toEqual([]);
});
