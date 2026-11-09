// Keeping this phone's sign-in, and the one decision that ends it.

export const STORE = { secret: 'encore.secret', name: 'encore.name', group: 'encore.group', client: 'encore.client', spotify: 'encore.spotify' };

// [signout-begin]
/**
 * Whether a refused request means this device really was removed. Only the
 * service's own word counts, and only after being asked again about this very
 * secret: a transient failure — the network, a timeout, 5xx, 429, a storage
 * blip, a proxy's or another site's 401 page, Spotify refusing its own token —
 * never signs the phone out. `probe` asks the service once more whether this
 * secret still works.
 */
async function removedDevice(answer, attached, probe) {
  if (!attached || answer.status !== 401) return false;
  const said = await answer.json().catch(() => null);
  if (said?.error !== 'Sign in to the group') return false;
  try {
    const again = await probe();
    const confirm = await again.json().catch(() => null);
    return again.status === 401 && confirm?.error === 'Sign in to the group';
  } catch {
    return false;  // asking again said nothing for certain: hold the sign-in.
  }
}
// [signout-end]
export { removedDevice };

// iOS quietly puts a site's localStorage away after a while of not opening it,
// unless that site is on the home screen. So the secret also lives in a
// first-party cookie only this address can send, and an emptied localStorage
// fills back from it. The cookie can't be HttpOnly — the page itself must read
// the secret to send it, and the service only ever stores hashes, so it cannot
// set this cookie for us. It stays narrow: Secure, SameSite=Strict, Path=/.
const SECRET_COOKIE = 'encore.device';
const SECRET_MAX_AGE = 90 * 86400;  // slides forward: rewritten every time the app opens
export function keepSecret(secret) {
  localStorage.setItem(STORE.secret, secret);
  document.cookie = `${SECRET_COOKIE}=${secret}; Path=/; Max-Age=${SECRET_MAX_AGE}; SameSite=Strict; Secure`;
}
export function forgetSecret() {
  localStorage.removeItem(STORE.secret);
  document.cookie = `${SECRET_COOKIE}=; Path=/; Max-Age=0; SameSite=Strict; Secure`;
}
/** The secret from the backup cookie, if the browser kept it and nothing else. */
export function rememberedSecret() {
  const held = (document.cookie.split(';').map(part => part.trim()).find(part => part.startsWith(`${SECRET_COOKIE}=`)) ?? '').slice(SECRET_COOKIE.length + 1);
  return /^[a-f0-9]{64}$/.test(held) ? held : null;
}
// Where the browser offers it, asking to keep this site's storage slows the
// quiet clearing of it further. An installed app asks for good, once.
export function keepStorage() {
  try { navigator.storage?.persist?.()?.catch?.(() => {}); } catch { /* an old browser */ }
}

export const authHeaders = (secret, group) => ({
  ...(secret ? { authorization: `Bearer ${secret}` } : {}),
  ...(group ? { 'x-siphon-group': group } : {}),
});

export const phoneName = () => (/iPhone/.test(navigator.userAgent) ? 'iPhone' : /Android/.test(navigator.userAgent) ? 'Android phone' : 'Phone');
