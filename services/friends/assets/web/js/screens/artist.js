// An artist, across every source.

import { art, bones, count, failed, h, icon, safeImage, SOURCE_NAMES, toast } from '../dom.js';
import { SignedOut } from '../errors.js';
import { current, go, topBar } from '../navigation.js';
import { playFrom } from '../player.js';
import { act, ask, tell } from '../remote.js';
import { card, heading, section, shelf } from '../rows.js';
import { hooks, player } from '../state.js';

/** Follow or unfollow, shown at once and put back if it doesn't go through. */
function followButton(found) {
  const show = () => {
    button.textContent = found.following ? 'Following' : 'Follow';
    button.setAttribute('aria-pressed', String(found.following));
  };
  const button = h('button', { class: 'play-all second', onclick: async () => {
    found.following = !found.following;
    show();
    try {
      await act('like', { uri: found.uri });
    } catch {
      found.following = !found.following;
      show();
      toast("That didn't go through.");
    }
  } });
  show();
  return button;
}

/** "Out in 9 days · Nov 20", "Out tomorrow", "Out today"; null once it is out. */
export function releaseWords(release, now = Date.now()) {
  const left = release * 1000 - now;
  if (!Number.isFinite(left) || left <= 0) return null;
  const days = Math.ceil(left / 86400000);
  if (left < 43200000) return 'Out today';
  if (days <= 1) return 'Out tomorrow';
  const date = new Date(release * 1000).toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
  return `Out in ${days} days · ${date}`;
}

/**
 * The artist's upcoming release with Pre-save, as on the computer. The press
 * shows at once; the computer makes it true, and a refusal puts it back.
 */
function upcomingCard(release) {
  const words = releaseWords(release.release);
  const can = player.online && player.state?.can?.includes?.('presave') && release.album_uri && release.saved != null;
  const button = h('button', { class: 'presave', type: 'button' });
  const show = () => {
    const saved = release.saved === true;
    button.replaceChildren(saved ? icon('check') : icon('plus'), h('span', { text: saved ? 'Pre-saved' : 'Pre-save' }));
    button.classList.toggle('done', saved);
    button.setAttribute('aria-pressed', String(saved));
    button.disabled = !can;
  };
  button.addEventListener('click', async () => {
    const was = release.saved === true;
    release.saved = !was;
    show();
    toast(was ? 'Pre-save removed' : 'Pre-saved on Spotify');
    try {
      await tell('presave', { uri: release.album_uri, saved: !was });
    } catch {
      release.saved = was;
      show();
      toast("That didn't go through.");
    }
  });
  show();
  const record = h('span', { class: 'record', 'aria-hidden': 'true' });
  const cover = safeImage(release.image);
  if (cover) record.style.setProperty('--cover', `url("${cover}")`);
  return h('div', { class: 'upcoming' },
    h('span', { class: 'release-art' }, record, art(release.image, 'card-art')),
    h('div', { class: 'upcoming-words' },
      words && h('small', { text: words }),
      h('b', { text: release.name }),
      h('span', { text: release.kind === 'single' ? 'Single' : release.kind === 'ep' ? 'EP' : 'Album' }),
      can ? button : h('p', { class: 'note', text: 'Pre-save works while Encore is open on your computer.' })));
}

export function artistScreen() {
  const id = current.id;
  const body = h('div');
  const bar = topBar();
  bar.classList.add('over');
  body.append(bones(8));
  let tries = 0;
  const draw = found => {
    const everything = [...(found.popular ?? []), ...(found.not_on_spotify ?? [])];
    bar.setTitle(found.name);
    body.replaceChildren(...[
      h('div', { class: 'hero' }, safeImage(found.image) && h('img', { src: found.image, alt: '' }),
        h('div', { class: 'hero-words' }, h('h1', { class: 'title', text: found.name }), found.followers != null && h('p', { class: 'note', text: [
          `${count(found.followers)} ${found.accounts?.some(account => account.followers) ? 'on Spotify' : 'followers'}`,
          ...(found.accounts ?? []).filter(account => account.followers).map(account => `${count(account.followers)} on ${SOURCE_NAMES[account.source]}`),
        ].join(' · ') }),
          h('div', { class: 'list-actions' },
            everything.length > 0 && h('button', { class: 'play-all', onclick: () => playFrom(everything, 0) }, icon('play'), 'Play'),
            found.following != null && followButton(found)))),
      found.accounts?.length ? h('div', { class: 'chips' }, found.accounts.map(account => h('button', { class: 'chip', onclick: () => go({ page: 'list', kind: 'source', uri: account.uri }) },
        art(account.image, ''), h('span', {}, account.name, h('small', { class: account.source, text: SOURCE_NAMES[account.source] }))))) : null,
      found.upcoming?.filter(release => release.release == null || releaseWords(release.release)).slice(0, 1)
        .map(release => [heading('Upcoming'), upcomingCard(release)]),
      section('Popular', found.popular, ''),
      section('Not on Spotify', found.not_on_spotify),
      found.sets?.length ? [heading('Sets and mixtapes', 'On SoundCloud'), shelf(found.sets.slice(0, 10).map(set => card(set.name, set.songs ? `${set.songs} songs` : null, set.image, () => go({ page: 'list', kind: 'source', uri: set.uri }))))] : null,
      found.also_as?.length ? section(`Also posts as ${found.also_as.join(', ')}`, found.alternate) : null,
      found.albums?.length ? [heading('Discography'), shelf(found.albums.slice(0, 12).map(album => card(album.name, album.year, album.image, () => go({ page: 'list', kind: 'album', id: album.id }))))] : null,
      section(`Produced by ${found.name}`, found.produced),
      section('Snippets and unreleased', found.unreleased),
      section('Edits and remixes', found.edits),
      found.looking ? h('p', { class: 'note looking', role: 'status', text: 'Still looking on SoundCloud and YouTube…' }) : null,
    ].flat(Infinity).filter(Boolean));
    hooks.markPlaying?.();
  };
  const read = async () => {
    try {
      const found = await ask('artist', { target: id });
      // Left for another page meanwhile: this one is drawn when it is come back to.
      draw(found);
      // What's on the other sources arrives while the page is open.
      if (found.looking && tries++ < 12) setTimeout(read, 3000);
    } catch (error) {
      if (error instanceof SignedOut) hooks.signedOut?.();
      else if (!body.querySelector('.hero')) body.replaceChildren(failed(error, () => { body.replaceChildren(bones(8)); read(); }));
    }
  };
  read();
  return [bar, body];
}
