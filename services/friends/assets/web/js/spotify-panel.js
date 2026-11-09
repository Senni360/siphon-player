// The listener's own Spotify app, added once per browser.

import { h } from './dom.js';
import { over, overClosed } from './navigation.js';
import { redirectUri, spotifySignIn } from './spotify.js';
import { STORE } from './auth.js';

/** Signing in with the listener's own Spotify app, whose address they add once. */
export function spotifyPanel() {
  const client = h('input', { 'aria-label': 'Spotify Client ID', placeholder: 'Client ID', autocapitalize: 'off', autocorrect: 'off', spellcheck: 'false', maxlength: 64, value: localStorage.getItem(STORE.client) ?? '' });
  const problem = h('p', { class: 'problem', role: 'alert', hidden: true });
  const close = () => { scrim.remove(); panel.remove(); overClosed(); };
  const scrim = h('div', { class: 'scrim', onclick: close });
  const address = h('input', { 'aria-label': 'Redirect address', readonly: true, value: redirectUri(), onfocus: event => event.target.select() });
  const panel = h('form', { class: 'door actions sign-in', onsubmit: event => {
    event.preventDefault();
    const id = client.value.trim();
    if (!/^[a-zA-Z0-9]{16,64}$/.test(id)) { problem.textContent = "That doesn't look like a Client ID."; problem.hidden = false; return; }
    spotifySignIn(id);
  } },
    h('p', { text: 'This uses your own Spotify app, the same one as on your computer. In the Spotify developer dashboard, add this redirect address to it once:' }),
    address,
    h('label', { text: 'Your Client ID' }), client, problem,
    h('button', { class: 'primary', type: 'submit', text: 'Continue to Spotify' }));
  document.body.append(scrim, panel);
  over();
}
