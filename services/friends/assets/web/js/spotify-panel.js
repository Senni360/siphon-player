// The listener's own Spotify app, added once per browser.

import { h } from './dom.js';
import { openPanel } from './sheet.js';
import { redirectUri, spotifySignIn } from './spotify.js';
import { STORE } from './auth.js';

/** Signing in with the listener's own Spotify app, whose address they add once. */
export function spotifyPanel() {
  const client = h('input', { 'aria-label': 'Spotify Client ID', placeholder: 'Client ID', autocapitalize: 'off', autocorrect: 'off', spellcheck: 'false', maxlength: 64, value: localStorage.getItem(STORE.client) ?? '' });
  const problem = h('p', { class: 'problem', role: 'alert', hidden: true });
  const address = h('input', { 'aria-label': 'Redirect address', readonly: true, value: redirectUri(), onfocus: event => event.target.select() });
  openPanel({
    label: 'Sign in with Spotify', className: 'sign-in', form: true,
    onsubmit: event => {
      event.preventDefault();
      const id = client.value.trim();
      if (!/^[a-zA-Z0-9]{16,64}$/.test(id)) { problem.textContent = "That doesn't look like a Client ID. It is 32 letters and numbers, on your app's page in the Spotify developer dashboard."; problem.hidden = false; return; }
      spotifySignIn(id);
    },
    content: [
      h('div', { class: 'about plain' }, h('div', {}, h('b', { text: 'Sign in with Spotify' }),
        h('span', { text: 'This uses your own Spotify app, the same one as on your computer.' }))),
      h('label', {}, h('span', { text: '1. Add this redirect address to your app, once' }), address),
      h('label', {}, h('span', { text: '2. Your Client ID' }), client),
      problem,
      h('button', { class: 'primary', type: 'submit', text: 'Continue to Spotify' }),
    ],
  });
}
