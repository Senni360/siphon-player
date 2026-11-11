// What is playing, as the listener sees it: the mini player above the tabs, the
// Now Playing sheet it opens into, the queue, the lyrics, and where it plays.
//
// One grammar for the thumb: sideways on the cover or the mini player changes
// the song, down anywhere but a slider closes the sheet, and only the bar under
// the song owns a sideways drag of its own.

import { art, bones, clock, h, icon, onHold, plural, safeImage, sourceOf, toast } from './dom.js';
import { findLyrics } from './lyrics.js';
import { go, over, overClosed } from './navigation.js';
import {
  chooseTarget, flipShuffle, likePlaying, ownsQueue, placeOf, playFrom, playOnComputer, playsHere, refreshPlayer,
  seekTo, setVolume, shown, skip, skipTrouble, sleepAfter, sleepSet, togglePlay, turnRepeat,
} from './player.js';
import { ask } from './remote.js';
import { shareSong, songActions } from './rows.js';
import { choice, insideScrolled, openPanel, pullDown } from './sheet.js';
import { hooks, player } from './state.js';

const still = () => matchMedia('(prefers-reduced-motion: reduce)').matches;
const clamp = (value, low, high) => Math.min(high, Math.max(low, value));

/**
 * A bar that is dragged to a value. It is the only thing in the player that owns
 * a sideways drag, shows where the finger is while it moves, and tells `commit`
 * once, when the finger lifts.
 */
function slider(label, { commit, preview, text = value => String(Math.round(value)), step = 5 }) {
  const bar = h('div', { class: 'slider', role: 'slider', tabindex: '0', 'aria-label': label, 'aria-valuemin': '0' },
    h('span', { class: 'track' }, h('span', { class: 'fill' })), h('span', { class: 'knob' }));
  let max = 1;
  let value = 0;
  let dragging = false;
  const show = () => {
    bar.style.setProperty('--at', String(max > 0 ? clamp(value / max, 0, 1) : 0));
    bar.setAttribute('aria-valuemax', String(Math.round(max)));
    bar.setAttribute('aria-valuenow', String(Math.round(value)));
    bar.setAttribute('aria-valuetext', text(value));
  };
  const under = event => {
    const box = bar.getBoundingClientRect();
    return clamp((event.clientX - box.left) / (box.width || 1), 0, 1) * max;
  };
  bar.addEventListener('pointerdown', event => {
    if (event.pointerType === 'mouse' && event.button !== 0) return;
    dragging = true;
    try { bar.setPointerCapture(event.pointerId); } catch { /* an old browser still gets the moves */ }
    bar.classList.add('dragging');
    value = under(event);
    show();
    preview?.(value);
  });
  bar.addEventListener('pointermove', event => {
    if (!dragging) return;
    value = under(event);
    show();
    preview?.(value);
  });
  const lift = sent => event => {
    if (!dragging) return;
    dragging = false;
    bar.classList.remove('dragging');
    if (sent) { value = under(event); show(); commit(value); }
    preview?.(null);
  };
  bar.addEventListener('pointerup', lift(true));
  bar.addEventListener('pointercancel', lift(false));
  bar.addEventListener('keydown', event => {
    const by = event.key === 'ArrowRight' || event.key === 'ArrowUp' ? step : event.key === 'ArrowLeft' || event.key === 'ArrowDown' ? -step : 0;
    if (!by) return;
    event.preventDefault();
    value = clamp(value + by, 0, max);
    show();
    commit(value);
  });
  show();
  return {
    node: bar,
    get dragging() { return dragging; },
    get value() { return value; },
    set(next, upTo) {
      if (dragging) return;
      max = upTo > 0 ? upTo : 1;
      value = clamp(next || 0, 0, max);
      show();
    },
  };
}

/**
 * A sideways swipe that changes the song. `node` follows the finger; past a
 * quarter of its width, or flicked, it leaves that way and `turn(±1)` runs.
 */
function swipeToSkip(node, turn, { allow = () => true } = {}) {
  let start = null;
  let swiping = false;
  let last = null;
  let speed = 0;
  let swallow = false;
  node.addEventListener('touchstart', event => {
    if (event.touches.length !== 1 || !allow(event)) { start = null; return; }
    const touch = event.touches[0];
    start = { x: touch.clientX, y: touch.clientY };
    last = { x: touch.clientX, at: event.timeStamp };
    swiping = false;
    speed = 0;
  }, { passive: true });
  node.addEventListener('touchmove', event => {
    if (!start) return;
    const touch = event.touches[0];
    const dx = touch.clientX - start.x;
    const dy = touch.clientY - start.y;
    if (!swiping) {
      if (Math.abs(dy) > 10 && Math.abs(dy) > Math.abs(dx)) { start = null; return; }
      if (Math.abs(dx) < 12) return;
      swiping = true;
      node.classList.add('swiping');
    }
    if (event.cancelable) event.preventDefault();
    event.stopPropagation();
    speed = (touch.clientX - last.x) / Math.max(1, event.timeStamp - last.at);
    last = { x: touch.clientX, at: event.timeStamp };
    node.style.setProperty('--swipe', `${dx}px`);
  }, { passive: false });
  const settle = () => { node.classList.remove('swiping'); node.style.removeProperty('--swipe'); };
  node.addEventListener('touchend', event => {
    if (!start) return;
    const dx = (event.changedTouches[0]?.clientX ?? start.x) - start.x;
    const was = swiping;
    start = null;
    swiping = false;
    if (!was) return;
    swallow = true;
    setTimeout(() => { swallow = false; }, 350);
    const width = node.offsetWidth || 1;
    const far = Math.abs(dx) > width * 0.25 || Math.abs(speed) > 0.35;
    settle();
    if (far) turn(dx < 0 ? 1 : -1);
  });
  node.addEventListener('touchcancel', () => { start = null; swiping = false; settle(); });
  // The lift at the end of a swipe is not a tap on what was under it.
  node.addEventListener('click', event => {
    if (!swallow) return;
    swallow = false;
    event.preventDefault();
    event.stopPropagation();
  }, true);
}

// ---------------------------------------------------------------- the mini player

export function buildMini() {
  const line = h('span', { class: 'line' });
  let image = h('div', { class: 'art' });
  const tint = h('span', { class: 'tint', 'aria-hidden': 'true' });
  const name = h('span', { class: 'name' });
  const by = h('span', { class: 'by' });
  const play = h('button', { class: 'play', 'aria-label': 'Play', onclick: togglePlay });
  const next = h('button', { class: 'skip', 'aria-label': 'Next', onclick: () => skip(1) }, icon('next'));
  const open = h('button', { class: 'open', 'aria-label': 'Open what is playing', onclick: openSheet }, image, h('span', { class: 'words' }, name, by));
  const node = h('div', { class: 'mini', hidden: true }, tint, open, play, next, line);
  // The glow the whole page sits on: the playing cover's colour, rising from the foot.
  const glow = h('div', { class: 'app-glow', 'aria-hidden': 'true' });
  swipeToSkip(open, step => {
    node.classList.remove('turned-next', 'turned-previous');
    void node.offsetWidth;
    node.classList.add(step > 0 ? 'turned-next' : 'turned-previous');
    skip(step);
  });
  let uri = null;
  let playing = null;
  let told = null;
  return {
    node, glow,
    update(state, visible) {
      const song = state?.song;
      node.hidden = !visible;
      document.querySelector('main')?.classList.toggle('no-player', !visible);
      glow.classList.toggle('lit', !!visible && !!safeImage(song?.image));
      if (!song) return;
      if (uri !== song.uri) {
        uri = song.uri;
        hooks.markPlaying?.();
        const fresh = art(song.image);
        image.replaceWith(fresh);
        image = fresh;
        const cover = safeImage(song.image);
        tint.style.backgroundImage = cover ? `url("${cover}")` : '';
        glow.style.backgroundImage = cover ? `url("${cover}")` : '';
        name.textContent = song.name;
        name.className = `name ${sourceOf(song.uri)}`;
        told = null;
      }
      if (playing !== state.playing) {
        playing = state.playing;
        play.replaceChildren(icon(state.playing ? 'pause' : 'play'));
        play.setAttribute('aria-label', state.playing ? 'Pause' : 'Play');
        hooks.markPlaying?.();
      }
      // The line under the name says what is wrong, when something is.
      const word = state.trouble ? state.trouble.reason : state.retrying ? 'Reconnecting…' : song.artists.join(', ');
      if (told !== word) {
        told = word;
        by.textContent = word;
        by.classList.toggle('problem', !!state.trouble);
      }
      node.classList.toggle('loading', !!state.loading);
      line.style.setProperty('--at', String(song.duration_ms ? clamp(placeOf(state) / song.duration_ms, 0, 1) : 0));
    },
  };
}

// ---------------------------------------------------------------- where it plays

function deviceSheet() {
  const state = shown();
  const onPhone = playsHere();
  const volume = slider('Volume', { commit: percent => setVolume(Math.round(percent)), text: value => `${Math.round(value)}%`, step: 5 });
  volume.set(state?.volume ?? 50, 100);
  const pick = target => () => close(() => {
    chooseTarget(target);
    if (target === 'phone' && !player.state?.song) toast('The next song you play starts on this phone');
  });
  const close = openPanel({
    label: 'Play on', className: 'devices',
    content: [
      h('div', { class: 'about plain' }, h('div', {}, h('b', { text: 'Play on' }),
        h('span', { text: 'Moving keeps the queue and the place in the song.' }))),
      choice('phone', 'This phone', pick('phone'), { pressed: onPhone, note: 'Every song plays right here. A Spotify song plays as the same song from SoundCloud or YouTube.' }),
      choice('computer', 'Your computer', pick('computer'), { pressed: !onPhone, note: player.online ? 'Encore is open there.' : "Encore isn't open there right now." }),
      // A phone's own volume is its side buttons; only the computer has one to set from here.
      !onPhone && player.online && h('div', { class: 'volume' }, icon('volume'), volume.node),
    ],
  });
}

// ---------------------------------------------------------------- the sleep timer

const minutesText = minutes => (minutes === 60 ? '1 hour' : `${minutes} minutes`);
/** What the timer is set to, in a few words; nothing when it is off. */
const sleepNote = set => (!set ? null : set.song ? 'Stops after this song' : set.minutes === 1 ? 'Stops in a minute' : `Stops in ${set.minutes} minutes`);

function sleepSheet() {
  const set = sleepSet();
  const pick = (when, told) => () => close(() => { sleepAfter(when); toast(told); });
  const close = openPanel({
    label: 'Sleep timer',
    content: [
      h('div', { class: 'about plain' }, h('div', {}, h('b', { text: 'Sleep timer' }),
        h('span', { text: sleepNote(set) ?? 'Pauses the music on this phone by itself.' }))),
      [15, 30, 45, 60].map(minutes => choice('clock', minutesText(minutes), pick(minutes, `Stops in ${minutesText(minutes)}`))),
      choice('note', 'End of this song', pick('song', 'Stops after this song'), { pressed: !!set?.song }),
      set && choice('close', 'Turn off', pick(null, 'Sleep timer off')),
    ],
  });
}

// ---------------------------------------------------------------- now playing

export function openSheet() {
  if (player.sheet) return;
  over();

  const cover = h('div', { class: 'cover-box' });
  const thumb = h('span', { class: 'thumb' });
  const name = h('span', { class: 'big' });
  const artist = h('button', { class: 'artist' });
  const whereName = h('span', { class: 'where-name' });
  const where = h('button', { class: 'where', onclick: deviceSheet }, h('small', { text: 'Playing on' }), h('span', {}, whereName, icon('down')));
  const heart = h('button', { class: 'heart', 'aria-label': 'Like', 'aria-pressed': 'false', onclick: () => {
    heart.classList.remove('popped');
    void heart.offsetWidth;
    heart.classList.add('popped');
    likePlaying();
  } }, icon('heart'));
  const elapsed = h('span');
  const total = h('span');
  const seek = slider('Position', {
    commit: ms => seekTo(ms),
    preview: ms => { if (ms !== null) elapsed.textContent = clock(ms); },
    text: value => clock(value), step: 5000,
  });
  const main = h('button', { class: 'main', 'aria-label': 'Play', onclick: togglePlay });
  const shuffle = h('button', { class: 'side', 'aria-label': 'Shuffle', 'aria-pressed': 'false', onclick: flipShuffle }, icon('shuffle'));
  const repeat = h('button', { class: 'side', 'aria-label': 'Repeat', 'aria-pressed': 'false', onclick: turnRepeat }, icon('repeat'));
  const status = h('div', { class: 'status', role: 'status', hidden: true });
  const lyricsBox = h('div', { class: 'lyrics panel', hidden: true });
  const queueBox = h('div', { class: 'upnext panel', hidden: true });

  // One of three things fills the middle: the cover, the words, or what comes next.
  let panel = 'cover';
  const lyricsButton = h('button', { 'aria-pressed': 'false', onclick: () => showPanel(panel === 'lyrics' ? 'cover' : 'lyrics') }, icon('lyrics'), h('span', { text: 'Lyrics' }));
  const queueButton = h('button', { 'aria-pressed': 'false', onclick: () => showPanel(panel === 'queue' ? 'cover' : 'queue') }, icon('queue'), h('span', { text: 'Queue' }));
  const shareButton = h('button', { onclick: () => {
    const song = shown()?.song;
    if (song?.link) shareSong(song);
    else toast("This song can't be shared from here.");
  } }, icon('share'), h('span', { text: 'Share' }));
  function showPanel(next) {
    panel = next;
    sheet.dataset.panel = next;
    lyricsBox.hidden = next !== 'lyrics';
    queueBox.hidden = next !== 'queue';
    lyricsButton.setAttribute('aria-pressed', String(next === 'lyrics'));
    queueButton.setAttribute('aria-pressed', String(next === 'queue'));
    if (next === 'lyrics') readLyrics(shown()?.song);
    if (next === 'queue') { queueKey = null; drawQueue(shown()); }
  }

  // ------------------------------------------------------------ lyrics
  let lyrics = { uri: null, lines: [], at: -1, synced: false };
  /**
   * The computer knows the words of the song it plays itself. A song playing on
   * this phone, or anything while the computer is off, is looked up from here.
   */
  async function wordsFor(song) {
    const state = shown();
    if (!state?.here && player.online && !player.direct) {
      try {
        let found = await ask('lyrics');
        // Just after a change the computer may still hold the last song's words.
        for (let tries = 0; found.uri && found.uri !== song.uri && tries < 4; tries++) {
          await new Promise(resolve => setTimeout(resolve, 1500));
          if (lyrics.uri !== song.uri) return null;
          found = await ask('lyrics');
        }
        if (!found.uri || found.uri === song.uri) return found;
      } catch { /* the phone's own look is next */ }
      if (lyrics.uri !== song.uri) return null;
    }
    return findLyrics(song);
  }
  async function readLyrics(song) {
    const forUri = song?.uri;
    if (!forUri || lyrics.uri === forUri) return;
    lyrics = { uri: forUri, lines: [], at: -1, synced: false };
    lyricsBox.replaceChildren(bones(5, 'line'));
    try {
      const found = await wordsFor(song);
      if (lyrics.uri !== forUri) return;
      lyrics.lines = found?.lines ?? [];
      if (!lyrics.lines.length) {
        lyricsBox.replaceChildren(h('p', { class: 'quiet-note', text: found?.instrumental ? 'This one has no words.' : 'No lyrics for this song.' }));
        return;
      }
      lyricsBox.replaceChildren(...lyrics.lines.map(line => (found.synced
        ? h('button', { text: line.text || '♪', onclick: () => seekTo(line.at_ms) })
        : h('p', { text: line.text || ' ' }))));
      lyrics.synced = !!found.synced;
      lyrics.at = -1;
    } catch (error) {
      if (lyrics.uri !== forUri) return;
      // Asked again the next time the words are opened, not remembered as missing.
      lyrics.uri = null;
      lyricsBox.replaceChildren(h('p', { class: 'quiet-note', text: error.message }));
    }
  }
  function followLyrics(at) {
    if (!lyrics.synced || panel !== 'lyrics') return;
    let line = -1;
    lyrics.lines.forEach((item, index) => { if (item.at_ms != null && item.at_ms <= at) line = index; });
    if (line === lyrics.at) return;
    lyricsBox.children[lyrics.at]?.classList.remove('now');
    lyrics.at = line;
    const sung = lyricsBox.children[line];
    if (!sung) return;
    sung.classList.add('now');
    lyricsBox.scrollTo({ top: sung.offsetTop - lyricsBox.clientHeight * 0.35, behavior: still() ? 'auto' : 'smooth' });
  }

  // ------------------------------------------------------------ the queue
  const QUEUE_SHOWN = 80;
  let queueKey = null;
  let draggingRow = false;
  function drawQueue(state) {
    if (panel !== 'queue' || draggingRow || !state) return;
    const own = ownsQueue();
    const coming = state.upcoming ?? state.next ?? [];
    const key = `${own}|${coming.length}|${coming.slice(0, QUEUE_SHOWN).map(item => item.uri).join()}`;
    if (key === queueKey) return;
    queueKey = key;
    const kept = queueBox.scrollTop;
    if (!coming.length) {
      queueBox.replaceChildren(h('div', { class: 'quiet-note' }, h('p', { text: 'Nothing queued.' }),
        h('p', { class: 'small', text: 'Hold any song and choose Play next.' })));
      return;
    }
    const list = h('div', { class: 'queue-list' }, coming.slice(0, QUEUE_SHOWN).map((song, at) => queueRow(song, at, own, coming)));
    queueBox.replaceChildren(
      h('div', { class: 'queue-head' }, h('h2', { text: 'Up next' }), h('span', { class: 'figure', text: plural(coming.length, 'song') }),
        own && h('button', { class: 'quiet', text: 'Clear', onclick: () => {
          const taken = hooks.hereActs.clear();
          toast('Queue cleared', { label: 'Undo', run: () => hooks.hereActs.restore({ songs: taken }) });
        } })),
      list,
      coming.length > QUEUE_SHOWN ? h('p', { class: 'note', text: `and ${coming.length - QUEUE_SHOWN} more` }) : null,
      !own && h('p', { class: 'note', text: 'The queue can be changed while the phone plays.' }));
    queueBox.scrollTop = kept;
  }
  function removeQueued(song, at) {
    const taken = hooks.hereActs.remove({ at });
    if (taken) toast(`Removed ${song.name}`, { label: 'Undo', run: () => hooks.hereActs.queue({ song: taken, at }) });
  }
  function queueRow(song, at, own, coming) {
    const jump = () => {
      if (own) Promise.resolve(hooks.hereActs.jump({ at })).catch(() => {});
      // The computer's own queue: play the same list on from that song.
      else playFrom(coming, at);
    };
    const handle = own && h('button', { class: 'handle', 'aria-label': `Move ${song.name}` }, icon('grip'));
    const node = h('div', { class: 'row queued', 'data-uri': song.uri },
      art(song.image),
      h('button', { class: 'words', onclick: jump },
        h('span', { class: `name ${sourceOf(song.uri)}`, text: song.name }), h('span', { class: 'by', text: song.artists.join(', ') })),
      handle);
    if (!own) return node;
    handle.addEventListener('pointerdown', event => dragRow(event, node, at));
    // A handle is also a button: arrows move the song for a keyboard or a switch.
    handle.addEventListener('keydown', event => {
      const to = event.key === 'ArrowUp' ? at - 1 : event.key === 'ArrowDown' ? at + 1 : null;
      if (to === null) return;
      event.preventDefault();
      hooks.hereActs.move({ from: at, to });
    });
    swipeAway(node, () => removeQueued(song, at));
    onHold(node, () => songActions(song, [['trash', 'Remove from queue', () => removeQueued(song, at)]]));
    return node;
  }
  /** Carries a row up or down under the finger; the others step aside. */
  function dragRow(event, node, from) {
    if (event.pointerType === 'mouse' && event.button !== 0) return;
    event.preventDefault();
    const handle = event.currentTarget;
    const list = node.parentElement;
    const others = [...list.children];
    const height = node.offsetHeight;
    const startY = event.clientY;
    const startScroll = queueBox.scrollTop;
    let pointerY = startY;
    let to = from;
    let frame = 0;
    draggingRow = true;
    try { handle.setPointerCapture(event.pointerId); } catch { /* moves still arrive */ }
    node.classList.add('lifted');
    list.classList.add('sorting');
    const place = () => {
      const moved = pointerY - startY + (queueBox.scrollTop - startScroll);
      node.style.transform = `translate3d(0, ${moved}px, 0) scale(1.02)`;
      to = clamp(from + Math.round(moved / height), 0, others.length - 1);
      others.forEach((other, at) => {
        if (other === node) return;
        const shift = from < to && at > from && at <= to ? -height : from > to && at >= to && at < from ? height : 0;
        other.style.transform = shift ? `translate3d(0, ${shift}px, 0)` : '';
      });
    };
    // Near an edge the list moves on by itself, so a song can travel further than a screen.
    const edge = () => {
      const box = queueBox.getBoundingClientRect();
      const push = pointerY < box.top + 56 ? -8 : pointerY > box.bottom - 56 ? 8 : 0;
      if (push) { queueBox.scrollTop += push; place(); }
      frame = requestAnimationFrame(edge);
    };
    frame = requestAnimationFrame(edge);
    const move = moving => { pointerY = moving.clientY; place(); };
    const drop = done => {
      cancelAnimationFrame(frame);
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
      handle.removeEventListener('pointercancel', cancel);
      draggingRow = false;
      node.classList.remove('lifted');
      list.classList.remove('sorting');
      for (const other of others) other.style.transform = '';
      if (done && to !== from) hooks.hereActs.move({ from, to });
    };
    const up = () => drop(true);
    const cancel = () => drop(false);
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
    handle.addEventListener('pointercancel', cancel);
  }
  /** A row pushed off to the left is taken out. Short of a third, it comes back. */
  function swipeAway(node, gone) {
    let start = null;
    let sliding = false;
    node.addEventListener('touchstart', event => {
      if (event.target.closest('.handle') || event.touches.length !== 1) { start = null; return; }
      start = { x: event.touches[0].clientX, y: event.touches[0].clientY };
      sliding = false;
    }, { passive: true });
    node.addEventListener('touchmove', event => {
      if (!start) return;
      const dx = event.touches[0].clientX - start.x;
      const dy = event.touches[0].clientY - start.y;
      if (!sliding) {
        if (Math.abs(dy) > 10 && Math.abs(dy) > Math.abs(dx)) { start = null; return; }
        if (dx > -12) return;
        sliding = true;
        node.classList.add('sliding');
      }
      if (event.cancelable) event.preventDefault();
      event.stopPropagation();
      node.style.transform = `translate3d(${Math.min(0, dx)}px, 0, 0)`;
      node.style.opacity = String(clamp(1 + dx / (node.offsetWidth || 1), 0.2, 1));
    }, { passive: false });
    const end = event => {
      if (!start) return;
      const dx = (event.changedTouches?.[0]?.clientX ?? start.x) - start.x;
      const was = sliding;
      start = null;
      sliding = false;
      if (!was) return;
      node.classList.remove('sliding');
      if (event.type === 'touchend' && -dx > (node.offsetWidth || 1) * 0.35) { gone(); return; }
      node.style.transform = '';
      node.style.opacity = '';
    };
    node.addEventListener('touchend', end);
    node.addEventListener('touchcancel', end);
  }

  // ------------------------------------------------------------ the sheet itself
  let closed = false;
  function close() {
    if (closed) return;
    closed = true;
    player.sheet = null;
    overClosed();
    if (still()) { sheet.remove(); return; }
    sheet.style.transition = '';
    sheet.style.transform = '';
    sheet.classList.add('leaving');
    setTimeout(() => sheet.remove(), 280);
  }
  const more = h('button', { class: 'icon-button', 'aria-label': 'More for this song', onclick: () => {
    const song = shown()?.song;
    // The sleep timer belongs to the phone's own player, so it shows while that plays.
    if (song) songActions(song, ownsQueue() ? [['moon', 'Sleep timer', sleepSheet, { note: sleepNote(sleepSet()) }]] : []);
  } }, icon('more'));
  const stage = h('div', { class: 'stage' }, cover, lyricsBox, queueBox);
  const sheet = h('div', { class: 'sheet', role: 'dialog', 'aria-label': 'Now playing', 'aria-modal': 'true', 'data-panel': 'cover' },
    h('div', { class: 'glow', 'aria-hidden': 'true' }),
    h('header', {}, h('button', { class: 'icon-button', 'aria-label': 'Close', onclick: close }, icon('down')), where, more),
    stage,
    h('div', { class: 'heading' }, thumb, h('div', { class: 'heading-words' }, name, artist), heart),
    status,
    h('div', { class: 'seek' }, seek.node, h('div', { class: 'times figure' }, elapsed, total)),
    h('div', { class: 'transport' }, shuffle,
      h('button', { class: 'step', 'aria-label': 'Previous', onclick: () => skip(-1) }, icon('previous')),
      main,
      h('button', { class: 'step', 'aria-label': 'Next', onclick: () => skip(1) }, icon('next')),
      repeat),
    h('div', { class: 'tools' }, lyricsButton, shareButton, queueButton));

  // Down closes it, from anywhere that is not a slider, a handle, or a list part-way down.
  pullDown(sheet, {
    allow: event => !event.target.closest('.slider, .handle') && !insideScrolled(event.target, sheet),
    onClose: close,
  });
  // Sideways on the cover or the song's name changes the song.
  let turned = 0;
  for (const zone of [cover, sheet.querySelector('.heading-words')]) {
    swipeToSkip(zone, step => { turned = step; skip(step); });
  }
  document.body.append(sheet);

  let uri = null;
  let wasPlaying = null;
  let shownTrouble = null;
  player.sheet = { close, update(state) {
    const song = state?.song;
    if (!song) return;
    if (song.uri !== uri) {
      const first = uri === null;
      uri = song.uri;
      // The new cover comes in from the side the song came from; the old one leaves the other way.
      const fresh = art(song.image, 'cover');
      const way = turned < 0 ? 'from-left' : turned > 0 ? 'from-right' : 'from-below';
      turned = 0;
      for (const old of cover.children) {
        old.classList.add('leaving');
        setTimeout(() => old.remove(), 320);
      }
      if (!first) fresh.classList.add(way);
      cover.append(fresh);
      thumb.replaceChildren(art(song.image));
      const picture = safeImage(song.image);
      sheet.querySelector('.glow').style.backgroundImage = picture ? `url("${picture}")` : '';
      name.textContent = song.name;
      name.className = `big ${sourceOf(song.uri)}`;
      artist.textContent = song.artists.join(', ');
      artist.onclick = song.artist_id ? () => { close(); go({ page: 'artist', id: song.artist_id }); } : null;
      artist.disabled = !song.artist_id;
      if (panel === 'lyrics') readLyrics(song);
    }
    whereName.textContent = state.here ? 'This phone' : state.device || 'Your computer';
    // One line says what is wrong and what can be done about it, never a dialog.
    const troubleKey = state.trouble ? `t|${state.trouble.uri}|${state.trouble.reason}` : state.retrying ? 'r' : '';
    if (troubleKey !== shownTrouble) {
      shownTrouble = troubleKey;
      status.hidden = !troubleKey;
      status.replaceChildren(...(state.trouble ? [
        h('p', { text: state.trouble.reason }),
        h('div', { class: 'status-actions' },
          h('button', { class: 'pill', text: 'Try again', onclick: togglePlay }),
          h('button', { class: 'pill', text: 'Skip', onclick: skipTrouble }),
          player.online && h('button', { class: 'pill', text: 'Play on computer', onclick: () => playOnComputer(song) })),
      ] : state.retrying ? [h('p', { text: 'Lost the connection. Trying this song again…' })] : []));
    }
    sheet.classList.toggle('loading', !!state.loading);
    sheet.classList.toggle('paused', !state.playing);
    heart.setAttribute('aria-pressed', String(!!state.liked));
    heart.setAttribute('aria-label', state.liked ? 'Unlike' : 'Like');
    shuffle.setAttribute('aria-pressed', String(!!state.shuffle));
    repeat.setAttribute('aria-pressed', String(state.repeat !== 'off'));
    repeat.classList.toggle('one', state.repeat === 'one');
    repeat.setAttribute('aria-label', state.repeat === 'one' ? 'Repeat one' : state.repeat === 'all' ? 'Repeat all' : 'Repeat');
    if (wasPlaying !== state.playing) {
      wasPlaying = state.playing;
      main.replaceChildren(icon(state.playing ? 'pause' : 'play'));
      main.setAttribute('aria-label', state.playing ? 'Pause' : 'Play');
    }
    const at = placeOf(state);
    followLyrics(at);
    seek.set(at, song.duration_ms || 1);
    if (!seek.dragging) elapsed.textContent = clock(at);
    total.textContent = clock(song.duration_ms);
    drawQueue(state);
  } };
  refreshPlayer();
}

hooks.closeSheet = () => player.sheet?.close();
