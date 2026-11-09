// Library: what the listener has, and the way to Spotify and unlink.

import { unlink } from '../account.js';
import { art, h } from '../dom.js';
import { load } from '../load.js';
import { go } from '../navigation.js';
import { ask } from '../remote.js';
import { hasSpotify } from '../spotify.js';
import { spotifyPanel } from '../spotify-panel.js';

export function libraryScreen() {
  const body = h('div');
  const place = (title, image, by, onclick) => h('div', { class: 'row' }, art(image), h('button', { class: 'words', onclick }, h('span', { class: 'name', text: title }), by && h('span', { class: 'by', text: by })));
  load(async () => {
    const found = await ask('library');
    return [
      place('Liked Songs', null, 'Spotify', () => go({ page: 'list', kind: 'liked' })),
      !found.direct && place('SoundCloud likes', null, 'SoundCloud', () => go({ page: 'list', kind: 'soundcloud_likes' })),
      !found.direct && place('YouTube likes', null, 'YouTube', () => go({ page: 'list', kind: 'youtube_likes' })),
      found.mixed?.length ? [h('h2', { class: 'section', text: 'Mixed playlists' }),
        found.mixed.map(list => place(list.name, list.image, `${list.count} songs · Spotify, SoundCloud and YouTube`, () => go({ page: 'list', kind: 'mixed', id: list.id })))] : null,
      h('h2', { class: 'section', text: 'Playlists' }),
      (found.playlists ?? []).map(list => place(list.name, list.image, null, () => go({ page: 'list', kind: 'playlist', id: list.id, uri: list.uri }))),
    ];
  }, body);
  const connect = !hasSpotify() && h('button', { class: 'quiet', text: 'Keep Spotify going with the computer off', onclick: () => spotifyPanel() });
  return [h('h1', { class: 'title', text: 'Library' }), body, h('div', { class: 'settings-link' }, connect, connect && h('br'), h('button', { class: 'quiet', text: 'Unlink this phone', onclick: unlink }))];
}
