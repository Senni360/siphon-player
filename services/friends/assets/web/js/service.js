// Talking to the group service.

import { STORE, authHeaders, forgetSecret, removedDevice } from './auth.js';
import { SignedOut } from './errors.js';

export async function api(path, method = 'GET', body) {
  const secret = localStorage.getItem(STORE.secret);
  const group = localStorage.getItem(STORE.group);
  const answer = await fetch(`/api${path}`, {
    method,
    headers: {
      ...(body !== undefined ? { 'content-type': 'application/json' } : {}),
      ...authHeaders(secret, group),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  // A 401 might mean this device was removed — or nothing of the sort. Only
  // the service's own fresh, repeated refusal about this very secret signs
  // the phone out; never a first word on its own.
  if (answer.status === 401 && secret) {
    const gone = await removedDevice(answer, true, () => fetch('/api/me', { headers: authHeaders(secret, group) }));
    if (gone) {
      forgetSecret();
      throw new SignedOut();
    }
    throw new Error("Encore couldn't be reached for a moment.");
  }
  const value = await answer.json().catch(() => ({}));
  if (!answer.ok) throw new Error(value.error || 'Something went wrong.');
  return value;
}
