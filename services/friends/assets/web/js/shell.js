// The frame around every screen: the tabs, the mini player, and the back step.

import { app, h, icon } from './dom.js';
import { current, go, trail } from './navigation.js';
import { hooks, openedAt, player, WAKE_MS } from './state.js';
import { hasSpotify } from './spotify.js';
import { buildMini, markPlaying, refreshPlayer, restoreHere } from './player.js';
import { homeScreen } from './screens/home.js';
import { searchScreen } from './screens/search.js';
import { libraryScreen } from './screens/library.js';
import { groupScreen } from './screens/group.js';
import { listScreen } from './screens/list.js';
import { artistScreen } from './screens/artist.js';
import { sourceScreen } from './screens/source.js';
import { sharedScreen } from './screens/shared.js';

export const TABS = [['home', 'Home', homeScreen], ['search', 'Search', searchScreen], ['library', 'Library', libraryScreen], ['group', 'Group', groupScreen]];
export const SCREENS = { list: listScreen, artist: artistScreen, source: sourceScreen, shared: sharedScreen };

/** A tab's page shows as it was left for this long, then reads again. */
const TAB_KEEPS_MS = 3 * 60 * 1000;
const tabViews = new Map();
let screen = null;
let bannerSlot = null;
let drawn = 0;
let shownView = null;
let tabOf = 'home';

export function keepPlace() {
  if (shownView) shownView.scroll = screen.scrollTop;
}

export function renderBanner() {
  if (!player.online && Date.now() - openedAt < WAKE_MS) {
    bannerSlot?.replaceChildren(h('div', { class: 'banner' }, h('div', {}, h('b', { text: 'Connecting to your computer…' }), 'This can take a few seconds.')));
    setTimeout(renderBanner, WAKE_MS);
    return;
  }
  const telling = hasSpotify()
    ? 'Showing your Spotify, played on your Spotify devices. Open it for SoundCloud, YouTube and everything else.'
    : 'Open it to browse your library and play. Your group works from here either way.';
  bannerSlot?.replaceChildren(...(player.online ? [] : [h('div', { class: 'banner' }, h('span', { class: 'dot' }),
    h('div', {}, h('b', { text: "Encore isn't open on your computer" }), telling))]));
}

export function render(fresh = false) {
  drawn++;
  const tab = TABS.find(([page]) => page === current.page);
  const holder = tab ? tabViews.get(current.page) : current.view;
  let view = !fresh && holder && (!tab || Date.now() - holder.at < TAB_KEEPS_MS) ? holder : null;
  if (!view) {
    const content = (tab ? tab[2] : current.kind === 'source' ? sourceScreen : SCREENS[current.page])();
    view = { node: h('div', { class: 'page' }, h('div', { class: 'banner-slot' }), ...[content].flat(Infinity).filter(Boolean)), scroll: 0, at: Date.now() };
    if (tab) tabViews.set(current.page, view);
    else current.view = view;
  }
  shownView = view;
  screen.replaceChildren(view.node);
  bannerSlot = view.node.querySelector('.banner-slot');
  renderBanner();
  screen.scrollTop = view.scroll;
  markPlaying();
  document.querySelectorAll('nav.tabs button').forEach(button => {
    if (button.dataset.page === (tab ? current.page : tabOf)) button.setAttribute('aria-current', 'page');
    else button.removeAttribute('aria-current');
  });
  if (tab) tabOf = current.page;
}

export function shell() {
  screen = h('main');
  player.mini = buildMini();
  const tabs = h('nav', { class: 'tabs', 'aria-label': 'Sections' }, TABS.map(([page, label]) => h('button', { 'data-page': page, onclick: () => {
    // Pressed again it goes to the top, and pressed at the top it reads again.
    if (current.page === page) {
      if (screen.scrollTop > 0) screen.scrollTo({ top: 0, behavior: 'smooth' });
      else render(true);
      return;
    }
    trail.length = 0;
    go({ page });
  } }, icon(page), label)));
  app.replaceChildren(screen, player.mini.node, tabs);
  render();
  restoreHere();
  refreshPlayer();
}

hooks.render = render;
hooks.keepPlace = keepPlace;
hooks.renderBanner = renderBanner;
hooks.reloadViews = () => { tabViews.clear(); render(true); };
hooks.forgetGroup = () => tabViews.delete('group');
