// Installing the app, and its one-time tip.

import { h } from './dom.js';
import { keepStorage } from './auth.js';
import { current } from './navigation.js';
import { hooks } from './state.js';

const INSTALL_TIP = 'encore.install';
let installEvent = null;
const installed = () => navigator.standalone === true || matchMedia('(display-mode: standalone)').matches;
const phoneBrowser = () => /iPhone|iPad|iPod|Android/i.test(navigator.userAgent) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
const applePhone = () => /iPhone|iPad|iPod/.test(navigator.userAgent) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);

// Android offers an install prompt once; the tip below only appears when it
// does. iPhone has no prompt, so its tip explains Share, then Add to Home Screen.
addEventListener('beforeinstallprompt', event => {
  if (!phoneBrowser() || installed()) return;
  event.preventDefault();
  installEvent = event;
  if (document.querySelector('main') && current.page === 'home' && !localStorage.getItem(INSTALL_TIP)) hooks.render?.(true);
});
addEventListener('appinstalled', () => {
  installEvent = null;
  localStorage.setItem(INSTALL_TIP, 'installed');
  keepStorage();
  for (const tip of document.querySelectorAll('.install')) tip.remove();
});

/** Once, a tip for keeping Encore on the home screen, where it opens like an app. */
export function installTip() {
  if (installed() || localStorage.getItem(INSTALL_TIP)) return null;
  const apple = applePhone();
  if (!apple && !installEvent) return null;
  const done = value => {
    localStorage.setItem(INSTALL_TIP, value);
    tip.remove();
  };
  const tip = h('div', { class: 'banner install' },
    h('div', {}, h('b', { text: apple ? 'Keep Encore on your home screen' : 'Install Encore' }),
      apple ? 'Press Share, then Add to Home Screen.' : 'It opens full screen, like the app on your computer.'),
    !apple && h('button', { class: 'chip plain install-go', text: 'Install', onclick: async () => {
      const event = installEvent;
      if (!event) return;
      event.prompt();
      const choice = await event.userChoice.catch(() => null);
      installEvent = null;
      done(choice?.outcome === 'accepted' ? 'installed' : '1');
    } }),
    h('button', { class: 'tip-close', 'aria-label': 'Dismiss', onclick: () => done('1') }, '×'));
  return tip;
}
