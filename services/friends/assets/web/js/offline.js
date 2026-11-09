// The offline shell: keep it, and offer a refresh when a new one lands.

import { h } from './dom.js';

let watchingShell = false;
/** Keeps the shell for an offline open, and offers a refresh when a new one lands. */
export function keepShell() {
  if (watchingShell || !('serviceWorker' in navigator)) return;
  watchingShell = true;
  navigator.serviceWorker.register('/sw.js').then(registration => {
    const offer = worker => {
      if (!worker || !navigator.serviceWorker.controller) return;  // the first install has nothing older
      if (sessionStorage.getItem('encore.update')) return;  // once per session
      sessionStorage.setItem('encore.update', '1');
      showUpdate(worker);
    };
    if (registration.waiting) offer(registration.waiting);
    registration.addEventListener('updatefound', () => {
      const worker = registration.installing;
      if (!worker) return;
      worker.addEventListener('statechange', () => { if (worker.state === 'installed') offer(worker); });
    });
  }).catch(() => { /* without it the app still runs, just not offline */ });
}
/** A small, dismissible note in a corner: it never stops what is playing. */
function showUpdate(worker) {
  const note = h('div', { class: 'update', role: 'status' },
    h('span', { text: 'A new version of Encore is ready.' }),
    h('button', { class: 'update-go', text: 'Refresh', onclick: () => {
      navigator.serviceWorker.addEventListener('controllerchange', () => location.reload(), { once: true });
      worker.postMessage({ type: 'skip-waiting' });
    } }),
    h('button', { class: 'update-close', 'aria-label': 'Not now', onclick: () => note.remove() }, '×'));
  document.body.append(note);
}
