// A playlist someone shared with the group. Each song plays by its own link.

import { art, h } from '../dom.js';
import { load } from '../load.js';
import { backButton, current } from '../navigation.js';
import { playShared } from '../post.js';
import { api } from '../service.js';

export function sharedScreen() {
  const body = h('div');
  const { id } = current;
  load(async () => {
    const found = await api(`/playlists/${encodeURIComponent(id)}`);
    const items = found.items ?? [];
    return [
      h('h1', { class: 'title', text: found.name }),
      h('p', { class: 'note', text: `${items.length} songs` }),
      items.map(item => h('div', { class: 'row' }, art(item.artwork_url), h('button', { class: 'words', onclick: () => playShared(item.url) },
        h('span', { class: `name ${item.source}`, text: item.title }), h('span', { class: 'by', text: (item.artists ?? []).join(', ') })))),
    ];
  }, body);
  return [backButton(), body];
}
