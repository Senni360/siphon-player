// Rows, cards, shelves, sections, and what a song's own menu does.

import { art, count, h, icon, onHold, safeImage, SOURCE_NAMES, sourceOf, toast } from './dom.js';
import { go } from './navigation.js';
import { shuffled } from './order.js';
import { ownsQueue, playNext } from './player.js';
import { api } from './service.js';
import { choice, openPanel } from './sheet.js';
import { hooks, player } from './state.js';
import { matching, plain } from './words.js';
import { act, tell } from './remote.js';

export { shuffled };

/** The song a sheet is about, at its top. */
export const about = song => h('div', { class: 'about' }, art(song.image),
  h('div', {}, h('b', { text: song.name, class: sourceOf(song.uri) }), h('span', { text: (song.artists ?? []).join(', ') })));

/**
 * Everything one song offers, in the same order wherever it is opened from.
 * `extra` adds choices that only make sense in one place (the queue's Remove).
 */
export function songActions(song, extra = []) {
  const run = work => () => close(work);
  const close = openPanel({
    label: `More for ${song.name}`,
    content: [
      about(song),
      choice('next_up', 'Play next', run(() => playNext(song).then(() => toast('Playing next'), error => toast(error.message)))),
      ownsQueue() && choice('last', 'Add to queue', run(() => { hooks.hereActs.last({ song }); toast('Added to the queue'); })),
      choice('heart', 'Like', run(() => Promise.resolve(act('like', { uri: song.uri })).then(() => toast('Liked'), () => toast("That didn't go through.")))),
      song.link && choice('share', 'Share with the group', run(() => shareSong(song))),
      // The computer does the downloading, so this shows while one that can is there to ask.
      player.online && player.state?.can?.includes?.('download') && choice('download', 'Download on your computer', run(() => {
        toast('Downloading on your computer');
        Promise.resolve(tell('download', { uri: song.uri, name: song.name, artists: (song.artists ?? []).slice(0, 20), duration_ms: Math.round(song.duration_ms ?? 0), ...(song.link ? { link: song.link } : {}) }))
          .catch(() => toast("Couldn't reach Encore."));
      })),
      song.artist_id && choice('person', 'Go to artist', run(() => { hooks.closeSheet?.(); go({ page: 'artist', id: song.artist_id }); })),
      extra.map(([name, label, work, options]) => choice(name, label, run(work), options)),
    ],
  });
}

/** Shares a song with the group, with a note if there's something to say. */
export function shareSong(song) {
  const note = h('input', { type: 'text', placeholder: 'Say something (optional)', 'aria-label': 'Note', maxlength: 280, enterkeyhint: 'send' });
  const close = openPanel({
    label: 'Share with the group', className: 'share', form: true,
    onsubmit: event => {
      event.preventDefault();
      const said = note.value.trim();
      close();
      // Shown as sent straight away; a failure says so.
      toast('Shared with the group');
      api('/posts', 'POST', { url: song.link, source: sourceOf(song.uri), title: song.name, artists: song.artists.slice(0, 20), artwork_url: safeImage(song.image), duration_ms: song.duration_ms ?? 0, note: said })
        .then(() => hooks.forgetGroup?.(), error => toast(error.message));
    },
    content: [about(song), note, h('button', { class: 'primary', type: 'submit', text: 'Share with the group' })],
  });
}

/** The three small bars on the song that is playing. */
const bars = () => h('span', { class: 'bars', 'aria-hidden': 'true' }, h('i'), h('i'), h('i'));

export function row(song, list, index) {
  const state = hooks.shown?.();
  const playing = state?.song?.uri === song.uri;
  const by = (song.artists ?? []).join(', ');
  const plays = song.plays ? ` · ${count(song.plays)} plays` : '';
  const node = h('div', { class: `row${playing ? ' playing' : ''}${playing && !state.playing ? ' paused' : ''}`, 'data-uri': song.uri },
    h('span', { class: 'art-box' }, art(song.image), bars()),
    h('button', { class: 'words', onclick: () => hooks.playFrom(list, index) },
      h('span', { class: `name ${sourceOf(song.uri)}`, text: song.name }),
      h('span', { class: 'by', text: by + plays })),
    h('button', { class: 'more', 'aria-label': `More for ${song.name}`, onclick: () => songActions(song) }, icon('more')));
  onHold(node, () => songActions(song));
  return node;
}

/** A place rather than a song: a playlist, an album, someone's likes. */
export function place(title, image, by, onclick, mark) {
  const picture = mark ? h('div', { class: `art tile ${mark.source ?? ''}` }, icon(mark.icon)) : art(image);
  return h('div', { class: 'row place' }, picture,
    h('button', { class: 'words', onclick }, h('span', { class: 'name', text: title }), by && h('span', { class: 'by', text: by })),
    h('span', { class: 'chevron', 'aria-hidden': 'true' }, icon('forward')));
}

const PAGE = 50;
/**
 * A list's rows. A long list draws a page at a time and adds the next as the
 * end comes near, so two hundred songs open as fast as twenty.
 */
export function rows(list, limit, found) {
  const box = h('div', { class: 'rows' });
  // A found song plays in its place in the whole list it was found in.
  const whole = found?.list ?? list;
  const placeOf = at => (found ? found.at[at] : at);
  const short = limit !== undefined && limit < PAGE;
  let shown = 0;
  let watcher = null;
  const draw = upTo => {
    box.querySelector('.rows-end')?.remove();
    watcher?.disconnect();
    box.append(...list.slice(shown, upTo).map((song, at) => row(song, whole, placeOf(shown + at))));
    shown = Math.min(upTo, list.length);
    if (list.length <= shown) return;
    if (short) {
      box.append(h('button', { class: 'pill rows-end', text: `Show ${Math.min(20, list.length - shown)} more`, onclick: () => draw(shown + 20) }));
      return;
    }
    const end = h('div', { class: 'rows-end', 'aria-hidden': 'true' });
    box.append(end);
    watcher = new IntersectionObserver(seen => { if (seen.some(entry => entry.isIntersecting)) draw(shown + PAGE); }, { rootMargin: '600px 0px' });
    watcher.observe(end);
  };
  draw(limit ?? PAGE);
  return box;
}

/** From this many on, a list gets a field to find something in it. */
const FIND_FROM = 12;
/**
 * A long list under a field that narrows it to what is typed. `textOf` says
 * what an item is found by, and `draw` shows the ones found, with where each
 * sits in the whole list.
 */
export function findable(items, { label, textOf, draw }) {
  const show = (found, at) => [draw(found, at)].flat();
  const shown = h('div', {}, show(items, null));
  if (items.length < FIND_FROM) return shown;
  let texts = null;
  const input = h('input', { type: 'search', placeholder: label, 'aria-label': label, enterkeyhint: 'search', autocapitalize: 'none', autocorrect: 'off', spellcheck: 'false' });
  const narrow = () => {
    const typed = input.value.trim();
    clear.hidden = !input.value;
    if (!typed) { shown.replaceChildren(...show(items, null)); return; }
    texts ??= items.map(item => plain(textOf(item)));
    const at = matching(texts, typed);
    shown.replaceChildren(...(at.length ? show(at.map(index => items[index]), at)
      : [h('p', { class: 'none-found', text: `Nothing here matches "${typed}".` })]));
  };
  const clear = h('button', { class: 'clear', type: 'button', 'aria-label': 'Clear', hidden: true, onclick: () => { input.value = ''; narrow(); input.focus(); } }, icon('close'));
  input.addEventListener('input', narrow);
  return h('div', { class: 'findable' },
    h('form', { class: 'find', role: 'search', onsubmit: event => { event.preventDefault(); input.blur(); } }, icon('search'), input, clear), shown);
}
/** A list of songs that can be searched. A found song still plays within the whole list. */
export const findableSongs = (list, label = 'Find in this list') => findable(list, {
  label,
  textOf: song => `${song.name} ${(song.artists ?? []).join(' ')}`,
  draw: (songs, at) => rows(songs, PAGE, at ? { list, at } : undefined),
});

export const heading =(title, note) => [h('h2', { class: 'section', text: title }), note ? h('p', { class: 'note', text: note }) : null];

export function section(title, list, note, limit = 5) {
  if (!list?.length) return null;
  const from = {};
  for (const song of list) from[sourceOf(song.uri)] = (from[sourceOf(song.uri)] ?? 0) + 1;
  const told = note ?? Object.entries(from).map(([source, amount]) => `${amount} on ${SOURCE_NAMES[source]}`).join(' · ');
  return [heading(title, told), rows(list, limit)];
}

export function card(title, by, image, onclick, uri, round = false) {
  return h('button', { class: `card${round ? ' circle' : ''}`, onclick }, art(image, 'card-art'),
    h('span', { class: `name${uri ? ` ${sourceOf(uri)}` : ''}`, text: title }), by && h('span', { class: 'by', text: by }));
}

const KIND_NAMES = { album: 'Album', ep: 'EP', single: 'Single' };

/**
 * A new release. An album or EP carries its record behind the cover, its own
 * cover on the record's label, as Home's large card does on the computer; a
 * single is the cover alone. Each says what it is under its name.
 */
export function releaseCard(release) {
  const kind = KIND_NAMES[release.kind] ?? 'Album';
  const by = [kind, (release.artists ?? []).join(', ')].filter(Boolean).join(' · ');
  const frame = h('span', { class: 'release-art' });
  if (release.kind !== 'single') {
    const record = h('span', { class: 'record', 'aria-hidden': 'true' });
    const cover = safeImage(release.image);
    // Set from script: the page's policy refuses inline style attributes.
    if (cover) record.style.setProperty('--cover', `url("${cover}")`);
    frame.append(record);
  }
  frame.append(art(release.image, 'card-art'));
  const open = () => {
    if (release.song) return hooks.playFrom([release.song], 0);
    if (release.id) return go({ page: 'list', kind: 'album', id: release.id });
    if (String(release.uri ?? '').startsWith('soundcloud:')) return go({ page: 'list', kind: 'source', uri: release.uri });
    return undefined;
  };
  return h('button', { class: `card release${release.kind === 'single' ? ' single' : ''}`, onclick: open },
    frame, h('span', { class: `name ${sourceOf(release.uri)}`, text: release.name }), h('span', { class: 'by', text: by }));
}

/** Cards side by side, swiped along. Each snaps to the edge it is pulled to. */
export const shelf = cards => h('div', { class: 'shelf' }, cards);
