// Where the page is, and the browser's back step.
//
// The view-drawing call lives in `hooks` rather than an import, so screens can
// navigate without importing the shell that imports them.

import { h, icon } from './dom.js';
import { hooks, player } from './state.js';

export const trail = [];
export let current = { page: 'home' };
/** Which way the last step went, so the page can arrive from that side. */
export let moved = 'tab';

export function go(next, remember = true, how) {
  hooks.keepPlace?.();
  if (remember) trail.push(current);
  moved = how ?? (remember ? 'forward' : 'tab');
  current = next;
  // A sheet closing right now gives back its own step first, then this page opens.
  settled(() => { history.pushState({ page: next }, ''); hooks.render?.(); });
}
/** A sheet over the page is a step back of its own: the gesture closes it first. */
let stepped = 0;
let afterStep = null;
export function over() { history.pushState({ over: true }, ''); }
export function overClosed() {
  if (!history.state?.over) return;
  // Two sheets closing in one breath are two steps, and both are counted.
  stepped++;
  history.back();
}
/** Runs now, or once the step a closing sheet gave back has landed. */
function settled(run) {
  if (stepped) afterStep = run;
  else run();
}
export function back() {
  hooks.keepPlace?.();
  // The gesture and this button take the same way back, so both agree.
  if (history.state) { history.back(); return; }
  moved = 'back';
  current = trail.pop() ?? { page: 'home' };
  hooks.render?.();
}
/** The sheet of choices on top, unless it is already on its way out. */
const topPanel = () => document.querySelector('.scrim:not(.leaving)');
// On a computer's browser, Escape closes what's on top.
addEventListener('keydown', event => {
  if (event.key !== 'Escape') return;
  if (topPanel()) topPanel().click();
  else if (player.sheet) player.sheet.close();
});
addEventListener('popstate', event => {
  // Closing a sheet by hand takes the step back it left behind.
  if (stepped > 0) {
    stepped--;
    if (stepped > 0) return;
    const run = afterStep;
    afterStep = null;
    if (run) run();
    return;
  }
  if (topPanel()) { topPanel().click(); return; }
  if (player.sheet) { player.sheet.close(); return; }
  hooks.keepPlace?.();
  moved = 'back';
  current = trail.pop() ?? event.state?.page ?? { page: 'home' };
  hooks.render?.();
});

export function backButton() {
  return h('button', { class: 'back', onclick: back, 'aria-label': 'Back' }, icon('back'));
}

/**
 * The bar a pushed page keeps at its top: the way back, always in reach (an
 * installed app has no back gesture of its own), and the page's name once its
 * heading has scrolled away. `tools` sit at its right.
 */
export function topBar(title, ...tools) {
  const name = h('span', { class: 'bar-title', 'aria-hidden': 'true', text: title ?? '' });
  const bar = h('div', { class: 'bar' }, backButton(), name, h('span', { class: 'bar-tools' }, tools));
  bar.setTitle = text => { name.textContent = text ?? ''; };
  return bar;
}

/** A shortcut's ?tab=... opens that tab, leaving a clean address behind. */
export function shortcutTab() {
  const tab = new URLSearchParams(location.search).get('tab');
  if (tab !== 'search' && tab !== 'library') return;
  current = { page: tab };
  history.replaceState(null, '', '/');
}
