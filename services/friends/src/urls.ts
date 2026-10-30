// Shared by request validation and persisted-row validation. No URL is fetched.
export function httpsUrl(value: unknown): URL | null {
  if (typeof value !== 'string' || !value.trim() || value.length > 4096 || /[\x00-\x1f\x7f]/.test(value) || /%0[ad]/i.test(value)) return null;
  try {
    const url = new URL(value.trim());
    return url.protocol === 'https:' && !url.username && !url.password && !url.port ? url : null;
  } catch { return null; }
}

export function sourceUrl(value: unknown, source: unknown): string | null {
  const url = httpsUrl(value);
  if (!url) return null;
  const parts = url.pathname.split('/').filter(Boolean);
  const id = (value?: string) => !!value && /^[a-zA-Z0-9_-]+$/.test(value);
  let valid = false;
  if (source === 'spotify') {
    if (parts[0]?.startsWith('intl-')) parts.shift();
    valid = url.hostname === 'open.spotify.com' && parts.length === 2 && ['track', 'album', 'playlist'].includes(parts[0]) && /^[a-zA-Z0-9]+$/.test(parts[1]);
  } else if (source === 'soundcloud') {
    const reserved = ['likes', 'reposts', 'tracks', 'albums', 'sets', 'popular-tracks'];
    valid = url.hostname === 'on.soundcloud.com' ? parts.length === 1 && id(parts[0]) :
      ['soundcloud.com', 'www.soundcloud.com', 'm.soundcloud.com'].includes(url.hostname) &&
      parts.every(id) && !reserved.includes(parts[1]) && (parts.length === 2 || parts.length === 3 && /^s-[\w-]+$/.test(parts[2]));
    if (['soundcloud.com', 'www.soundcloud.com', 'm.soundcloud.com'].includes(url.hostname) && parts[1] === 'sets') {
      valid = parts.every(id) && (parts.length === 3 || parts.length === 4 && /^s-[\w-]+$/.test(parts[3]));
    }
  } else if (source === 'youtube') {
    const video = url.hostname === 'youtu.be' && parts.length === 1 ? parts[0] :
      ['youtube.com', 'www.youtube.com', 'music.youtube.com'].includes(url.hostname) && url.pathname === '/watch' ? url.searchParams.get('v') : null;
    valid = typeof video === 'string' && /^[\w-]{11}$/.test(video);
  }
  if (!valid) return null;
  url.hash = '';
  return url.toString();
}
