// Home: picks, cards, and the once-only install tip.

import { art, h, sourceOf } from '../dom.js';
import { installTip } from '../install.js';
import { load } from '../load.js';
import { go } from '../navigation.js';
import { playFrom } from '../player.js';
import { ask } from '../remote.js';
import { card, rows, section } from '../rows.js';

export function homeScreen() {
  const body = h('div');
  load(async () => {
    const found = await ask('home');
    const picks = found.picks ?? [];
    const [first, ...rest] = picks;
    if (!first) return h('p', { class: 'empty', text: 'Play some music on Encore and your picks show up here.' });
    return [
      h('button', { class: 'lead', onclick: () => playFrom(picks, 0) }, art(first.image, ''), h('span', { class: 'words' },
        h('span', { class: `name ${sourceOf(first.uri)}`, text: first.name }), h('span', { class: 'by', text: first.artists.join(', ') }))),
      h('h2', { class: 'section', text: 'For you' }),
      h('div', { class: 'cards' }, rest.slice(0, 8).map((song, index) => card(song.name, song.artists.join(', '), song.image, () => playFrom(picks, index + 1), song.uri))),
      found.recent?.length ? [h('h2', { class: 'section', text: 'Jump back in' }), rows(found.recent, 5)] : null,
      found.made_for_you?.length ? [h('h2', { class: 'section', text: 'Made for you' }), h('div', { class: 'cards' }, found.made_for_you.slice(0, 6).map(list => card(list.name, null, list.image, () => go({ page: 'list', kind: 'playlist', id: list.id }))))] : null,
      section('New on SoundCloud from artists you follow', found.news, ''),
      section('New from who you follow', found.following),
    ];
  }, body);
  return [installTip(), h('h1', { class: 'title', text: greeting() }), body];
}
function greeting() {
  const hour = new Date().getHours();
  return hour < 5 ? 'Good night' : hour < 12 ? 'Good morning' : hour < 18 ? 'Good afternoon' : 'Good evening';
}
