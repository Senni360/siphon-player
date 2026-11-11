// Settings: where sound comes out, Spotify on this phone, and unlinking.

import { unlink } from '../account.js';
import { STORE } from '../auth.js';
import { h, toast } from '../dom.js';
import { topBar } from '../navigation.js';
import { chooseTarget, playsHere } from '../player.js';
import { heading } from '../rows.js';
import { choice } from '../sheet.js';
import { hasSpotify } from '../spotify.js';
import { spotifyPanel } from '../spotify-panel.js';
import { hooks, player } from '../state.js';

export function settingsScreen() {
  const playOn = h('div', { class: 'group' });
  const drawPlayOn = () => {
    const onPhone = playsHere();
    const pick = target => () => { chooseTarget(target); drawPlayOn(); };
    playOn.replaceChildren(
      choice('phone', 'This phone', pick('phone'), { pressed: onPhone, note: 'Every song plays right here. A Spotify song plays as the same song from SoundCloud or YouTube.' }),
      choice('computer', 'Your computer', pick('computer'), { pressed: !onPhone, note: player.online ? 'Encore is open there. This phone is its remote.' : "Encore isn't open there right now." }));
  };
  drawPlayOn();

  const spotify = h('div', { class: 'group' });
  const drawSpotify = () => {
    spotify.replaceChildren(hasSpotify()
      ? choice('spotify_mark', 'Signed in to Spotify on this phone', () => {
        localStorage.removeItem(STORE.spotify);
        toast('Signed out of Spotify on this phone');
        drawSpotify();
        hooks.reloadViews?.();
      }, { note: 'Your Spotify library and search answer with the computer off. Press to sign out.' })
      : choice('spotify_mark', 'Keep Spotify going with the computer off', () => spotifyPanel(), { note: 'Sign in with your own Spotify app, the same one as on your computer.' }));
  };
  drawSpotify();

  const name = localStorage.getItem(STORE.name);
  return [
    topBar('Settings'),
    h('h1', { class: 'title', text: 'Settings' }),
    heading('Play on'), playOn,
    heading('Spotify'), spotify,
    heading('This phone', name ? `Linked as ${name}` : null),
    h('div', { class: 'group' },
      choice('unlink', 'Unlink this phone', unlink, { danger: true, note: 'Signs Encore out here. A new code from your computer links it again.' })),
    heading('Good to know'),
    h('ul', { class: 'facts' },
      h('li', { text: 'Your computer and this phone stay in step while Encore is open here. A locked phone catches up when you open it.' }),
      h('li', { text: "The volume is the phone's own buttons." }),
      h('li', { text: 'With the computer off, a song that only exists on YouTube plays if it was played recently.' })),
  ];
}
