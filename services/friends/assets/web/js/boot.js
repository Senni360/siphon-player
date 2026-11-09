// Starting the page: the door, the shell, and the first look at the computer.

import { STORE, keepSecret, keepStorage, rememberedSecret } from './auth.js';
import { toast } from './dom.js';
import { edgeLive } from './edge.js';
import { shortcutTab } from './navigation.js';
import { keepShell } from './offline.js';
import { listenComputer, pollState } from './player.js';
import { linkFromAddress, linkScreen, signedOutScreen } from './screens/link.js';
import { shell } from './shell.js';
import { spotifyReturn } from './spotify.js';
import { hooks, player } from './state.js';

let polling = false;
let arrival = null;

function start({ signedOut = false } = {}) {
  keepShell();
  // The browser may have emptied its storage since last time (iOS does, for
  // sites gone quiet): the backup cookie puts the sign-in back.
  if (!localStorage.getItem(STORE.secret)) {
    const remembered = rememberedSecret();
    if (remembered) localStorage.setItem(STORE.secret, remembered);
  }
  if (localStorage.getItem(STORE.secret)) {
    keepSecret(localStorage.getItem(STORE.secret));  // the backup slides forward
    keepStorage();
    shell();
    if (!polling) { polling = true; pollState(); }
    listenComputer();
    edgeLive();
  } else {
    player.mini = null;
    if (signedOut) signedOutScreen();
    else linkScreen();
  }
}

hooks.signedOut = () => start({ signedOut: true });
hooks.restart = start;

export async function arrive() {
  if (location.pathname === '/callback') {
    try { await spotifyReturn(); } catch (error) { arrival = error.message; }
  }
  if (await linkFromAddress()) return;
  shortcutTab();
  start();
  if (arrival) { toast(arrival); arrival = null; }
}
