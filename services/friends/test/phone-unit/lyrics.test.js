import assert from 'node:assert/strict';
import test from 'node:test';
import { baseTitle, parseSynced, pick, sameSong, wordsOf } from '../../assets/web/js/lyrics.js';
import { matching, plain } from '../../assets/web/js/words.js';

test('typed words find what holds all of them, in any order', () => {
  const texts = ['Midnight City M83', 'Slow Static Bonobo', 'City Lights Bonobo'].map(plain);
  assert.deepEqual(matching(texts, 'city'), [0, 2]);
  assert.deepEqual(matching(texts, 'BONOBO  city'), [2]);
  assert.deepEqual(matching(texts, 'nothing'), []);
  assert.deepEqual(matching(texts, '  '), []);
});

const song = { uri: 'spotify:track:1', name: 'Midnight City', artists: ['M83'], duration_ms: 244000 };

test('names compare without case, accents or punctuation', () => {
  assert.equal(plain('M83.'), 'm83');
  assert.equal(plain('Beyoncé'), 'beyonce');
  assert.equal(plain("Don't  Stop!"), 'don t stop');
});

test('a title loses what a service adds after it', () => {
  assert.equal(baseTitle('Heroes - 2017 Remaster'), 'Heroes');
  assert.equal(baseTitle('Holding (feat. Someone)'), 'Holding');
  assert.equal(baseTitle('(Intro)'), '(Intro)');
});

test('timed words are read in order, one entry per time', () => {
  const lines = parseSynced('[00:20.50] Second\n[00:05.00] First\n[01:10.00][02:10.00] Again\nno time here\n[00:30] ');
  assert.deepEqual(lines.map(line => [line.at_ms, line.text]), [[5000, 'First'], [20500, 'Second'], [30000, ''], [70000, 'Again'], [130000, 'Again']]);
});

test('a transcript is timed words, plain words, no words at all, or nothing', () => {
  assert.equal(wordsOf({ syncedLyrics: '[00:01.00] a', plainLyrics: 'a' }).synced, true);
  assert.deepEqual(wordsOf({ plainLyrics: 'one\n\ntwo\n\n' }), { lines: [{ text: 'one' }, { text: '' }, { text: 'two' }], synced: false, instrumental: false });
  assert.deepEqual(wordsOf({ instrumental: true }), { lines: [], synced: false, instrumental: true });
  assert.equal(wordsOf({ plainLyrics: '  \n ' }), null);
  assert.equal(wordsOf(null), null);
});

test('the same song has the same name and artist, and nearly the same length', () => {
  assert.equal(sameSong({ trackName: 'Midnight City', artistName: 'M83.', duration: 244 }, song), true);
  assert.equal(sameSong({ trackName: 'Midnight City', artistName: 'M83', duration: 251 }, song), false);
  assert.equal(sameSong({ trackName: 'Midnight City (Live)', artistName: 'M83', duration: 244 }, song), false);
  assert.equal(sameSong({ trackName: 'Midnight City', artistName: 'Someone Else', duration: 244 }, song), false);
  // A remaster is still the song, and a song of unknown length is taken on its name.
  assert.equal(sameSong({ trackName: 'Heroes', artistName: 'David Bowie', duration: 371 }, { name: 'Heroes - 2017 Remaster', artists: ['David Bowie'], duration_ms: 371000 }), true);
  assert.equal(sameSong({ trackName: 'Midnight City', artistName: 'M83', duration: 300 }, { ...song, duration_ms: 0 }), true);
});

test('timed words win over plain ones, and other songs are passed over', () => {
  const records = [
    { trackName: 'Midnight City', artistName: 'Cover Band', duration: 244, syncedLyrics: '[00:01.00] wrong' },
    { trackName: 'Midnight City', artistName: 'M83', duration: 244, plainLyrics: 'plain' },
    { trackName: 'Midnight City', artistName: 'M83', duration: 245, syncedLyrics: '[00:01.00] timed' },
  ];
  assert.equal(pick(records, song).lines[0].text, 'timed');
  assert.equal(pick(records.slice(0, 2), song).lines[0].text, 'plain');
  assert.equal(pick(records.slice(0, 1), song), null);
  assert.equal(pick(null, song), null);
});
