// The group: who is listening, shared playlists, and the feed.

import { STORE } from '../auth.js';
import { art, empty, h } from '../dom.js';
import { load } from '../load.js';
import { go } from '../navigation.js';
import { postView, playShared } from '../post.js';
import { api } from '../service.js';
import { card, heading, shelf } from '../rows.js';

export function groupScreen() {
  const body = h('div');
  const title = h('h1', { class: 'title', text: 'Group' });
  const chooser = h('div');
  const refresh = () => load(async () => {
    const [account, feed, live, people, lists] = await Promise.all([api('/account'), api('/posts?limit=30'), api('/live').catch(() => null), api('/members').catch(() => null), api('/playlists').catch(() => null)]);
    const groups = account.groups ?? [];
    const chosen = localStorage.getItem(STORE.group);
    const chosenGroup = groups.find(group => group.id === chosen) ?? groups[0];
    if (chosenGroup) title.textContent = chosenGroup.name;
    chooser.replaceChildren(...(groups.length > 1 ? [h('div', { class: 'chips scope', role: 'group', 'aria-label': 'Group' }, groups.map(group => h('button', { class: 'chip plain', 'aria-pressed': String(group.id === chosenGroup?.id), text: group.name, onclick: () => {
      localStorage.setItem(STORE.group, group.id);
      refresh();
    } })))] : []));
    const posts = feed.posts ?? [];
    const names = new Map((people?.members ?? []).map(person => [person.user_id, person.display_name]));
    const listening = (live?.presence ?? []).filter(someone => someone.item && someone.playing && someone.user_id !== account.user_id);
    return [
      listening.length ? [heading('Listening now'), listening.map(someone => h('div', { class: 'row live' }, art(someone.item.artwork_url),
        h('button', { class: 'words', onclick: () => playShared(someone.item.url) },
          h('span', { class: `name ${someone.item.source}`, text: someone.item.title }),
          h('span', { class: 'by', text: `${names.get(someone.user_id) ?? 'Someone'} · ${(someone.item.artists ?? []).join(', ')}` })),
        h('span', { class: 'live-dot', 'aria-hidden': 'true' })))] : null,
      lists?.playlists?.length ? [heading('Shared playlists'), shelf(lists.playlists.slice(0, 10).map(list => card(list.name, `${list.by_name} · ${list.count} songs`, list.artwork_url, () => go({ page: 'shared', id: list.id, name: list.name }))))] : null,
      posts.length ? [listening.length || lists?.playlists?.length ? heading('Shared songs') : null, h('div', { class: 'feed' }, posts.map(post => postView(post, refresh)))]
        : empty('Nobody has shared anything yet. Hold a song and choose Share with the group.', 'group'),
    ];
  }, body);
  refresh();
  return [title, chooser, body];
}
