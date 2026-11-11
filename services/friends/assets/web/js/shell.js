// The frame around every screen: the tabs, the mini player, and the back step.

import { app, h, icon } from './dom.js';
import { edgeBrowsing } from './edge-library.js';
import { current, go, moved, trail } from './navigation.js';
import { hooks, openedAt, player, WAKE_MS } from './state.js';
import { hasSpotify } from './spotify.js';
import { markPlaying, refreshPlayer, restoreHere } from './player.js';
import { buildMini } from './playing.js';
import { homeScreen } from './screens/home.js';
import { searchScreen } from './screens/search.js';
import { libraryScreen } from './screens/library.js';
import { groupScreen } from './screens/group.js';
import { listScreen } from './screens/list.js';
import { artistScreen } from './screens/artist.js';
import { sourceScreen } from './screens/source.js';
import { sharedScreen } from './screens/shared.js';
import { settingsScreen } from './screens/settings.js';

export const TABS = [['home', 'Home', homeScreen], ['search', 'Search', searchScreen], ['library', 'Library', libraryScreen], ['group', 'Group', groupScreen]];
export const SCREENS = { list: listScreen, artist: artistScreen, source: sourceScreen, shared: sharedScreen, settings: settingsScreen };

/** A tab's page shows as it was left for this long, then reads again. */
const TAB_KEEPS_MS = 3 * 60 * 1000;
const tabViews = new Map();
let screen = null;
let tabsNode = null;
let bannerSlot = null;
let shownView = null;
let tabOf = 'home';

export function keepPlace() {
  if (shownView) shownView.scroll = screen.scrollTop;
}

/** A computer that is there says so within a moment. Only a longer wait is worth a word. */
const QUIET_MS = 1500;
let bannerTimer = null;

export function renderBanner() {
  clearTimeout(bannerTimer);
  const waited = Date.now() - openedAt;
  // With the computer off and the listener's server answering, everything on
  // the page works, so there is nothing to say.
  if (!player.online && edgeBrowsing()) {
    bannerSlot?.replaceChildren();
    return;
  }
  if (!player.online && waited < WAKE_MS) {
    const quiet = waited < QUIET_MS;
    bannerSlot?.replaceChildren(...(quiet ? [] : [h('div', { class: 'banner waking', role: 'status' }, h('span', { class: 'dot' }),
      h('div', {}, h('b', { text: 'Connecting to your computer…' }), 'This can take a few seconds.'))]));
    bannerTimer = setTimeout(renderBanner, (quiet ? QUIET_MS : WAKE_MS) - waited);
    return;
  }
  const telling = hasSpotify()
    ? 'Showing your Spotify, played on your Spotify devices. Open it for SoundCloud, YouTube and everything else.'
    : 'Open it to browse your library and play. Your group works from here either way.';
  bannerSlot?.replaceChildren(...(player.online ? [] : [h('div', { class: 'banner', role: 'status' }, h('span', { class: 'dot' }),
    h('div', {}, h('b', { text: "Encore isn't open on your computer" }), telling))]));
}

export function render(fresh = false) {
  const tab = TABS.find(([page]) => page === current.page);
  const holder = tab ? tabViews.get(current.page) : current.view;
  let view = !fresh && holder && (!tab || Date.now() - holder.at < TAB_KEEPS_MS) ? holder : null;
  if (!view) {
    const content = (tab ? tab[2] : current.kind === 'source' ? sourceScreen : SCREENS[current.page])();
    view = { node: h('div', { class: 'page' }, h('div', { class: 'banner-slot' }), ...[content].flat(Infinity).filter(Boolean)), scroll: 0, at: Date.now() };
    if (tab) tabViews.set(current.page, view);
    else current.view = view;
  }
  const same = shownView === view;
  shownView = view;
  // The page arrives from the side it was reached from: forward, back, or a tab's plain fade.
  view.node.className = `page${tab ? '' : ' pushed'}${same ? '' : ` enter-${moved}`}`;
  screen.replaceChildren(view.node);
  bannerSlot = view.node.querySelector('.banner-slot');
  // The banner sits under a pushed page's bar, where it does not cover the way back.
  const bar = view.node.querySelector(':scope > .bar');
  if (bar && bannerSlot.previousElementSibling !== bar) bar.after(bannerSlot);
  renderBanner();
  screen.scrollTop = view.scroll;
  watchBar();
  markPlaying();
  const shownTab = tab ? current.page : tabOf;
  tabsNode.querySelectorAll('button').forEach((button, at) => {
    if (button.dataset.page === shownTab) {
      button.setAttribute('aria-current', 'page');
      tabsNode.style.setProperty('--tab', String(at));
    } else button.removeAttribute('aria-current');
  });
  if (tab) tabOf = current.page;
}

/** A pushed page's bar takes its glass, and its name, once the heading has gone under it. */
let barFrame = 0;
function watchBar() {
  if (barFrame) return;
  barFrame = requestAnimationFrame(() => {
    barFrame = 0;
    const bar = shownView?.node.querySelector(':scope > .bar');
    if (!bar) return;
    const title = shownView.node.querySelector('h1.title');
    const under = bar.getBoundingClientRect().bottom;
    bar.classList.toggle('scrolled', title ? title.getBoundingClientRect().bottom < under + 4 : screen.scrollTop > 24);
  });
}

export function shell() {
  screen = h('main');
  screen.addEventListener('scroll', watchBar, { passive: true });
  player.mini = buildMini();
  tabsNode = h('nav', { class: 'tabs', 'aria-label': 'Sections' }, h('span', { class: 'tab-pill', 'aria-hidden': 'true' }),
    TABS.map(([page, label]) => h('button', { 'data-page': page, onclick: () => {
      // Pressed again it goes to the top, and pressed at the top it reads again.
      if (current.page === page) {
        if (screen.scrollTop > 0) screen.scrollTo({ top: 0, behavior: 'smooth' });
        else render(true);
        return;
      }
      trail.length = 0;
      go({ page }, true, 'tab');
    } }, icon(page), h('span', { text: label }))));
  app.replaceChildren(player.mini.glow, screen, h('div', { class: 'dock' }, player.mini.node, tabsNode));
  render();
  restoreHere();
  refreshPlayer();
}

hooks.render = render;
hooks.keepPlace = keepPlace;
hooks.renderBanner = renderBanner;
hooks.reloadViews = () => { tabViews.clear(); render(true); };
hooks.forgetGroup = () => tabViews.delete('group');
