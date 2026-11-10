// A source's own page: someone on SoundCloud or YouTube.

import { empty, h, SOURCE_NAMES, sourceOf } from '../dom.js';
import { load } from '../load.js';
import { current, topBar } from '../navigation.js';
import { ask } from '../remote.js';
import { rows } from '../rows.js';
import { listHead } from './list.js';

export function sourceScreen() {
  const body = h('div');
  const bar = topBar();
  const { uri } = current;
  load(async () => {
    const found = await ask('source', { uri });
    const songs = found.songs ?? [];
    const name = found.name ?? 'Songs';
    bar.setTitle(name);
    return [
      listHead(name, songs, { image: found.image, note: SOURCE_NAMES[sourceOf(uri)], round: !!found.image }),
      songs.length ? h('div', { class: 'upnext' }, rows(songs, 50)) : empty('Nothing here yet.'),
    ];
  }, body, 'list');
  return [bar, body];
}
