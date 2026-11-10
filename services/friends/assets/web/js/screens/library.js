// Library: what the listener has, and the way to Settings.

import { h, icon, plural } from '../dom.js';
import { load } from '../load.js';
import { go } from '../navigation.js';
import { ask } from '../remote.js';
import { heading, place } from '../rows.js';

export function libraryScreen() {
  const body = h('div');
  load(async () => {
    const found = await ask('library');
    const lists = found.playlists ?? [];
    return [
      place('Liked Songs', null, 'Spotify', () => go({ page: 'list', kind: 'liked' }), { icon: 'heart', source: 'spotify' }),
      !found.direct && place('SoundCloud likes', null, 'SoundCloud', () => go({ page: 'list', kind: 'soundcloud_likes' }), { icon: 'heart', source: 'soundcloud' }),
      !found.direct && place('YouTube likes', null, 'YouTube', () => go({ page: 'list', kind: 'youtube_likes' }), { icon: 'heart', source: 'youtube' }),
      found.mixed?.length ? [heading('Mixed playlists', 'Spotify, SoundCloud and YouTube in one list'),
        found.mixed.map(list => place(list.name, list.image, list.count != null ? plural(list.count, 'song') : null, () => go({ page: 'list', kind: 'mixed', id: list.id })))] : null,
      heading('Playlists'),
      lists.length ? lists.map(list => place(list.name, list.image, list.count != null ? plural(list.count, 'song') : null, () => go({ page: 'list', kind: 'playlist', id: list.id, uri: list.uri })))
        : h('p', { class: 'note', text: 'Playlists you make or follow show up here.' }),
    ];
  }, body);
  return [
    h('div', { class: 'title-row' }, h('h1', { class: 'title', text: 'Library' }),
      h('button', { class: 'icon-button', 'aria-label': 'Settings', onclick: () => go({ page: 'settings' }) }, icon('sliders'))),
    body,
  ];
}
