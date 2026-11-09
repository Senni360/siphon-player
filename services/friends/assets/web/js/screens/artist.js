// An artist, across every source.

import { art, bones, count, failed, h, icon, safeImage, SOURCE_NAMES, toast } from '../dom.js';
import { SignedOut } from '../errors.js';
import { backButton, current, go } from '../navigation.js';
import { playFrom } from '../player.js';
import { act, ask } from '../remote.js';
import { card, section } from '../rows.js';
import { hooks } from '../state.js';

/** Follow or unfollow, shown at once and put back if it doesn't go through. */
function followButton(found) {
  const button = h('button', { class: 'play-all second', 'aria-pressed': String(found.following), text: found.following ? 'Following' : 'Follow', onclick: async () => {
    found.following = !found.following;
    button.textContent = found.following ? 'Following' : 'Follow';
    button.setAttribute('aria-pressed', String(found.following));
    try {
      await act('like', { uri: found.uri });
    } catch {
      found.following = !found.following;
      button.textContent = found.following ? 'Following' : 'Follow';
      button.setAttribute('aria-pressed', String(found.following));
      toast("That didn't go through.");
    }
  } });
  return button;
}

export function artistScreen() {
  const id = current.id;
  const body = h('div');
  body.append(bones(8));
  let tries = 0;
  const draw = found => {
    const everything = [...(found.popular ?? []), ...(found.not_on_spotify ?? [])];
    body.replaceChildren(...[
      h('div', { class: 'hero' }, safeImage(found.image) && h('img', { src: found.image, alt: '' }), backButton(),
        h('div', {}, h('h1', { text: found.name }), found.followers != null && h('p', { text: [
          `${count(found.followers)} ${found.accounts?.some(account => account.followers) ? 'on Spotify' : 'followers'}`,
          ...(found.accounts ?? []).filter(account => account.followers).map(account => `${count(account.followers)} on ${SOURCE_NAMES[account.source]}`),
        ].join(' · ') }),
          h('div', { class: 'list-actions' },
            everything.length > 0 && h('button', { class: 'play-all', onclick: () => playFrom(everything, 0) }, icon('play'), 'Play everything'),
            found.following != null && followButton(found)))),
      found.accounts?.length ? h('div', { class: 'chips' }, found.accounts.map(account => h('button', { class: 'chip', onclick: () => go({ page: 'list', kind: 'source', uri: account.uri }) },
        art(account.image, ''), h('span', {}, account.name, h('small', { class: account.source, text: SOURCE_NAMES[account.source] }))))) : null,
      section('Popular', found.popular, ''),
      section('Not on Spotify', found.not_on_spotify),
      found.sets?.length ? [h('h2', { class: 'section', text: 'Sets and mixtapes' }), h('p', { class: 'note', text: 'On SoundCloud' }), h('div', { class: 'cards' }, found.sets.slice(0, 8).map(set => card(set.name, set.songs ? `${set.songs} songs` : null, set.image, () => go({ page: 'list', kind: 'source', uri: set.uri }))))] : null,
      found.also_as?.length ? section(`Also posts as ${found.also_as.join(', ')}`, found.alternate) : null,
      found.albums?.length ? [h('h2', { class: 'section', text: 'Discography' }), h('div', { class: 'cards' }, found.albums.slice(0, 8).map(album => card(album.name, album.year, album.image, () => go({ page: 'list', kind: 'album', id: album.id }))))] : null,
      section(`Produced by ${found.name}`, found.produced),
      section('Snippets and unreleased', found.unreleased),
      section('Edits and remixes', found.edits),
      found.looking ? h('p', { class: 'note', text: 'Still looking on SoundCloud and YouTube…' }) : null,
    ].flat(Infinity).filter(Boolean));
  };
  const read = async () => {
    try {
      const found = await ask('artist', { target: id });
      draw(found);
      // What's on the other sources arrives while the page is open.
      if (found.looking && tries++ < 12) setTimeout(read, 3000);
    } catch (error) {
      if (error instanceof SignedOut) hooks.signedOut?.();
      else if (!body.querySelector('.hero')) body.replaceChildren(backButton(), failed(error, () => { body.replaceChildren(bones(8)); read(); }));
    }
  };
  read();
  return [body];
}
