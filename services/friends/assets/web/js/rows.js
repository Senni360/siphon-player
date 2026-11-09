// Rows, cards, sections, and what a song's own menu does.

import { art, count, h, icon, safeImage, SOURCE_NAMES, sourceOf, toast } from './dom.js';
import { go, over, overClosed } from './navigation.js';
import { act } from './remote.js';
import { api } from './service.js';
import { hooks } from './state.js';

export function songActions(song) {
  const scrim = h('div', { class: 'scrim', onclick: () => close() });
  const close = () => { scrim.remove(); panel.remove(); overClosed(); };
  const choose = (label, run) => h('button', { onclick: () => { close(); run(); } , text: label });
  const panel = h('div', { class: 'actions', role: 'menu' },
    h('div', { class: 'about' }, h('b', { text: song.name, class: sourceOf(song.uri) }), h('span', { text: song.artists.join(', ') })),
    choose('Play next', () => act('queue', { uri: song.uri }).then(() => toast('Added to the queue'))),
    choose('Like', () => act('like', { uri: song.uri }).then(() => toast('Done'))),
    song.link && choose('Share with the group', () => shareSong(song)),
    song.artist_id && choose('Go to artist', () => go({ page: 'artist', id: song.artist_id })),
    choose('Cancel', () => {}));
  document.body.append(scrim, panel);
  over();
}
/** Shares a song with the group, with a note if there's something to say. */
export function shareSong(song) {
  const note = h('input', { type: 'text', placeholder: 'Say something (optional)', 'aria-label': 'Note', maxlength: 280, enterkeyhint: 'send' });
  const scrim = h('div', { class: 'scrim', onclick: () => close() });
  const close = () => { scrim.remove(); panel.remove(); overClosed(); };
  const panel = h('form', { class: 'actions share', onsubmit: async event => {
    event.preventDefault();
    close();
    try {
      await api('/posts', 'POST', { url: song.link, source: sourceOf(song.uri), title: song.name, artists: song.artists.slice(0, 20), artwork_url: safeImage(song.image), duration_ms: song.duration_ms ?? 0, note: note.value.trim() });
      hooks.forgetGroup?.();
      toast('Shared with the group');
    } catch (error) { toast(error.message); }
  } },
    h('div', { class: 'about' }, h('b', { text: song.name, class: sourceOf(song.uri) }), h('span', { text: song.artists.join(', ') })),
    note,
    h('button', { class: 'send-share', type: 'submit', text: 'Share with the group' }));
  document.body.append(scrim, panel);
  over();
  note.focus();
}

export function row(song, list, index) {
  const playing = hooks.shown?.()?.song?.uri === song.uri;
  const by = song.artists.join(', ');
  const plays = song.plays ? ` · ${count(song.plays)} plays` : '';
  return h('div', { class: `row${playing ? ' playing' : ''}`, 'data-uri': song.uri },
    art(song.image),
    h('button', { class: 'words', onclick: () => hooks.playFrom(list, index) },
      h('span', { class: `name ${sourceOf(song.uri)}`, text: song.name }),
      h('span', { class: 'by', text: by + plays })),
    h('button', { class: 'more', 'aria-label': `More for ${song.name}`, onclick: () => songActions(song) }, icon('more')));
}
export function rows(list, limit) {
  const box = h('div');
  const draw = shown => {
    box.replaceChildren(...list.slice(0, shown).map((song, index) => row(song, list, index)));
    if (list.length > shown) box.append(h('button', { class: 'see-more', text: 'See more', onclick: () => draw(shown + 20) }));
  };
  draw(limit ?? list.length);
  return box;
}
export function section(title, list, note, limit = 5) {
  if (!list?.length) return null;
  const from = {};
  for (const song of list) from[sourceOf(song.uri)] = (from[sourceOf(song.uri)] ?? 0) + 1;
  const told = note ?? Object.entries(from).map(([source, amount]) => `${amount} on ${SOURCE_NAMES[source]}`).join(' · ');
  return [h('h2', { class: 'section', text: title }), h('p', { class: 'note', text: told }), rows(list, limit)];
}
export function card(title, by, image, onclick, uri) {
  return h('button', { class: 'card', onclick }, art(image, ''), h('span', { class: `name${uri ? ` ${sourceOf(uri)}` : ''}`, text: title }), by && h('span', { class: 'by', text: by }));
}
export function shuffled(list) {
  const out = [...list];
  for (let at = out.length - 1; at > 0; at--) {
    const other = Math.floor(Math.random() * (at + 1));
    [out[at], out[other]] = [out[other], out[at]];
  }
  return out;
}
