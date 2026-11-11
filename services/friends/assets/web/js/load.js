// Filling a screen, even if the page has been left meanwhile: going back finds
// it done.

import { bones, failed, h } from './dom.js';
import { SignedOut } from './errors.js';
import { hooks } from './state.js';

/** What stands in while a screen reads: the shape of what is coming, never a spinner. */
const WAITING = {
  rows: () => bones(7),
  list: () => [h('div', { class: 'bones list-head', 'aria-hidden': 'true' }, h('span', { class: 'bone list-cover' }), h('span', { class: 'bone long' }), h('span', { class: 'bone short' })), bones(6)],
  home: () => [h('div', { class: 'bones', 'aria-hidden': 'true' }, h('span', { class: 'bone lead' })), bones(5)],
};

export async function load(fill, into, shape = 'rows') {
  const mine = {};
  into.loading = mine;
  const run = async () => {
    into.setAttribute('aria-busy', 'true');
    into.replaceChildren(...[(WAITING[shape] ?? WAITING.rows)()].flat());
    try {
      const content = await fill();
      if (into.loading !== mine) return;
      into.replaceChildren(...[content].flat(Infinity).filter(Boolean));
      // The entrance is for arriving, once: a page come back to is simply there.
      into.classList.add('arrived');
      setTimeout(() => into.classList.remove('arrived'), 700);
      hooks.markPlaying?.();
      // Who answered may have changed what the banner should say.
      hooks.renderBanner?.();
    } catch (error) {
      if (error instanceof SignedOut) hooks.signedOut?.();
      else if (into.loading === mine) into.replaceChildren(failed(error, run));
    } finally {
      if (into.loading === mine) into.removeAttribute('aria-busy');
    }
  };
  await run();
}
