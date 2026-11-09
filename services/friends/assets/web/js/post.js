// Shared songs in the group: a post, and a link that plays on the computer.

import { art, h, icon, toast } from './dom.js';
import { act } from './remote.js';
import { api } from './service.js';
import { player } from './state.js';

export function playShared(url) {
  // The phone never leaves the page to play: the computer takes the link from here.
  if (!player.online) { toast('Shared links play through Encore on your computer.'); return; }
  act('play_link', { url }).catch(() => toast("Couldn't reach Encore."));
  toast('Starting on your computer');
}

export function postView(post, refresh) {
  const like = h('button', { 'aria-pressed': String(!!post.liked), 'aria-label': 'Heart', onclick: async () => {
    post.liked = !post.liked;
    post.like_count += post.liked ? 1 : -1;
    like.setAttribute('aria-pressed', String(post.liked));
    likes.textContent = post.like_count || '';
    try {
      await api(`/posts/${post.id}/like`, post.liked ? 'PUT' : 'DELETE');
    } catch {
      post.liked = !post.liked;
      post.like_count += post.liked ? 1 : -1;
      like.setAttribute('aria-pressed', String(post.liked));
      likes.textContent = post.like_count || '';
      toast("That didn't go through.");
    }
  } }, icon('heart'));
  const likes = h('span', { text: post.like_count || '' });
  like.append(likes);
  const replies = h('div', {}, (post.replies ?? []).map(reply => h('p', { class: 'reply' }, h('b', { text: `${reply.author_name ?? 'Someone'} ` }), reply.text)));
  const input = h('input', { type: 'text', placeholder: 'Reply', 'aria-label': 'Reply', maxlength: 500, enterkeyhint: 'send' });
  const form = h('form', { class: 'send', hidden: true, onsubmit: async event => {
    event.preventDefault();
    const text = input.value.trim();
    if (!text) return;
    input.value = '';
    replies.append(h('p', { class: 'reply' }, h('b', { text: 'You ' }), text));
    try { await api(`/posts/${post.id}/replies`, 'POST', { text }); } catch { toast("That reply didn't go through."); refresh(); }
  } }, input);
  const song = { uri: `link:${post.url}`, name: post.title, artists: post.artists, image: post.artwork_url, duration_ms: post.duration_ms };
  const play = () => playShared(post.url);
  return h('article', { class: 'post' },
    h('p', { class: 'who' }, h('b', { text: post.author_name }), ' shared'),
    post.note ? h('p', { class: 'said', text: post.note }) : null,
    h('div', { class: 'row' }, art(song.image), h('button', { class: 'words', onclick: play },
      h('span', { class: `name ${post.source}`, text: post.title }), h('span', { class: 'by', text: post.artists.join(', ') }))),
    replies, form,
    h('div', { class: 'tools' }, like, h('button', { 'aria-label': 'Reply', onclick: () => { form.hidden = false; input.focus(); } }, icon('reply'), h('span', { text: (post.replies ?? []).length || '' }))));
}
