// Linking this phone, and the door when its sign-in is gone.

import { STORE, keepSecret, phoneName } from '../auth.js';
import { app, h } from '../dom.js';
import { api } from '../service.js';
import { spotifyPanel } from '../spotify-panel.js';
import { hooks } from '../state.js';

const logo = () => h('img', { class: 'logo', src: '/icon.svg', alt: '', width: 56, height: 56 });
const door = (...content) => h('div', { class: 'door-page' }, ...content);

/** A code in a /link address's fragment: a fragment never reaches the server. */
function linkingCode() {
  if (location.pathname !== '/link') return null;
  let raw;
  try { raw = decodeURIComponent(location.hash.slice(1)); } catch { raw = location.hash.slice(1); }
  const code = raw.toUpperCase().replace(/[^A-Z0-9]/g, '');
  return code.length === 8 ? code : null;
}

/** Claims a code and keeps the device secret it answers with. */
export async function claimCode(code) {
  const linked = await api('/link/claim', 'POST', { code, device: phoneName() });
  keepSecret(linked.secret);
  localStorage.setItem(STORE.name, linked.display_name);
  if (linked.client_id) localStorage.setItem(STORE.client, linked.client_id);
  return linked;
}

/** Linking from an address, so nobody has to type eight letters on a phone. */
export async function linkFromAddress() {
  const code = linkingCode();
  if (!code) return false;
  app.replaceChildren(door(h('div', { class: 'door', role: 'status' }, logo(),
    h('h1', { text: 'Encore' }), h('p', { class: 'working', text: 'Linking this phone…' }))));
  try {
    await claimCode(code);
  } catch (error) {
    // Spent or wrong: take the code out of the address, and let them type another.
    history.replaceState(null, '', '/link');
    linkScreen(error.message);
    return true;
  }
  app.replaceChildren(door(h('div', { class: 'door', role: 'status' }, logo(),
    h('h1', { text: 'This phone is linked' }), h('p', { class: 'working', text: 'Opening your music…' }))));
  history.replaceState(null, '', '/');
  setTimeout(() => hooks.restart?.(), 700);
  return true;
}

/** A phone whose device secret the service no longer accepts. */
export function signedOutScreen() {
  app.replaceChildren(door(h('div', { class: 'door' }, logo(),
    h('h1', { text: 'This phone was signed out' }),
    h('p', { text: 'Encore on your computer removed this phone, or its sign-in was cleared. Your other devices are still signed in.' }),
    h('p', { class: 'small', text: 'To link it again, open Encore on your computer, go to Group, then People, and press Link a phone. Open the link it shows on this phone, or type the code below.' }),
    h('button', { class: 'primary', text: 'Link this phone again', onclick: () => linkScreen() }),
    h('button', { class: 'quiet', type: 'button', text: 'Sign in with Spotify instead', onclick: spotifyPanel }))));
}

export function linkScreen(problemText) {
  const problem = h('p', { class: 'problem', role: 'alert', hidden: !problemText, text: problemText ?? '' });
  const input = h('input', { class: 'code', 'aria-label': 'Code from your computer', placeholder: 'XXXX XXXX', maxlength: 9, autocapitalize: 'characters', autocorrect: 'off', autocomplete: 'one-time-code', spellcheck: 'false', enterkeyhint: 'go', required: true });
  const button = h('button', { class: 'primary', type: 'submit', text: 'Link this phone' });
  const form = h('form', { class: 'door', onsubmit: async event => {
    event.preventDefault();
    button.disabled = true;
    button.textContent = 'Linking…';
    problem.hidden = true;
    try {
      await claimCode(input.value);
      hooks.restart?.();
    } catch (error) {
      problem.textContent = error.message;
      problem.hidden = false;
      button.disabled = false;
      button.textContent = 'Link this phone';
    }
  } },
    logo(),
    h('h1', { text: 'Encore' }),
    h('p', { text: 'Your library, your group and your queue, on this phone.' }),
    h('ol', { class: 'steps' },
      h('li', { text: 'On your computer, open Encore and go to Group, then People.' }),
      h('li', { text: 'Press Link a phone.' }),
      h('li', { text: 'Open the link it shows on this phone, or type its code here.' })),
    input, problem, button,
    h('button', { class: 'quiet', type: 'button', text: 'Sign in with Spotify instead', onclick: spotifyPanel }));
  app.replaceChildren(door(form));
  input.focus();
}
