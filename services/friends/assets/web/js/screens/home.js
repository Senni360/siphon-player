// Home: one pick up front, more beside it, and the once-only install tip.

import { art, empty, h, icon, sourceOf } from '../dom.js';
import { installTip } from '../install.js';
import { load } from '../load.js';
import { go } from '../navigation.js';
import { playFrom } from '../player.js';
import { ask } from '../remote.js';
import { card, heading, rows, section, shelf } from '../rows.js';

export function homeScreen() {
  const body = h('div');
  load(async () => {
    const found = await ask('home');
    const picks = found.picks ?? [];
    const [first, ...rest] = picks;
    if (!first) return empty('Play some music on Encore and your picks show up here.');
    return [
      h('button', { class: 'lead', onclick: () => playFrom(picks, 0) }, art(first.image, 'lead-art'),
        h('span', { class: 'lead-words' },
          h('small', { text: 'Picked for you' }),
          h('span', { class: `name ${sourceOf(first.uri)}`, text: first.name }),
          h('span', { class: 'by', text: first.artists.join(', ') })),
        h('span', { class: 'lead-play', 'aria-hidden': 'true' }, icon('play'))),
      rest.length ? [heading('For you'),
        shelf(rest.slice(0, 10).map((song, index) => card(song.name, song.artists.join(', '), song.image, () => playFrom(picks, index + 1), song.uri)))] : null,
      found.recent?.length ? [heading('Jump back in'), rows(found.recent, 5)] : null,
      found.made_for_you?.length ? [heading('Made for you'),
        shelf(found.made_for_you.slice(0, 10).map(list => card(list.name, list.count ? `${list.count} songs` : null, list.image, () => go({ page: 'list', kind: 'playlist', id: list.id }))))] : null,
      section('New on SoundCloud from artists you follow', found.news, ''),
      section('New from who you follow', found.following),
    ];
  }, body, 'home');
  return [installTip(), h('h1', { class: 'title', text: greeting() }), body];
}
function greeting() {
  const hour = new Date().getHours();
  return hour < 5 ? 'Good night' : hour < 12 ? 'Good morning' : hour < 18 ? 'Good afternoon' : 'Good evening';
}
