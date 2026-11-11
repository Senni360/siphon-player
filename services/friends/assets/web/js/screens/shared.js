// A playlist someone shared with the group. Each song plays by its own link.

import { art, empty, h, plural } from '../dom.js';
import { load } from '../load.js';
import { current, topBar } from '../navigation.js';
import { playShared } from '../post.js';
import { api } from '../service.js';

export function sharedScreen() {
  const body = h('div');
  const bar = topBar(current.name);
  const { id } = current;
  load(async () => {
    const found = await api(`/playlists/${encodeURIComponent(id)}`);
    const items = found.items ?? [];
    bar.setTitle(found.name);
    return [
      h('header', { class: 'list-head plain' }, h('h1', { class: 'title', text: found.name }),
        h('p', { class: 'note', text: `${plural(items.length, 'song')} · shared with the group` })),
      items.length ? items.map(item => h('div', { class: 'row' }, art(item.artwork_url), h('button', { class: 'words', onclick: () => playShared(item.url) },
        h('span', { class: `name ${item.source}`, text: item.title }), h('span', { class: 'by', text: (item.artists ?? []).join(', ') }))))
        : empty('Nothing in this playlist yet.'),
    ];
  }, body, 'list');
  return [bar, body];
}
