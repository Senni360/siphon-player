// Drawing helpers. Everything shown goes in as text, never as markup.

export const app = document.getElementById('app');

export function h(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === undefined || value === null || value === false) continue;
    if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else if (key === 'class') node.className = value;
    else if (key === 'text') node.textContent = value;
    else node.setAttribute(key, value === true ? '' : value);
  }
  node.append(...children.flat(Infinity).filter(child => child !== null && child !== undefined && child !== false));
  return node;
}
const ICONS = {
  home: '<path d="M3 11l9-8 9 8v9a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="M20 20l-3.5-3.5"/>',
  library: '<path d="M5 4v16M10 4v16M15 4l5 16"/>',
  group: '<circle cx="9" cy="8" r="4"/><path d="M2 21c0-4 3-7 7-7s7 3 7 7M16 4a4 4 0 0 1 0 8M22 21c0-3-2-6-5-6.5"/>',
  back: '<path d="M15 5l-7 7 7 7"/>',
  down: '<path d="M5 9l7 7 7-7"/>',
  more: '<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
  heart: '<path d="M12 20s-8-5-8-11a4.5 4.5 0 0 1 8-3 4.5 4.5 0 0 1 8 3c0 6-8 11-8 11z"/>',
  shuffle: '<path d="M3 6h3c5 0 7 12 12 12h3M3 18h3c2 0 3.500-2 5-4.500M21 6h-3c-2 0-3.500 2-5 4.500M18 3l3 3-3 3M18 15l3 3-3 3"/>',
  repeat: '<path d="M17 3l4 4-4 4M3 11V9a2 2 0 0 1 2-2h16M7 21l-4-4 4-4M21 13v2a2 2 0 0 1-2 2H3"/>',
  volume: '<path d="M4 9v6h4l5 4V5L8 9zM17 9a4 4 0 0 1 0 6"/>',
  reply: '<path d="M21 12a8 8 0 0 1-12 7l-5 1 1-5a8 8 0 1 1 16-3z"/>',
};
const SOLID = {
  play: '<path d="M7 4l14 8-14 8z"/>',
  pause: '<rect x="6" y="4" width="4" height="16" rx="1"/><rect x="14" y="4" width="4" height="16" rx="1"/>',
  next: '<path d="M5 4l11 8-11 8zM18 4h2v16h-2z"/>',
  previous: '<path d="M19 4L8 12l11 8zM4 4h2v16H4z"/>',
};
export function icon(name) {
  const solid = name in SOLID;
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  if (solid) svg.setAttribute('class', 'solid');
  svg.innerHTML = solid ? SOLID[name] : ICONS[name];
  return svg;
}
export const safeImage = url => (typeof url === 'string' && url.startsWith('https://') ? url : null);
export function art(url, className = 'art') {
  const safe = safeImage(url);
  return safe ? h('img', { class: className, src: safe, alt: '', loading: 'lazy', decoding: 'async' }) : h('div', { class: className });
}
export const sourceOf = uri => (String(uri).startsWith('soundcloud:') ? 'soundcloud' : String(uri).startsWith('youtube:') ? 'youtube' : 'spotify');
export const SOURCE_NAMES = { spotify: 'Spotify', soundcloud: 'SoundCloud', youtube: 'YouTube' };
export const clock = ms => {
  const seconds = Math.max(0, Math.floor((ms || 0) / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
};
export const uuid = () => crypto.randomUUID();
export const count = value => new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 }).format(value);

export const bones = amount => h('div', {}, Array.from({ length: amount }, () => h('div', { class: 'bone line' })));
export const failed = (error, retry) => h('div', { class: 'empty' }, h('p', { text: error.message }), retry && h('button', { class: 'see-more', text: 'Try again', onclick: retry }));

let toastTimer;
export function toast(message) {
  document.querySelector('.toast')?.remove();
  document.body.append(h('div', { class: 'toast', role: 'status', text: message }));
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => document.querySelector('.toast')?.remove(), 2600);
}
