import { randomBytes, randomUUID } from 'node:crypto';
import { fail, json, text, body as readBody } from './http.js';
import { MAX_USERS, type Device, type Group, type Member, type Rights, type SharedPlaylist, type State, type User } from './storage.js';

// The group that existed before accounts did. Requests that name no group mean
// this one, so a client from before groups keeps working untouched.
export const FIRST_GROUP = '00000000-0000-4000-8000-000000000001';
export const MAX_GROUPS = 200;
export const MAX_GROUPS_PER_OWNER = 10;
export const MAX_DEVICES = 20;
export const MAX_PENDING = 50;
const UUID = /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/;
// Spotify usernames are short and plain; older ones can be mixed-case words.
const SPOTIFY_ID = /^[\w.-]{1,128}$/;

export const memberRights = (): Rights => ({ share: true, host: true, queue: true, invite: false });
export const fullRights = (): Rights => ({ share: true, host: true, queue: true, invite: true });

export const newInvite = () => randomBytes(12).toString('base64url');

/** A device lasts while it's used. An account keeps at most MAX_DEVICES, and
 *  when it holds too many, the one least recently seen goes — never one still
 *  being used, however long ago it was added. The rows keep their order. */
export function keepDevices(devices: Device[]): Device[] {
  if (devices.length <= MAX_DEVICES) return devices;
  const seen = (device: Device) => device.seen ?? device.created_at;
  const keep = new Set([...devices].sort((a, b) => seen(b) - seen(a)).slice(0, MAX_DEVICES));
  return devices.filter(device => keep.has(device));
}

/** Gives a document from before groups its one group, holding everyone in it. */
export function withGroups(state: State): Group[] {
  if (!state.groups) {
    state.groups = [{
      id: FIRST_GROUP, name: 'Friends', owner_id: null, created_at: 0,
      members: state.users.map(u => ({ user_id: u.user_id, role: 'member' as const, rights: memberRights() })),
      invite: null, pending: [], removed: [],
    }];
  }
  return state.groups;
}

export const groupById = (state: State, id: string) => withGroups(state).find(g => g.id === id);
export const memberOf = (group: Group, user: string) => group.members.find(m => m.user_id === user);
export const manages = (member: Member | undefined) => member?.role === 'owner' || member?.role === 'admin';
export const mayInvite = (member: Member | undefined) => manages(member) || member?.rights.invite === true;

/** The group a request is for, and the reader's place in it. */
export function requestGroup(state: State, header: string | null, user: User): { group: Group; member: Member } {
  const id = header ?? FIRST_GROUP;
  if (!UUID.test(id)) return fail(400, 'Invalid group');
  const group = groupById(state, id) ?? fail(404, 'Group not found');
  const member = memberOf(group, user.user_id) ?? fail(403, 'You are not in this group');
  return { group, member };
}

/** Whoever a departed owner's group falls to: the longest-standing admin, else member. */
function succeed(group: Group) {
  const next = group.members.find(m => m.role === 'admin') ?? group.members[0];
  group.owner_id = next?.user_id ?? null;
  if (next) { next.role = 'owner'; next.rights = fullRights(); }
}

export function removeMember(state: State, group: Group, user: string) {
  group.members = group.members.filter(m => m.user_id !== user);
  if (group.owner_id === user) succeed(group);
  if (!group.members.length && group.id !== FIRST_GROUP) deleteGroup(state, group.id);
}

export function deleteGroup(state: State, id: string): SharedPlaylist[] {
  state.groups = withGroups(state).filter(g => g.id !== id);
  state.posts = state.posts.filter(p => (p.group_id ?? FIRST_GROUP) !== id);
  const gone = (state.playlists ?? []).filter(p => p.group_id === id);
  state.playlists = (state.playlists ?? []).filter(p => p.group_id !== id);
  return gone;
}

/** One person joined from two devices before accounts: everything of `from` becomes `into`'s. */
export function mergeUsers(state: State, from: User, into: User) {
  const swap = (ids: string[]) => [...new Set(ids.map(id => id === from.user_id ? into.user_id : id))];
  for (const post of state.posts) {
    if (post.author_id === from.user_id) post.author_id = into.user_id;
    post.likes = swap(post.likes);
    if (post.to) post.to = swap(post.to);
    if (post.affinity) delete post.affinity[from.user_id];
    for (const reply of post.replies ?? []) if (reply.author_id === from.user_id) reply.author_id = into.user_id;
  }
  for (const playlist of state.playlists ?? []) {
    if (playlist.by === from.user_id) playlist.by = into.user_id;
    if (playlist.to) playlist.to = swap(playlist.to);
  }
  for (const group of withGroups(state)) {
    group.removed = group.removed.filter(id => id !== from.user_id);
    const theirs = memberOf(group, from.user_id);
    if (!theirs) continue;
    const ours = memberOf(group, into.user_id);
    if (ours) {
      // The stronger seat wins: being owner on either device is being owner.
      if (theirs.role === 'owner' || (theirs.role === 'admin' && ours.role === 'member')) { ours.role = theirs.role; ours.rights = theirs.rights; }
      group.members = group.members.filter(m => m !== theirs);
    } else theirs.user_id = into.user_id;
    if (group.owner_id === from.user_id) group.owner_id = into.user_id;
  }
  // Their other computers keep working, as the merged account.
  into.devices = keepDevices([...into.devices ?? [], { id: randomUUID(), hash: from.secret_hash, name: 'Earlier device', created_at: 0 }, ...from.devices ?? []]);
  state.users = state.users.filter(u => u !== from);
}

/** Sign-in brings someone an admin added ahead of time into their groups. */
export function admitPending(state: State, user: User) {
  if (!user.spotify_id) return;
  for (const group of withGroups(state)) {
    if (!group.pending.includes(user.spotify_id)) continue;
    group.pending = group.pending.filter(id => id !== user.spotify_id);
    group.removed = group.removed.filter(id => id !== user.user_id);
    if (!memberOf(group, user.user_id)) group.members.push({ user_id: user.user_id, role: 'member', rights: memberRights() });
  }
}

const deviceView = (device: Device, current: string) => ({ id: device.id, name: device.name, created_at: device.created_at, current: device.hash === current });

export function groupView(group: Group, state: State, reader: string) {
  const me = memberOf(group, reader);
  const names = new Map(state.users.map(u => [u.user_id, u]));
  return {
    id: group.id, name: group.name, owner_id: group.owner_id, role: me?.role ?? 'member', rights: me?.rights ?? memberRights(),
    members: group.members.map(m => {
      const user = names.get(m.user_id);
      return {
        user_id: m.user_id, display_name: user?.display_name ?? 'Former member', role: m.role, rights: m.rights,
        spotify_linked: !!user?.spotify_id, soundcloud: user?.soundcloud?.username ?? null,
      };
    }),
    invite: mayInvite(me) ? group.invite : null,
    pending: manages(me) ? group.pending : [],
  };
}

export function accountView(state: State, user: User, currentHash: string) {
  return {
    user_id: user.user_id, display_name: user.display_name, spotify_id: user.spotify_id ?? null, soundcloud: user.soundcloud ?? null,
    devices: (user.devices ?? []).map(d => deviceView(d, currentHash)),
    groups: withGroups(state).filter(g => memberOf(g, user.user_id)).map(g => groupView(g, state, user.user_id)),
  };
}

function parsedRights(value: unknown, current: Rights): Rights {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return fail(400, 'Invalid rights');
  const raw = value as Record<string, unknown>;
  const pick = (name: keyof Rights) => {
    if (raw[name] === undefined) return current[name];
    if (typeof raw[name] !== 'boolean') return fail(400, 'Invalid rights');
    return raw[name] as boolean;
  };
  return { share: pick('share'), host: pick('host'), queue: pick('queue'), invite: pick('invite') };
}

/** A Spotify username, or the profile link people actually have to hand. */
export function spotifyIdOf(value: unknown): string {
  const raw = text(value, 'spotify_id', 300);
  const link = /^(?:https:\/\/open\.spotify\.com\/(?:intl-[a-z-]+\/)?user\/|spotify:user:)([^/?#]+)/i.exec(raw);
  const id = link ? decodeURIComponent(link[1]) : raw;
  return SPOTIFY_ID.test(id) ? id : fail(400, 'Invalid spotify_id');
}

type Change = <T>(update: (state: State, user: User) => T) => Promise<T>;

/** `/account` and `/groups`. Null when the path is neither. */
export async function groupRoute(request: Request, path: string, user: User, state: State, currentHash: string, change: Change, signal: AbortSignal, isDeploymentInvite: (code: string) => boolean = () => false): Promise<Response | null> {
  const method = request.method;
  const body = () => readBody(request, signal);
  const me = user.user_id;
  if (path === '/account' && method === 'GET') {
    return json(accountView(state, user, currentHash));
  }
  const device = /^\/account\/devices\/([a-f0-9-]{36})$/.exec(path);
  if (device && method === 'DELETE') {
    await change((_s, current) => { current.devices = (current.devices ?? []).filter(d => d.id !== device[1]); });
    return json({ ok: true });
  }
  if (path === '/account/soundcloud' && method === 'PUT') {
    const input = await body();
    const id = text(input.id, 'id', 40);
    const username = text(input.username, 'username', 100);
    await change((_s, current) => { current.soundcloud = { id, username }; });
    return json({ ok: true });
  }
  if (path === '/account/soundcloud' && method === 'DELETE') {
    await change((_s, current) => { delete current.soundcloud; });
    return json({ ok: true });
  }
  if (path === '/account/spotify' && method === 'DELETE') {
    await change((_s, current) => { delete current.spotify_id; });
    return json({ ok: true });
  }
  if (path === '/groups' && method === 'POST') {
    const name = text((await body()).name, 'name', 60);
    return json(await change((s, current) => {
      const groups = withGroups(s);
      if (groups.length >= MAX_GROUPS) return fail(409, 'Too many groups');
      if (groups.filter(g => g.owner_id === current.user_id).length >= MAX_GROUPS_PER_OWNER) return fail(409, 'You own too many groups');
      const created: Group = {
        id: randomUUID(), name, owner_id: current.user_id, created_at: Date.now(),
        members: [{ user_id: current.user_id, role: 'owner', rights: fullRights() }],
        invite: newInvite(), pending: [], removed: [],
      };
      groups.push(created);
      return groupView(created, s, current.user_id);
    }), 201);
  }
  if (path === '/groups/join' && method === 'POST') {
    const code = text((await body()).code, 'code', 256);
    return json(await change((s, current) => {
      // The first group has no code of its own at the start, so the deployment's invite opens it.
      const found = withGroups(s).find(g => g.invite !== null && g.invite === code)
        ?? (isDeploymentInvite(code) ? groupById(s, FIRST_GROUP) : undefined)
        ?? fail(403, 'Invalid invite code');
      if (found.removed.includes(current.user_id)) return fail(403, 'You were removed from this group. Ask its admin to add you back');
      if (found.members.length >= MAX_USERS) return fail(409, 'Group is full');
      if (!memberOf(found, current.user_id)) {
        // Nobody can hand over a group with no owner, so whoever comes back to it holds it.
        const claims = !found.owner_id && !found.members.length;
        found.members.push({ user_id: current.user_id, role: claims ? 'owner' : 'member', rights: claims ? fullRights() : memberRights() });
        if (claims) found.owner_id = current.user_id;
      }
      return groupView(found, s, current.user_id);
    }));
  }
  const match = /^\/groups\/([a-f0-9-]{36})(?:\/(invite|leave|owner|members)(?:\/([a-f0-9-]{36}))?)?$/.exec(path);
  if (!match) return null;
  const [, id, verb, target] = match;
  const open = (s: State, current: User) => {
    const group = groupById(s, id) ?? fail(404, 'Group not found');
    const member = memberOf(group, current.user_id) ?? fail(403, 'You are not in this group');
    return { group, member };
  };
  const managing = (s: State, current: User) => {
    const found = open(s, current);
    if (!manages(found.member)) fail(403, 'Only an admin can do that');
    return found;
  };
  if (!verb && method === 'PATCH') {
    const name = text((await body()).name, 'name', 60);
    return json(await change((s, current) => {
      const { group } = managing(s, current);
      group.name = name;
      return groupView(group, s, me);
    }));
  }
  if (!verb && method === 'DELETE') {
    const gone = await change((s, current) => {
      const { group, member } = open(s, current);
      if (member.role !== 'owner') return fail(403, 'Only the owner can delete the group');
      if (group.id === FIRST_GROUP) return fail(409, 'This group cannot be deleted');
      return deleteGroup(s, group.id).map(p => p.id);
    });
    return json({ ok: true, playlists: gone });
  }
  if (verb === 'invite' && !target && method === 'POST') {
    return json(await change((s, current) => {
      const { group, member } = open(s, current);
      if (!mayInvite(member)) return fail(403, 'You cannot make invite codes in this group');
      group.invite = newInvite();
      return groupView(group, s, me);
    }));
  }
  if (verb === 'invite' && !target && method === 'DELETE') {
    return json(await change((s, current) => {
      const { group } = managing(s, current);
      group.invite = null;
      return groupView(group, s, me);
    }));
  }
  if (verb === 'leave' && !target && method === 'POST') {
    await change((s, current) => {
      const { group } = open(s, current);
      removeMember(s, group, current.user_id);
    });
    return json({ ok: true });
  }
  if (verb === 'owner' && !target && method === 'POST') {
    const next = text((await body()).user_id, 'user_id', 36);
    return json(await change((s, current) => {
      const { group, member } = open(s, current);
      if (member.role !== 'owner') return fail(403, 'Only the owner can hand the group over');
      const heir = memberOf(group, next) ?? fail(404, 'Member not found');
      member.role = 'admin';
      heir.role = 'owner';
      heir.rights = fullRights();
      group.owner_id = heir.user_id;
      return groupView(group, s, me);
    }));
  }
  if (verb === 'members' && !target && method === 'POST') {
    const spotify = spotifyIdOf((await body()).spotify_id);
    return json(await change((s, current) => {
      const { group } = managing(s, current);
      const known = s.users.find(u => u.spotify_id === spotify);
      if (known) {
        group.removed = group.removed.filter(u => u !== known.user_id);
        if (!memberOf(group, known.user_id)) group.members.push({ user_id: known.user_id, role: 'member', rights: memberRights() });
      } else if (!group.pending.includes(spotify)) {
        if (group.pending.length >= MAX_PENDING) return fail(409, 'Too many people waiting to sign in');
        group.pending.push(spotify);
      }
      return groupView(group, s, me);
    }), 201);
  }
  if (verb === 'members' && !target && method === 'DELETE') {
    // Taking back someone added ahead of time, who has not signed in yet.
    const spotify = spotifyIdOf((await body()).spotify_id);
    return json(await change((s, current) => {
      const { group } = managing(s, current);
      group.pending = group.pending.filter(pending => pending !== spotify);
      return groupView(group, s, me);
    }));
  }
  if (verb === 'members' && target && method === 'PATCH') {
    const input = await body();
    return json(await change((s, current) => {
      const { group, member } = managing(s, current);
      const other = memberOf(group, target) ?? fail(404, 'Member not found');
      if (other.role === 'owner') return fail(403, 'The owner keeps every right');
      if (input.role !== undefined) {
        if (input.role !== 'admin' && input.role !== 'member') return fail(400, 'Invalid role');
        // An admin may not make peers; handing out that power is the owner's.
        if (member.role !== 'owner') return fail(403, 'Only the owner can change who is an admin');
        other.role = input.role;
      }
      if (input.rights !== undefined) {
        if (other.role === 'admin' && member.role !== 'owner') return fail(403, 'Only the owner can change an admin');
        other.rights = parsedRights(input.rights, other.rights);
      }
      return groupView(group, s, me);
    }));
  }
  if (verb === 'members' && target && method === 'DELETE') {
    return json(await change((s, current) => {
      const { group, member } = managing(s, current);
      const other = memberOf(group, target) ?? fail(404, 'Member not found');
      if (other.role === 'owner') return fail(403, 'The owner cannot be removed');
      if (other.role === 'admin' && member.role !== 'owner') return fail(403, 'Only the owner can remove an admin');
      if (other.user_id === current.user_id) return fail(400, 'Leave the group instead');
      group.members = group.members.filter(m => m !== other);
      // An old invite must not walk them straight back in.
      if (!group.removed.includes(other.user_id)) group.removed.push(other.user_id);
      return groupView(group, s, me);
    }));
  }
  return null;
}
