// A small stack router over the History API.
//
// Tab roots replace the current entry; details push a new one. A sheet is a
// history entry of its own, so Back closes it before it leaves the page. Every
// entry remembers where the page was scrolled to. A direct load of any URL
// works: the router marks it as its own first entry, so Back from a detail
// page goes home instead of leaving Encore.
//
// This file only decides what screen is current. It does not draw anything.

/** The screens the phone knows. Tab roots are the ones the bottom bar switches. */
export const ROUTES = Object.freeze([
  { name: 'home', path: '/', tab: true },
  { name: 'search', path: '/search', tab: true },
  { name: 'library', path: '/library', tab: true },
  { name: 'group', path: '/group', tab: true },
  { name: 'artist', path: '/artist/:id' },
  { name: 'album', path: '/album/:id' },
  { name: 'playlist', path: '/playlist/:id' },
  { name: 'liked', path: '/liked' },
  { name: 'settings', path: '/settings' },
  { name: 'queue', path: '/queue' },
  { name: 'now', path: '/now' },
]);

export const ROUTER_KEY = 'encore';

const HOME = ROUTES[0];
const decode = part => {
  try {
    return decodeURIComponent(part);
  } catch {
    return part;
  }
};

const splitUrl = url => {
  const text = String(url ?? '/');
  const at = text.indexOf('?');
  return at < 0 ? { path: text, search: '' } : { path: text.slice(0, at), search: text.slice(at) };
};

const homeRoute = (routes = ROUTES) => {
  const found = routes.find(route => route.name === 'home') ?? HOME;
  return { name: found.name, params: {}, path: '/', search: '', query: new URLSearchParams(), tab: !!found.tab, fallback: false };
};

/**
 * Match a path (with or without a query) against a route table. An unknown
 * path matches home and says so with `fallback: true`.
 */
export function match(url, routes = ROUTES) {
  const { path: raw, search } = splitUrl(url);
  let path = raw.startsWith('/') ? raw : `/${raw}`;
  if (path.length > 1) path = path.replace(/\/+$/, '') || '/';
  const parts = path.split('/').filter(Boolean).map(decode);

  for (const route of routes) {
    const pieces = route.path.split('/').filter(Boolean);
    if (pieces.length !== parts.length) continue;
    const params = {};
    let fits = true;
    for (let at = 0; at < pieces.length; at++) {
      const piece = pieces[at];
      if (piece.startsWith(':')) params[piece.slice(1)] = parts[at];
      else if (piece !== parts[at]) {
        fits = false;
        break;
      }
    }
    if (!fits) continue;
    return {
      name: route.name,
      params,
      path,
      search,
      query: new URLSearchParams(search.slice(1)),
      tab: !!route.tab,
      fallback: false,
    };
  }
  return { ...homeRoute(routes), fallback: true };
}

/** Build a path for a named route. Missing params become empty and encode safely. */
export function href(name, params = {}, routes = ROUTES) {
  const route = routes.find(candidate => candidate.name === name);
  if (!route) return '/';
  return route.path.replace(/:([A-Za-z0-9_]+)/g, (_, key) => encodeURIComponent(String(params[key] ?? '')));
}

const metaOf = state => (
  state && typeof state === 'object' && state[ROUTER_KEY] && typeof state[ROUTER_KEY] === 'object'
    ? state[ROUTER_KEY]
    : null
);

const mergeState = (existing, meta, extra) => ({
  ...(existing && typeof existing === 'object' ? existing : {}),
  ...(extra && typeof extra === 'object' ? extra : {}),
  [ROUTER_KEY]: meta,
});

const urlOf = route => `${route.path}${route.search ?? ''}`;

/**
 * @param {object} [options]
 * @param {Window} [options.window] The window to route. Defaults to the global.
 * @param {History} [options.history] Defaults to `window.history`.
 * @param {Location} [options.location] Defaults to `window.location`.
 * @param {(route: object, info: {how: string, scroll: number}) => void} [options.onNavigate]
 * @param {{read: () => number, write: (y: number) => void}} [options.scroll]
 * @param {string[]} [options.roots] Names of the tab roots that replace.
 * @param {Array<{name: string, path: string, tab?: boolean}>} [options.routes]
 */
export function createRouter({
  window: win = globalThis.window,
  history = win?.history,
  location = win?.location,
  onNavigate = () => {},
  scroll = null,
  roots = null,
  routes = ROUTES,
} = {}) {
  if (!history || !location) throw new TypeError('createRouter needs a window with history and location');

  const table = routes;
  const tabs = new Set(roots ?? table.filter(route => route.tab).map(route => route.name));
  const positions = new Map();
  const readScroll = scroll?.read ?? (() => Number(win?.scrollY ?? win?.pageYOffset ?? 0) || 0);
  const writeScroll = scroll?.write ?? (y => win?.scrollTo?.(0, y));

  let current = null;
  let started = false;
  let counter = 0;
  let queuedBacks = 0;

  const nextKey = () => `entry-${++counter}`;

  const remember = () => {
    if (current) positions.set(current.key, readScroll());
  };

  const tell = how => {
    if (!current) return;
    const y = positions.get(current.key) ?? 0;
    onNavigate(current, { how, scroll: y });
    writeScroll(y);
  };

  const stepBack = () => {
    if (!current) return;
    if (current.index <= 0) {
      queuedBacks = 0;
      go('/', { replace: true });
      return;
    }
    history.back();
  };

  const onPop = event => {
    const found = match(`${location.pathname}${location.search}`, table);
    const meta = metaOf(event?.state ?? history.state);
    remember();

    let index = Number.isInteger(meta?.index) ? Math.max(0, meta.index) : 0;
    let key = typeof meta?.key === 'string' && meta.key ? meta.key : nextKey();
    let sheet = meta?.sheet ?? null;
    let target = found;

    if (found.fallback) {
      // A state we did not write, or a URL we do not know: start again at home
      // rather than trust whatever is at the top of the stack.
      target = match('/', table);
      index = 0;
      key = nextKey();
      sheet = null;
      history.replaceState(mergeState(event?.state ?? history.state, { index, key, path: '/', sheet }), '', '/');
    }

    current = { ...target, index, key, sheet };
    tell('pop');

    if (queuedBacks > 0) {
      queuedBacks -= 1;
      if (queuedBacks > 0) stepBack();
    }
  };

  function start() {
    if (started) return current;
    started = true;
    win?.addEventListener?.('popstate', onPop);

    let found = match(`${location.pathname}${location.search}`, table);
    const meta = metaOf(history.state);
    const index = Number.isInteger(meta?.index) ? Math.max(0, meta.index) : 0;
    const key = typeof meta?.key === 'string' && meta.key ? meta.key : nextKey();
    const sheet = meta?.sheet ?? null;
    const fallback = found.fallback;
    if (fallback) found = match('/', table);

    const state = mergeState(history.state, { index, key, path: found.path, sheet });
    history.replaceState(state, '', fallback ? '/' : urlOf(found));

    current = { ...found, index, key, sheet };
    tell('start');
    return current;
  }

  /**
   * Go to a path. Tab roots and sheet exits replace; details push. An unknown
   * path goes home. Going where we already are does nothing.
   */
  function go(target, options = {}) {
    if (!started) start();
    const found = match(target, table);
    const route = found.fallback ? match('/', table) : found;
    const url = found.fallback ? '/' : urlOf(route);

    if (current && !current.sheet && urlOf(current) === url) return current;

    const replace = options.replace ?? (current?.sheet ? true : tabs.has(route.name));
    remember();
    const index = replace ? (current?.index ?? 0) : (current?.index ?? -1) + 1;
    const key = nextKey();
    const meta = { index, key, path: route.path, sheet: null };
    const state = mergeState(history.state, meta, options.state);
    if (replace) history.replaceState(state, '', url);
    else history.pushState(state, '', url);

    current = { ...route, index, key, sheet: null };
    tell(replace ? 'replace' : 'push');
    return current;
  }

  /** Go home without ever leaving Encore. */
  function goHome() {
    return go('/', { replace: true });
  }

  /**
   * Back one step. At the first entry it goes home instead of out of the app.
   * Presses that arrive before the browser has moved are queued, so a double
   * back walks two entries and still stops at home.
   */
  function back() {
    if (!started) start();
    if (!current) return;
    if (queuedBacks === 0 && current.index <= 0) {
      goHome();
      return;
    }
    queuedBacks += 1;
    if (queuedBacks === 1) stepBack();
  }

  /** A sheet is its own history entry; Back closes it before the page goes. */
  function openSheet(name, data = {}) {
    if (!started) start();
    if (!current) return null;
    remember();
    const index = current.index + 1;
    const key = nextKey();
    const sheet = { name, data };
    const meta = { index, key, path: current.path, sheet };
    history.pushState(mergeState(history.state, meta), '', urlOf(current));
    current = { ...current, index, key, sheet };
    tell('sheet');
    return current;
  }

  function closeSheet() {
    if (!current?.sheet) return false;
    back();
    return true;
  }

  return {
    start,
    go,
    goHome,
    back,
    openSheet,
    closeSheet,
    href: (name, params) => href(name, params, table),
    /** Put the current entry's saved scroll back, after content has rendered. */
    restoreScroll() {
      if (current) writeScroll(positions.get(current.key) ?? 0);
    },
    get current() {
      return current;
    },
    get canGoBack() {
      return !!current && (queuedBacks > 0 || current.index > 0 || current.path !== '/');
    },
    stop() {
      win?.removeEventListener?.('popstate', onPop);
      started = false;
    },
  };
}
