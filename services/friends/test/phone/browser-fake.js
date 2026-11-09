// Browser-side doubles for the phone tests, installed with addInitScript before
// assets/web/app.js runs on every document:
//
//   * window.Audio is a fake element, so play, pause, currentTime, ended and
//     error can be driven and read without real audio.
//   * navigator.mediaSession and MediaMetadata record what the app asks the lock
//     screen for, without a real one.
//   * window.open and unhandled rejections are recorded for the test to assert.
//
// The test reads it through window.__audio, window.__mediaSession and
// window.__phoneTest (see fixtures.ts for the typed wrappers).
(() => {
  'use strict';
  if (window.__phoneTest && window.__phoneTest.installed) return;

  // -------------------------------------------------------------- fake audio
  const created = [];

  class FakeAudio extends EventTarget {
    constructor() {
      super();
      created.push(this);
      this.preload = '';
      this._src = '';
      this._currentTime = 0;
      this._duration = 214;
      this._paused = true;
      this._ended = false;
      this._volume = 1;
      this._muted = false;
      this._error = null;
      this._readyState = 0;
      this.playCalls = 0;
      this.pauseCalls = 0;
    }

    get src() { return this._src; }
    set src(value) {
      this._src = String(value);
      this._currentTime = 0;
      this._ended = false;
      this._error = null;
      this._paused = true;
      this._readyState = 4;
      this.dispatchEvent(new Event('loadstart'));
      this.dispatchEvent(new Event('loadedmetadata'));
    }

    get currentTime() { return this._currentTime; }
    set currentTime(value) {
      this._currentTime = Math.max(0, Number(value) || 0);
      this.dispatchEvent(new Event('timeupdate'));
      this.dispatchEvent(new Event('seeked'));
    }

    get duration() { return this._duration; }
    get paused() { return this._paused; }
    get ended() { return this._ended; }
    get error() { return this._error; }
    get readyState() { return this._readyState; }

    get volume() { return this._volume; }
    set volume(value) { this._volume = Math.min(1, Math.max(0, Number(value) || 0)); }
    get muted() { return this._muted; }
    set muted(value) { this._muted = Boolean(value); }

    canPlayType() { return ''; }

    play() {
      this.playCalls += 1;
      this._paused = false;
      this._ended = false;
      this._readyState = 4;
      this.dispatchEvent(new Event('play'));
      this.dispatchEvent(new Event('playing'));
      return Promise.resolve();
    }

    pause() {
      this.pauseCalls += 1;
      if (this._paused) return;
      this._paused = true;
      this.dispatchEvent(new Event('pause'));
    }

    load() {
      this._currentTime = 0;
      this._ended = false;
      this._error = null;
      this._paused = true;
      this._readyState = 0;
      this.dispatchEvent(new Event('loadstart'));
    }
  }

  window.Audio = FakeAudio;

  const snapshot = element => ({
    src: element.src,
    currentTime: element.currentTime,
    duration: element.duration,
    paused: element.paused,
    ended: element.ended,
    volume: element.volume,
    muted: element.muted,
    error: element.error,
    readyState: element.readyState,
    playCalls: element.playCalls,
    pauseCalls: element.pauseCalls,
  });
  const active = () => created[created.length - 1] || null;
  const requireActive = () => {
    const element = active();
    if (!element) throw new Error('no audio element has been created yet');
    return element;
  };

  window.__audio = {
    count: () => created.length,
    current: () => (active() ? snapshot(active()) : null),
    all: () => created.map(snapshot),
    advance(seconds) {
      const element = requireActive();
      element._currentTime = Math.min(element._duration, element._currentTime + Number(seconds));
      element.dispatchEvent(new Event('timeupdate'));
      if (element._currentTime >= element._duration && !element._ended) {
        element._paused = true;
        element._ended = true;
        element.dispatchEvent(new Event('ended'));
      }
      return snapshot(element);
    },
    seek(seconds) {
      requireActive().currentTime = seconds;
      return snapshot(requireActive());
    },
    fail(details) {
      const element = requireActive();
      const given = details || {};
      element._error = { code: given.code || 2, message: given.message || 'Media failed to load' };
      element._paused = true;
      element.dispatchEvent(new Event('error'));
      return snapshot(element);
    },
    play() {
      requireActive().play();
      return snapshot(requireActive());
    },
    pause() {
      requireActive().pause();
      return snapshot(requireActive());
    },
  };

  // -------------------------------------------------------- mediaSession recorder
  const recorded = { handlers: Object.create(null), metadata: null, positions: [], calls: [], playbackState: 'none' };
  recorded.call = (action, details) => {
    const handler = recorded.handlers[action];
    if (typeof handler !== 'function') throw new Error('no ' + action + ' handler');
    return handler(details);
  };

  const define = (target, name, descriptor) => {
    try {
      Object.defineProperty(target, name, { configurable: true, ...descriptor });
      return true;
    } catch {
      return false;
    }
  };

  function RecordedMetadata(init) {
    const value = init || {};
    this.title = value.title || '';
    this.artist = value.artist || '';
    this.album = value.album || '';
    this.artwork = value.artwork || [];
  }
  window.MediaMetadata = RecordedMetadata;

  const session = navigator.mediaSession || {};
  const setActionHandler = function (action, handler) {
    recorded.handlers[action] = typeof handler === 'function' ? handler : null;
    recorded.calls.push(action);
  };
  const setPositionState = function (state) {
    recorded.positions.push(state || null);
  };
  if (!define(session, 'setActionHandler', { value: setActionHandler, writable: true })) {
    session.setActionHandler = setActionHandler;
  }
  if (!define(session, 'setPositionState', { value: setPositionState, writable: true })) {
    session.setPositionState = setPositionState;
  }
  define(session, 'metadata', {
    get: () => recorded.metadata,
    set: value => { recorded.metadata = value; },
  });
  define(session, 'playbackState', {
    get: () => recorded.playbackState,
    set: value => { recorded.playbackState = value; },
  });
  if (!navigator.mediaSession) {
    // Some WebKit builds have no MediaSession at all; the app only asks whether
    // it is there and then sets handlers.
    if (!define(navigator, 'mediaSession', { value: session })) {
      define(Object.getPrototypeOf(navigator), 'mediaSession', { get: () => session });
    }
  }

  window.__mediaSession = recorded;

  // ------------------------------------------------------------ test bookkeeping
  const book = { installed: true, external: [], opens: 0, unhandled: [] };
  window.__phoneTest = book;
  window.open = function (url) {
    book.opens += 1;
    if (url) book.external.push(String(url));
    return null;
  };
  window.addEventListener('unhandledrejection', event => {
    const reason = event.reason;
    const text = reason && reason.stack ? reason.stack : String(reason);
    book.unhandled.push(text);
  });
})();
