// A panel that rises from the bottom over the page, and the drag that every
// sheet in the app shares: it follows the finger, and lets go either way.

import { h, icon } from './dom.js';
import { over, overClosed } from './navigation.js';

const still = () => matchMedia('(prefers-reduced-motion: reduce)').matches;

/**
 * Lets `node` be pulled down and away. `allow(event)` says whether a touch may
 * start a pull (not from a slider, not from a list scrolled part-way). `onClose`
 * runs once it has gone; a short or slow pull springs back.
 */
export function pullDown(node, { allow = () => true, onClose, onMove } = {}) {
  let start = null;
  let pulling = false;
  let last = null;
  let speed = 0;
  const place = (y, moving) => {
    node.style.transition = moving ? 'none' : '';
    node.style.transform = y ? `translate3d(0, ${y}px, 0)` : '';
    onMove?.(y, node.offsetHeight || 1);
  };
  node.addEventListener('touchstart', event => {
    if (event.touches.length !== 1 || !allow(event)) { start = null; return; }
    const touch = event.touches[0];
    start = { x: touch.clientX, y: touch.clientY };
    last = { y: touch.clientY, at: event.timeStamp };
    pulling = false;
    speed = 0;
  }, { passive: true });
  node.addEventListener('touchmove', event => {
    if (!start) return;
    const touch = event.touches[0];
    const dx = touch.clientX - start.x;
    const dy = touch.clientY - start.y;
    if (!pulling) {
      // Sideways or upwards first: it is some other gesture's touch.
      if (Math.abs(dx) > 10 && Math.abs(dx) > Math.abs(dy)) { start = null; return; }
      if (dy < -6) { start = null; return; }
      if (dy < 8) return;
      pulling = true;
    }
    if (event.cancelable) event.preventDefault();
    const elapsed = Math.max(1, event.timeStamp - last.at);
    speed = (touch.clientY - last.y) / elapsed;
    last = { y: touch.clientY, at: event.timeStamp };
    place(Math.max(0, dy), true);
  }, { passive: false });
  const release = event => {
    if (!start) return;
    const dy = (event.changedTouches[0]?.clientY ?? start.y) - start.y;
    const was = pulling;
    start = null;
    pulling = false;
    if (!was) return;
    // A third of the way, or a flick, sends it off.
    if (dy > node.offsetHeight * 0.3 || speed > 0.5) onClose?.();
    else place(0, false);
  };
  node.addEventListener('touchend', release);
  node.addEventListener('touchcancel', () => { if (pulling) place(0, false); start = null; pulling = false; });
}

/** Whether a touch began inside something that is itself scrolled part-way down. */
export function insideScrolled(target, root) {
  for (let node = target; node && node !== root; node = node.parentElement) {
    if (node.scrollHeight > node.clientHeight + 1 && node.scrollTop > 0) return true;
  }
  return false;
}

/**
 * A small sheet for a handful of choices or a short form. Returns `close`.
 * It is a step back of its own: the back gesture closes it before the page.
 */
export function openPanel({ label, className = '', content, form = false, onsubmit }) {
  let closed = false;
  const scrim = h('div', { class: 'scrim', onclick: () => close() });
  const panel = h(form ? 'form' : 'div', {
    class: `actions ${className}`.trim(), role: form ? null : 'dialog', 'aria-label': label, 'aria-modal': 'true', onsubmit,
  }, h('span', { class: 'grab', 'aria-hidden': 'true' }), content);
  function close(after) {
    if (closed) return;
    closed = true;
    const gone = () => { scrim.remove(); panel.remove(); after?.(); };
    overClosed();
    if (still()) { gone(); return; }
    scrim.classList.add('leaving');
    panel.classList.add('leaving');
    panel.style.transition = '';
    panel.style.transform = '';
    setTimeout(gone, 220);
  }
  // Escape and the back gesture find the panel through its scrim.
  scrim.closePanel = close;
  pullDown(panel, {
    allow: event => !event.target.closest('input, textarea, [role="slider"]') && !insideScrolled(event.target, panel),
    onClose: () => close(),
    onMove: (y, height) => { scrim.style.opacity = y ? String(Math.max(0, 1 - y / height)) : ''; },
  });
  document.body.append(scrim, panel);
  over();
  return close;
}

/** One line in a sheet of choices. */
export function choice(name, label, run, { note, pressed, danger } = {}) {
  return h('button', { class: `choice${danger ? ' danger' : ''}`, type: 'button', 'aria-pressed': pressed === undefined ? null : String(pressed), onclick: run },
    icon(name), h('span', { class: 'choice-words' }, h('span', { text: label }), note && h('small', { text: note })),
    pressed ? icon('check') : null);
}
