import assert from 'node:assert/strict';
import test from 'node:test';
import { createRouter, href, match } from '../../assets/web/js/router.js';

/** A History API and window small enough to follow by eye. */
class FakeHistory {
  constructor(win) {
    this.win = win;
    this.entries = [{ state: null, url: `${win.location.pathname}${win.location.search}` }];
    this.at = 0;
    this.hold = false;
    this.held = [];
  }

  get state() { return this.entries[this.at].state; }
  get length() { return this.entries.length; }
  get depth() { return this.at; }

  pushState(state, _title, url) {
    this.entries = this.entries.slice(0, this.at + 1);
    this.entries.push({ state, url });
    this.at = this.entries.length - 1;
    this.win.change(url);
  }

  replaceState(state, _title, url) {
    this.entries[this.at] = { state, url };
    this.win.change(url);
  }

  apply(delta) {
    const to = this.at + delta;
    if (to < 0 || to >= this.entries.length) return false;
    this.at = to;
    this.win.change(this.entries[to].url);
    this.win.emit('popstate', { state: this.entries[to].state });
    return true;
  }

  go(delta) { return this.apply(delta); }
  back() {
    if (this.hold) {
      this.held.push(-1);
      return undefined;
    }
    return this.apply(-1);
  }
  forward() { return this.apply(1); }
  release() {
    const delta = this.held.shift();
    return delta === undefined ? false : this.apply(delta);
  }
}

class FakeWindow {
  constructor(url = '/') {
    this.listeners = new Map();
    this.scrollY = 0;
    this.change(url);
    this.history = new FakeHistory(this);
  }

  change(url) {
    const at = url.indexOf('?');
    if (!this.location) this.location = { pathname: '/', search: '' };
    const path = at < 0 ? url : url.slice(0, at);
    this.location.pathname = path || '/';
    this.location.search = at < 0 ? '' : url.slice(at);
  }

  get pageYOffset() { return this.scrollY; }
  scrollTo(_x, y) { this.scrollY = y; }
  addEventListener(type, fn) {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type).add(fn);
  }
  removeEventListener(type, fn) { this.listeners.get(type)?.delete(fn); }
  emit(type, event) { for (const fn of this.listeners.get(type) ?? []) fn(event); }
}

const opening = (url = '/', options = {}) => {
  const win = new FakeWindow(url);
  const visits = [];
  const router = createRouter({
    window: win,
    onNavigate: (route, info) => visits.push({ route, info }),
    ...options,
  });
  router.start();
  return { win, router, visits };
};

test('known paths match, with decoded params and the query', () => {
  assert.deepEqual(match('/artist/a%20b').params, { id: 'a b' });
  assert.equal(match('/search?q=cats').query.get('q'), 'cats');
  assert.equal(match('/album/abc/').name, 'album');
  assert.equal(match('/not/a/page').fallback, true);
  assert.equal(href('playlist', { id: 'a b' }), '/playlist/a%20b');
});

test('a direct load starts at its own entry', () => {
  const { router } = opening('/artist/a1');
  assert.equal(router.current.name, 'artist');
  assert.equal(router.current.params.id, 'a1');
  assert.equal(router.current.index, 0);
});

test('back from a direct detail page goes home and never exits', () => {
  const { win, router } = opening('/artist/a1');

  router.back();

  assert.equal(router.current.name, 'home');
  assert.equal(win.location.pathname, '/');
  assert.equal(win.history.depth, 0);
  assert.equal(win.history.length, 1);

  router.back();  // already home: still here
  assert.equal(router.current.name, 'home');
  assert.equal(win.history.length, 1);
});

test('a tab root replaces, a detail pushes', () => {
  const { win, router } = opening('/');

  router.go('/library');
  assert.equal(win.history.length, 1);
  assert.equal(win.history.depth, 0);
  assert.equal(router.current.name, 'library');

  router.go('/search');
  assert.equal(win.history.length, 1);
  assert.equal(router.current.name, 'search');

  router.go('/album/a2');
  assert.equal(win.history.length, 2);
  assert.equal(win.history.depth, 1);
  assert.equal(router.current.name, 'album');

  router.back();
  assert.equal(router.current.name, 'search');
  assert.equal(win.history.depth, 0);
});

test('an unknown route goes home', () => {
  const { win, router } = opening('/nope/one/two');
  assert.equal(router.current.name, 'home');
  assert.equal(win.location.pathname, '/');
  assert.equal(win.history.length, 1);

  router.go('/library');
  router.go('/also/not/real');
  assert.equal(router.current.name, 'home');
  assert.equal(win.location.pathname, '/');
});

test('each entry keeps its own scroll, back and forward', () => {
  const { win, router } = opening('/');

  win.scrollY = 120;
  router.go('/album/a2');
  assert.equal(win.scrollY, 0);

  win.scrollY = 40;
  router.back();
  assert.equal(win.scrollY, 120);

  win.history.forward();
  assert.equal(win.scrollY, 40);
});

test('a sheet is a history entry and back closes it first', () => {
  const { win, router } = opening('/library');
  router.go('/album/a2');

  router.openSheet('actions', { uri: 'spotify:track:1' });
  assert.equal(win.history.depth, 2);
  assert.equal(router.current.name, 'album');
  assert.equal(router.current.sheet.name, 'actions');
  assert.deepEqual(router.current.sheet.data, { uri: 'spotify:track:1' });

  router.back();

  assert.equal(win.history.depth, 1);
  assert.equal(router.current.sheet, null);
  assert.equal(router.current.name, 'album');
});

test('navigating from a sheet replaces that entry', () => {
  const { win, router } = opening('/');
  router.openSheet('actions');
  router.go('/album/a2');

  assert.equal(win.history.length, 2);
  assert.equal(win.history.depth, 1);
  assert.equal(router.current.sheet, null);
  assert.equal(router.current.name, 'album');
});

test('a double back walks two entries and still stops at home', () => {
  const { win, router } = opening('/');
  router.go('/library');          // replaces home, still entry 0
  router.go('/album/a2');         // entry 1
  router.go('/queue');            // entry 2

  win.history.hold = true;
  router.back();
  router.back();
  assert.equal(win.history.held.length, 1);  // one step in flight, the second waits

  win.history.release();  // to the album
  assert.equal(router.current.name, 'album');
  assert.equal(win.history.depth, 1);

  win.history.release();  // to the library, the first entry
  assert.equal(router.current.name, 'library');
  assert.equal(win.history.depth, 0);

  // Still inside Encore at the first entry: one more back goes home, not out.
  win.history.hold = false;
  router.back();
  assert.equal(router.current.name, 'home');
  assert.equal(win.history.depth, 0);
});

test('closing a sheet by hand takes back the step it added', () => {
  const { win, router } = opening('/group');
  router.openSheet('invite');
  assert.equal(win.history.depth, 1);

  assert.equal(router.closeSheet(), true);
  assert.equal(win.history.depth, 0);
  assert.equal(router.current.sheet, null);
  assert.equal(router.closeSheet(), false);
});

test('every visit is announced with its screen and scroll', () => {
  const { router, visits } = opening('/');
  router.go('/artist/a1');
  const last = visits.at(-1);

  assert.equal(last.route.name, 'artist');
  assert.equal(last.info.how, 'push');
  assert.equal(last.info.scroll, 0);
});
