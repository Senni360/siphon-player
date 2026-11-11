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

// One set of line icons on a 24 grid, drawn with a 1.75 stroke, and a few solid
// ones for the transport, where a filled shape reads faster under a thumb.
const ICONS = {
  home: '<path d="M4 10.5 12 4l8 6.5V19a1 1 0 0 1-1 1h-4.5v-5.5h-5V20H5a1 1 0 0 1-1-1z"/>',
  search: '<circle cx="11" cy="11" r="6.5"/><path d="m20 20-4.2-4.2"/>',
  library: '<path d="M5 4.5v15M10 4.5v15M14.6 5.2l4.6 14"/>',
  group: '<circle cx="9" cy="8.5" r="3.5"/><path d="M2.5 20c0-3.6 2.9-6 6.500-6s6.500 2.400 6.500 6M16 5.300a3.500 3.500 0 0 1 0 6.400M21.500 20c0-2.700-1.600-4.800-4-5.600"/>',
  back: '<path d="m14.500 5.500-6.500 6.500 6.500 6.500"/>',
  forward: '<path d="m9.500 5.500 6.500 6.500-6.500 6.500"/>',
  down: '<path d="m5.500 9.500 6.500 6.500 6.500-6.500"/>',
  more: '<circle cx="5.500" cy="12" r="1.100"/><circle cx="12" cy="12" r="1.100"/><circle cx="18.500" cy="12" r="1.100"/>',
  heart: '<path d="M12 20s-7.500-4.700-7.500-10.300A4.200 4.200 0 0 1 12 7a4.200 4.200 0 0 1 7.500 2.700C19.500 15.300 12 20 12 20z"/>',
  shuffle: '<path d="M3.500 7h2.700c4.800 0 6.300 10 11.100 10h3.200M3.500 17h2.700c1.700 0 2.900-1.200 4-2.900M20.500 7h-3.200c-1.700 0-2.900 1.200-4 2.900M18 4.500 20.500 7 18 9.500M18 14.500l2.500 2.500-2.500 2.500"/>',
  repeat: '<path d="M16.500 3.500 20 7l-3.500 3.500M4 11V9.500A2.500 2.500 0 0 1 6.500 7H20M7.500 20.500 4 17l3.500-3.500M20 13v1.500a2.500 2.500 0 0 1-2.500 2.500H4"/>',
  volume: '<path d="M4.500 9.500v5h3.300l4.700 3.800V5.700L7.800 9.500zM16 9.200a4 4 0 0 1 0 5.600M18.600 6.600a7.600 7.600 0 0 1 0 10.800"/>',
  reply: '<path d="M20.500 11.500a8 8 0 0 1-11.700 7.100L4 20l1.400-4.500a8 8 0 1 1 15.100-4z"/>',
  queue: '<path d="M4 6.500h11M4 12h11M4 17.500h7M17.500 13.200v6.300l4-3.150z" />',
  lyrics: '<path d="M5 6h14M5 10.500h9M5 15h12M5 19.500h6"/>',
  share: '<path d="M12 15V4M8 7.500 12 3.500l4 4M5.500 11.500v7a1.500 1.500 0 0 0 1.500 1.500h10a1.500 1.500 0 0 0 1.500-1.500v-7"/>',
  phone: '<rect x="7" y="3" width="10" height="18" rx="2.500"/><path d="M11 17.800h2"/>',
  computer: '<rect x="3" y="4.500" width="18" height="12" rx="2"/><path d="M8.500 20h7M12 16.500V20"/>',
  check: '<path d="m5 12.500 4.500 4.500L19 7.500"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  clock: '<circle cx="12" cy="12" r="8"/><path d="M12 7.500V12l3 2"/>',
  close: '<path d="m6.500 6.500 11 11M17.500 6.500l-11 11"/>',
  sliders: '<path d="M4 7.500h9M17.500 7.500H20M4 16.500h2.500M11 16.500h9"/><circle cx="15.200" cy="7.500" r="2.300"/><circle cx="8.800" cy="16.500" r="2.300"/>',
  grip: '<path d="M5 9h14M5 15h14"/>',
  next_up: '<path d="M4 6.500h16M4 12h9M4 17.500h9M17 11.500v8M13.500 16l3.500 3.500 3.500-3.500"/>',
  last: '<path d="M4 6.500h16M4 12h16M4 17.500h8M18 15v5M15.500 17.500h5"/>',
  person: '<circle cx="12" cy="8.500" r="3.700"/><path d="M4.500 20c0-3.900 3.300-6.300 7.500-6.300s7.500 2.400 7.500 6.300"/>',
  disc: '<circle cx="12" cy="12" r="8.500"/><circle cx="12" cy="12" r="2.500"/>',
  trash: '<path d="M5 7h14M10 7V4.500h4V7M7 7l.8 12a1.500 1.500 0 0 0 1.500 1.400h5.400a1.500 1.500 0 0 0 1.500-1.400L17 7"/>',
  spotify_mark: '<circle cx="12" cy="12" r="9"/><path d="M7.500 9.800c3.200-1 6.700-.7 9.500.9M8.200 12.800c2.500-.7 5.200-.4 7.400.8M9 15.600c1.900-.5 3.700-.3 5.300.6"/>',
  unlink: '<path d="M10 14a4 4 0 0 0 5.700 0l2.600-2.600a4 4 0 0 0-5.700-5.700l-.8.8M14 10a4 4 0 0 0-5.700 0l-2.600 2.600a4 4 0 0 0 5.700 5.700l.8-.8M4 4l16 16"/>',
  download: '<path d="M12 4v11M7.500 10.500 12 15l4.500-4.500M5 19.500h14"/>',
  moon: '<path d="M19.500 14.200A7.800 7.800 0 0 1 9.800 4.500a7.800 7.800 0 1 0 9.700 9.700Z"/>',
  note: '<path d="M9.500 17.500V5.500l9-2v12"/><circle cx="7" cy="17.500" r="2.500"/><circle cx="16" cy="15.500" r="2.500"/>',
};
const SOLID = {
  play: '<path d="M8 4.800v14.400a.8.8 0 0 0 1.200.7l11.400-7.200a.8.8 0 0 0 0-1.400L9.200 4.100a.8.8 0 0 0-1.200.7z"/>',
  pause: '<rect x="6" y="4.500" width="4.200" height="15" rx="1.200"/><rect x="13.800" y="4.500" width="4.200" height="15" rx="1.200"/>',
  next: '<path d="M4.500 5.600v12.800a.7.7 0 0 0 1.100.6l9.400-6.400a.7.7 0 0 0 0-1.200L5.600 5a.7.7 0 0 0-1.100.6z"/><rect x="17" y="4.800" width="2.600" height="14.400" rx="1"/>',
  previous: '<path d="M19.500 5.600v12.800a.7.7 0 0 1-1.100.6L9 12.600a.7.7 0 0 1 0-1.200L18.400 5a.7.7 0 0 1 1.100.6z"/><rect x="4.400" y="4.800" width="2.600" height="14.400" rx="1"/>',
};
export function icon(name) {
  const solid = name in SOLID;
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  svg.setAttribute('class', solid ? `solid i-${name}` : `i-${name}`);
  svg.innerHTML = solid ? SOLID[name] : ICONS[name];
  return svg;
}
export const safeImage = url => (typeof url === 'string' && url.startsWith('https://') ? url : null);
/** A cover, or the quiet tile that stands in for one. */
export function art(url, className = 'art') {
  const safe = safeImage(url);
  if (!safe) return h('div', { class: `${className} blank` }, icon('note'));
  // A cover that never arrives leaves the tile, not a broken picture.
  const image = h('img', { class: className, src: safe, alt: '', loading: 'lazy', decoding: 'async', draggable: 'false' });
  image.addEventListener('error', () => image.replaceWith(h('div', { class: `${className} blank` }, icon('note'))), { once: true });
  return image;
}
export const sourceOf = uri => (String(uri).startsWith('soundcloud:') ? 'soundcloud' : String(uri).startsWith('youtube:') ? 'youtube' : 'spotify');
export const SOURCE_NAMES = { spotify: 'Spotify', soundcloud: 'SoundCloud', youtube: 'YouTube' };
export const clock = ms => {
  const seconds = Math.max(0, Math.floor((ms || 0) / 1000));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor(seconds / 60) % 60;
  return hours ? `${hours}:${String(minutes).padStart(2, '0')}:${String(seconds % 60).padStart(2, '0')}` : `${minutes}:${String(seconds % 60).padStart(2, '0')}`;
};
/** A list's whole length in words: "1 hr 12 min". */
export const span = ms => {
  const minutes = Math.round((ms || 0) / 60000);
  if (minutes < 1) return '';
  return minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)} hr ${minutes % 60} min`;
};
/** How long ago, in a word or two. */
export const ago = when => {
  const at = typeof when === 'number' ? when : Date.parse(when);
  if (!Number.isFinite(at)) return '';
  const minutes = Math.max(0, Math.round((Date.now() - at) / 60000));
  if (minutes < 1) return 'now';
  if (minutes < 60) return `${minutes} min`;
  if (minutes < 1440) return `${Math.round(minutes / 60)} h`;
  if (minutes < 10080) return `${Math.round(minutes / 1440)} d`;
  return new Date(at).toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
};
export const uuid = () => crypto.randomUUID();
export const count = value => new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 }).format(value);
export const plural = (amount, one, many = `${one}s`) => `${amount} ${amount === 1 ? one : many}`;

/** Rows waiting for their songs, in the shape the songs will have. */
export const bones = (amount, kind = 'row') => h('div', { class: 'bones', 'aria-hidden': 'true' },
  Array.from({ length: amount }, (_, at) => {
    const bone = h('div', { class: `bone-${kind}` },
      h('span', { class: 'bone square' }), h('span', { class: 'bone-words' }, h('span', { class: 'bone long' }), h('span', { class: 'bone short' })));
    // Set through the style object: the page's policy refuses style attributes.
    bone.style.setProperty('--i', String(at));
    return bone;
  }));
export const failed = (error, retry) => h('div', { class: 'empty', role: 'alert' }, h('p', { text: error.message }),
  retry && h('button', { class: 'pill', text: 'Try again', onclick: retry }));
/** A page with nothing on it yet says how it fills. */
export const empty = (text, name = 'note') => h('div', { class: 'empty' }, h('span', { class: 'empty-mark' }, icon(name)), h('p', { text }));

/**
 * A short word above the player. With `action` it carries one thing to press
 * (Undo, mostly) and stays longer.
 */
let toastTimer;
export function toast(message, action) {
  document.querySelector('.toast')?.remove();
  const node = h('div', { class: 'toast', role: 'status' }, h('span', { text: message }),
    action && h('button', { text: action.label, onclick: () => { node.remove(); action.run(); } }));
  document.body.append(node);
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    node.classList.add('leaving');
    setTimeout(() => node.remove(), 200);
  }, action ? 4000 : 2400);
}

/**
 * A held finger: `run` fires after a moment of holding still, and the tap that
 * would follow the lift is swallowed.
 */
export function onHold(node, run) {
  let timer = null;
  let from = null;
  let fired = false;
  const stop = () => { clearTimeout(timer); timer = null; };
  node.addEventListener('pointerdown', event => {
    if (event.pointerType === 'mouse' && event.button !== 0) return;
    fired = false;
    from = { x: event.clientX, y: event.clientY };
    stop();
    timer = setTimeout(() => { fired = true; timer = null; run(); }, 420);
  });
  node.addEventListener('pointermove', event => {
    if (timer && from && Math.hypot(event.clientX - from.x, event.clientY - from.y) > 10) stop();
  });
  for (const type of ['pointerup', 'pointercancel', 'pointerleave']) node.addEventListener(type, stop);
  node.addEventListener('click', event => {
    if (!fired) return;
    fired = false;
    event.preventDefault();
    event.stopImmediatePropagation();
  }, true);
  node.addEventListener('contextmenu', event => event.preventDefault());
}
