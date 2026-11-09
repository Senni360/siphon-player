// Search across Spotify, SoundCloud and YouTube.

import { art, h, icon, SOURCE_NAMES } from '../dom.js';
import { load } from '../load.js';
import { current, go } from '../navigation.js';
import { ask } from '../remote.js';
import { card, section } from '../rows.js';

const RECENT_KEY = 'encore.searches';
const recentSearches = () => { try { return JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]'); } catch { return []; } };
function rememberSearch(query) {
  localStorage.setItem(RECENT_KEY, JSON.stringify([query, ...recentSearches().filter(old => old !== query)].slice(0, 8)));
}

export function searchScreen() {
  const results = h('div');
  const input = h('input', { type: 'search', placeholder: 'Spotify, SoundCloud and YouTube', 'aria-label': 'Search', enterkeyhint: 'search', autocapitalize: 'off', autocorrect: 'off', value: current.query ?? '' });
  const run = () => {
    const query = input.value.trim();
    current.query = query;
    if (!query) { showRecent(); return; }
    rememberSearch(query);
    load(async () => {
      const found = await ask('search', { query });
      // Each part says which source it's from, so the chips can show one at a time.
      const part = (source, ...nodes) => h('div', { 'data-source': source }, nodes);
      const parts = [];
      if (found.artists?.length) {
        parts.push(part('spotify', h('h2', { class: 'section', text: 'Artists' }), h('div', { class: 'cards' },
          found.artists.slice(0, 4).map(artist => card(artist.name, 'Artist', artist.image, () => go({ page: 'artist', id: artist.id }))))));
      }
      for (const source of ['soundcloud', 'youtube']) {
        const accounts = (found.accounts ?? []).filter(account => account.source === source);
        if (!accounts.length) continue;
        parts.push(part(source, h('h2', { class: 'section', text: `People on ${SOURCE_NAMES[source]}` }), h('div', { class: 'chips' }, accounts.map(account => h('button', { class: 'chip', onclick: () => go({ page: 'list', kind: 'source', uri: account.uri }) },
          art(account.image, ''), h('span', {}, account.name, h('small', { class: account.source, text: SOURCE_NAMES[account.source] })))))));
      }
      for (const source of ['spotify', 'soundcloud', 'youtube']) {
        const songs = section(`On ${SOURCE_NAMES[source]}`, found[source], '');
        if (songs) parts.push(part(source, songs));
      }
      if (!parts.length) return h('p', { class: 'empty', text: `Nothing found for "${query}".` });
      const shown = new Set(parts.map(node => node.dataset.source));
      const scope = h('div', { class: 'chips scope' });
      const pick = wanted => {
        current.scope = wanted;
        scope.querySelectorAll('button').forEach(button => button.setAttribute('aria-pressed', String(button.dataset.scope === wanted)));
        parts.forEach(node => { node.hidden = wanted !== 'all' && node.dataset.source !== wanted; });
      };
      scope.append(...['all', 'spotify', 'soundcloud', 'youtube'].filter(source => source === 'all' || shown.has(source))
        .map(source => h('button', { class: 'chip plain', 'data-scope': source, text: source === 'all' ? 'All' : SOURCE_NAMES[source], onclick: () => pick(source) })));
      pick(shown.has(current.scope) ? current.scope : 'all');
      return [shown.size > 1 ? scope : null, ...parts];
    }, results);
  };
  const showRecent = () => {
    const recent = recentSearches();
    results.loading = null;
    results.replaceChildren(...(recent.length ? [h('h2', { class: 'section', text: 'Recent' }), h('div', { class: 'chips' }, recent.map(query => h('button', { class: 'chip plain', text: query, onclick: () => { input.value = query; run(); } })))] : []));
  };
  input.addEventListener('input', () => { if (!input.value.trim()) showRecent(); });
  const form = h('form', { class: 'search', role: 'search', onsubmit: event => { event.preventDefault(); input.blur(); run(); } }, icon('search'), input);
  if (current.query) setTimeout(run);
  else showRecent();
  return [h('h1', { class: 'title', text: 'Search' }), form, results];
}
