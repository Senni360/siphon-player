// Where the page is, and the browser's back step.
//
// The view-drawing call lives in `hooks` rather than an import, so screens can
// navigate without importing the shell that imports them.

import { h, icon } from './dom.js';
import { hooks, player } from './state.js';

export const trail = [];
export let current = { page: 'home' };

export function go(next, remember = true) {
  hooks.keepPlace?.();
  if (remember) trail.push(current);
  current = next;
  // A sheet closing right now gives back its own step first, then this page opens.
  settled(() => { history.pushState({ page: next }, ''); hooks.render?.(); });
}
/** A sheet over the page is a step back of its own: the gesture closes it first. */
let stepped = false;
let afterStep = null;
export function over() { history.pushState({ over: true }, ''); }
export function overClosed() {
  if (!history.state?.over) return;
  stepped = true;
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
  current = trail.pop() ?? { page: 'home' };
  hooks.render?.();
}
// On a computer's browser, Escape closes what's on top.
addEventListener('keydown', event => {
  if (event.key !== 'Escape') return;
  if (document.querySelector('.actions')) document.querySelector('.scrim')?.click();
  else if (player.sheet) player.sheet.close();
});
addEventListener('popstate', event => {
  // Closing a sheet by hand takes the step back it left behind.
  if (stepped) {
    stepped = false;
    const run = afterStep;
    afterStep = null;
    if (run) run();
    return;
  }
  if (document.querySelector('.actions')) { document.querySelector('.scrim')?.click(); return; }
  if (player.sheet) { player.sheet.close(); return; }
  hooks.keepPlace?.();
  current = trail.pop() ?? event.state?.page ?? { page: 'home' };
  hooks.render?.();
});

export function backButton() {
  return h('button', { class: 'back', onclick: back, 'aria-label': 'Back' }, icon('back'));
}

/** A shortcut's ?tab=... opens that tab, leaving a clean address behind. */
export function shortcutTab() {
  const tab = new URLSearchParams(location.search).get('tab');
  if (tab !== 'search' && tab !== 'library') return;
  current = { page: tab };
  history.replaceState(null, '', '/');
}
