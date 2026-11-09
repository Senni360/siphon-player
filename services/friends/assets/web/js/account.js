// Unlinking this phone, from the Library.

import { STORE, forgetSecret } from './auth.js';
import { api } from './service.js';
import { hooks } from './state.js';
import { stopHere } from './player.js';

/** Unlinks this phone from the account, then forgets it here even without a network. */
export async function unlink() {
  if (!confirm('Unlink this phone? Encore here signs out, and a new code links it again.')) return;
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
