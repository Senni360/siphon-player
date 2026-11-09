// A source's own page: someone on SoundCloud or YouTube.

import { art, h, icon, SOURCE_NAMES, sourceOf } from '../dom.js';
import { load } from '../load.js';
import { backButton, current } from '../navigation.js';
import { playFrom } from '../player.js';
import { ask } from '../remote.js';
import { rows, shuffled } from '../rows.js';

export function sourceScreen() {
  const body = h('div');
  const { uri } = current;
  load(async () => {
    const found = await ask('source', { uri });
    const songs = found.songs ?? [];
    return [
      found.image ? h('div', { class: 'source-head' }, art(found.image, 'source-picture')) : null,
      h('h1', { class: 'title', text: found.name ?? 'Songs' }),
      h('p', { class: 'note', text: SOURCE_NAMES[sourceOf(uri)] }),
      songs.length ? h('div', { class: 'list-actions' },
        h('button', { class: 'play-all', onclick: () => playFrom(songs, 0) }, icon('play'), 'Play'),
        h('button', { class: 'play-all second', onclick: () => playFrom(shuffled(songs), 0) }, icon('shuffle'), 'Shuffle')) : h('p', { class: 'empty', text: 'Nothing here yet.' }),
      h('div', { class: 'upnext' }, rows(songs, 50)),
    ];
  }, body);
  return [backButton(), body];
}
