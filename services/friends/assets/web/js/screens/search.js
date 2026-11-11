// Search across Spotify, SoundCloud and YouTube, as the words are typed.

import { art, empty, h, icon, SOURCE_NAMES } from '../dom.js';
import { load } from '../load.js';
import { current, go } from '../navigation.js';
import { ask } from '../remote.js';
import { card, heading, section, shelf } from '../rows.js';

const RECENT_KEY = 'encore.searches';
/** Typing waits this long for the next letter before it asks. */
const TYPING_MS = 350;
const recentSearches = () => { try { return JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]').filter(item => typeof item === 'string'); } catch { return []; } };
const keepRecent = list => { try { localStorage.setItem(RECENT_KEY, JSON.stringify(list.slice(0, 8))); } catch { /* no room: nothing is lost but the list */ } };
const rememberSearch = query => keepRecent([query, ...recentSearches().filter(old => old !== query)]);

export function searchScreen() {
  const results = h('div', { class: 'results' });
  const input = h('input', { type: 'search', placeholder: 'Songs, artists, people', 'aria-label': 'Search', enterkeyhint: 'search', inputmode: 'search', autocapitalize: 'none', autocorrect: 'off', spellcheck: 'false', value: current.query ?? '' });
  const clear = h('button', { class: 'clear', type: 'button', 'aria-label': 'Clear', hidden: !input.value, onclick: () => {
    input.value = '';
    clear.hidden = true;
    current.query = '';
    asked = '';
    showRecent();
    input.focus();
  } }, icon('close'));
  let asked = '';
  let typing = null;
  const run = (remember = true) => {
    clearTimeout(typing);
    const query = input.value.trim();
    current.query = query;
    if (!query) { asked = ''; showRecent(); return; }
    // The same words again are already on the page, or on their way.
    if (query === asked) return;
    asked = query;
    if (remember) rememberSearch(query);
    load(async () => {
      let found;
      try { found = await ask('search', { query }); } catch (error) { asked = ''; throw error; }
      // Each part says which source it's from, so the chips can show one at a time.
      const part = (source, ...nodes) => h('div', { 'data-source': source }, nodes);
      const parts = [];
      if (found.artists?.length) {
        parts.push(part('spotify', heading('Artists'), shelf(
          found.artists.slice(0, 8).map(artist => card(artist.name, 'Artist', artist.image, () => go({ page: 'artist', id: artist.id }), null, true)))));
      }
      for (const source of ['soundcloud', 'youtube']) {
        const accounts = (found.accounts ?? []).filter(account => account.source === source);
        if (!accounts.length) continue;
        parts.push(part(source, heading(`People on ${SOURCE_NAMES[source]}`), h('div', { class: 'chips' }, accounts.map(account => h('button', { class: 'chip', onclick: () => go({ page: 'list', kind: 'source', uri: account.uri }) },
          art(account.image, ''), h('span', {}, account.name, h('small', { class: account.source, text: SOURCE_NAMES[account.source] })))))));
      }
      for (const source of ['spotify', 'soundcloud', 'youtube']) {
        const songs = section(`On ${SOURCE_NAMES[source]}`, found[source], '');
        if (songs) parts.push(part(source, songs));
      }
      if (!parts.length) return empty(`Nothing for "${query}" on Spotify, SoundCloud or YouTube.`, 'search');
      const shown = new Set(parts.map(node => node.dataset.source));
      const scope = h('div', { class: 'chips scope', role: 'group', 'aria-label': 'Show results from' });
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
    results.removeAttribute('aria-busy');
    if (!recent.length) {
      results.replaceChildren(empty('One search across Spotify, SoundCloud and YouTube.', 'search'));
      return;
    }
    results.replaceChildren(...heading('Recent').filter(Boolean), ...recent.map(query => h('div', { class: 'row recent' },
      h('span', { class: 'art tile' }, icon('clock')),
      h('button', { class: 'words', onclick: () => { input.value = query; clear.hidden = false; run(); } }, h('span', { class: 'name', text: query })),
      h('button', { class: 'more', 'aria-label': `Forget ${query}`, onclick: () => { keepRecent(recentSearches().filter(old => old !== query)); showRecent(); } }, icon('close')))));
  };
  input.addEventListener('input', () => {
    clear.hidden = !input.value;
    clearTimeout(typing);
    if (!input.value.trim()) { current.query = ''; asked = ''; showRecent(); return; }
    // Typed words ask by themselves; only a finished search is remembered.
    typing = setTimeout(() => run(false), TYPING_MS);
  });
  const form = h('form', { class: 'search', role: 'search', onsubmit: event => { event.preventDefault(); input.blur(); run(); } }, icon('search'), input, clear);
  if (current.query) setTimeout(run);
  else showRecent();
  return [h('h1', { class: 'title', text: 'Search' }), form, results];
}
