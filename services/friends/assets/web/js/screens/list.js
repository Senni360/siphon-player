// A playlist, album, likes, or a mixed list.

import { h, icon } from '../dom.js';
import { load } from '../load.js';
import { backButton, current } from '../navigation.js';
import { playFrom } from '../player.js';
import { ask } from '../remote.js';
import { rows, shuffled } from '../rows.js';

export function listScreen() {
  const body = h('div');
  const { kind, id } = current;
  load(async () => {
    const found = await ask(kind, id ? { target: id } : {});
    const songs = found.songs ?? [];
    return [
      h('h1', { class: 'title', text: found.name ?? 'Songs' }),
      songs.length ? h('div', { class: 'list-actions' },
        h('button', { class: 'play-all', onclick: () => playFrom(songs, 0) }, icon('play'), 'Play'),
        h('button', { class: 'play-all second', onclick: () => playFrom(shuffled(songs), 0) }, icon('shuffle'), 'Shuffle')) : h('p', { class: 'empty', text: 'Nothing here yet.' }),
      h('div', { class: 'upnext' }, rows(songs, 50)),
    ];
  }, body);
  return [backButton(), body];
}
