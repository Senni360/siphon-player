// A playlist, album, likes, or a mixed list.

import { art, empty, h, icon, plural, safeImage, SOURCE_NAMES, sourceOf, span } from '../dom.js';
import { load } from '../load.js';
import { current, topBar } from '../navigation.js';
import { playFrom } from '../player.js';
import { ask } from '../remote.js';
import { rows, shuffled } from '../rows.js';

/** A list's cover: its own, or its first four different covers as one. */
export function coverOf(image, songs) {
  if (safeImage(image)) return art(image, 'list-cover');
  const covers = [...new Set(songs.map(song => safeImage(song.image)).filter(Boolean))].slice(0, 4);
  if (covers.length < 4) return art(covers[0] ?? null, 'list-cover');
  return h('div', { class: 'list-cover mosaic' }, covers.map(cover => art(cover, '')));
}

/** "200 songs · 11 hr 2 min" and where they come from, when that is more than one place. */
export function listFacts(songs) {
  const from = [...new Set(songs.map(song => sourceOf(song.uri)))].map(source => SOURCE_NAMES[source]);
  const length = span(songs.reduce((sum, song) => sum + (song.duration_ms || 0), 0));
  const where = from.length > 1 ? from.join(', ').replace(/, ([^,]*)$/, ' and $1') : '';
  return [plural(songs.length, 'song'), length, where].filter(Boolean).join(' · ');
}

/** The top of any list of songs: cover, name, what's in it, Play and Shuffle. */
export function listHead(name, songs, { image, note, round = false } = {}) {
  const cover = round ? art(image, 'list-cover circle') : coverOf(image, songs);
  return h('header', { class: 'list-head' }, cover,
    h('h1', { class: 'title', text: name }),
    h('p', { class: 'note', text: [note, listFacts(songs)].filter(Boolean).join(' · ') }),
    songs.length ? h('div', { class: 'list-actions' },
      h('button', { class: 'play-all', onclick: () => playFrom(songs, 0) }, icon('play'), 'Play'),
      h('button', { class: 'play-all second', onclick: () => playFrom(shuffled(songs), 0) }, icon('shuffle'), 'Shuffle')) : null);
}

export function listScreen() {
  const body = h('div');
  const bar = topBar();
  const { kind, id } = current;
  load(async () => {
    const found = await ask(kind, id ? { target: id } : {});
    const songs = found.songs ?? [];
    const name = found.name ?? 'Songs';
    bar.setTitle(name);
    return [
      listHead(name, songs, { image: found.image }),
      songs.length ? h('div', { class: 'upnext' }, rows(songs, 50)) : empty('Nothing here yet.'),
    ];
  }, body, 'list');
  return [bar, body];
}
