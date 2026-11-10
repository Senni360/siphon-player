// Unlinking this phone, from Settings.

import { STORE, forgetSecret } from './auth.js';
import { h } from './dom.js';
import { api } from './service.js';
import { openPanel } from './sheet.js';
import { hooks } from './state.js';
import { stopHere } from './player.js';

/** Asks first, in the page's own words, then unlinks. */
export function unlink() {
  const close = openPanel({
    label: 'Unlink this phone',
    content: [
      h('div', { class: 'about plain' }, h('div', {}, h('b', { text: 'Unlink this phone?' }),
        h('span', { text: 'Encore signs out here and stops playing. Your computer and other phones stay as they are, and a new code links this one again.' }))),
      h('button', { class: 'primary danger', type: 'button', text: 'Unlink this phone', onclick: () => close(forget) }),
      h('button', { class: 'quiet wide', type: 'button', text: 'Cancel', onclick: () => close() }),
    ],
  });
}

/** Unlinks this phone from the account, then forgets it here even without a network. */
async function forget() {
  try {
    const account = await api('/account');
    const mine = (account.devices ?? []).find(device => device.current);
    if (mine) await api(`/account/devices/${encodeURIComponent(mine.id)}`, 'DELETE');
  } catch { /* the phone is forgotten here either way */ }
  stopHere();
  for (const key of Object.values(STORE)) localStorage.removeItem(key);
  forgetSecret();
  hooks.restart?.();
}
