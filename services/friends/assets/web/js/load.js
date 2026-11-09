// Filling a screen, even if the page has been left meanwhile: going back finds
// it done.

import { bones, failed } from './dom.js';
import { SignedOut } from './errors.js';
import { hooks } from './state.js';

export async function load(fill, into) {
  const mine = {};
  into.loading = mine;
  const run = async () => {
    into.replaceChildren(bones(6));
    try {
      const content = await fill();
      if (into.loading === mine) into.replaceChildren(...[content].flat(Infinity).filter(Boolean));
    } catch (error) {
      if (error instanceof SignedOut) hooks.signedOut?.();
      else if (into.loading === mine) into.replaceChildren(failed(error, run));
    }
  };
  await run();
}
