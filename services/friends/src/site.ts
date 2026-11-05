// The two pages a person can land on from a browser: where to get Encore, and
// the door a party invite opens. They hold no group data, so nothing here reads
// storage: a party link names ids only someone already holding the link knows.

const RELEASES = 'https://github.com/Senni360/siphon-player/releases';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const STYLE = `
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  body { margin: 0; min-height: 100vh; display: grid; place-items: center; padding: 24px;
    background: #0b0b0c; color: #ededef;
    font: 15px/1.5 Inter, "Segoe UI", system-ui, sans-serif; }
  main { width: 100%; max-width: 440px; padding: 32px; background: #121214;
    border: 1px solid #232327; border-radius: 14px; }
  h1 { margin: 0 0 8px; font-size: 26px; font-weight: 600; letter-spacing: -0.01em; }
  p { margin: 0 0 20px; color: #a1a1aa; }
  a.primary { display: inline-block; padding: 10px 18px; border-radius: 999px;
    background: #fafafa; color: #09090b; font-weight: 500; text-decoration: none; }
  a.primary:hover { background: #e4e4e7; }
  a.primary:focus-visible, small a:focus-visible { outline: 2px solid #fafafa; outline-offset: 2px; }
  small { display: block; margin-top: 20px; color: #8a8a93; font-size: 13px; }
  small a { color: #ededef; }
`;

function page(title: string, head: string, body: string): string {
  return `<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>${title}</title>${head}<style>${STYLE}</style></head>
<body><main>${body}</main></body></html>`;
}

const home = () => page('Encore', '', `<h1>Encore</h1>
<p>A private music app for friends. One library and one queue for Spotify, SoundCloud and YouTube, shared inside your group. Windows only.</p>
<a class="primary" href="/download">Download for Windows</a>
<small><a href="/privacy">Privacy</a><br>Extract the zip and open <b>siphon.exe</b>. If Windows warns you, choose More info, then Run anyway. Encore then shows you how to sign in with your own free Spotify app.</small>`);

const privacy = () => page('Encore privacy', '', `<h1>Privacy</h1>
<p>Encore is a desktop music app that runs on your computer. It has no ads, analytics or telemetry, and nothing here is sold.</p>
<p><b>On your computer.</b> Your settings, library copies and listening history stay on your computer. Sign-ins are kept in Windows' credential store.</p>
<p><b>Spotify, SoundCloud and YouTube.</b> Encore talks to these services directly, with the accounts you connect, to browse and play music. Your data goes between your computer and that service only.</p>
<p><b>Your Google account.</b> Connecting YouTube is optional and uses a Google project of your own. Encore reads and changes your YouTube subscriptions and likes when you ask it to or when they sync. Your Client ID, secret and sign-in stay on your computer, and your YouTube account data isn't sent to this site or to anyone but Google. You can disconnect in Encore, and remove its access at any time at <a href="https://myaccount.google.com/permissions">your Google account's permissions</a>. Encore's use of Google data follows the <a href="https://developers.google.com/terms/api-services-user-data-policy">Google API Services User Data Policy</a>.</p>
<p><b>Your phone.</b> If you link a phone, what it asks of Encore on your computer passes through this site's service on its way: the commands, your computer's current song, and the search results and playlists it sends back. They last about a minute and aren't logged or kept. The link is yours to remove on your computer at any time. If you also sign in to Spotify on your phone, that sign-in stays in your phone's browser and goes only to Spotify; this site sees it once, to confirm which account you are, and doesn't keep it.</p>
<p><b>Groups.</b> Only if you join a group, this site's service keeps what the group needs: who you are in it, what you post and share, and what you're listening to if you choose to share that. Only your group sees it. Leave out sharing and nothing about your listening is sent.</p>
<a class="primary" href="/">Back to Encore</a>
<small>Questions: <a href="https://github.com/Senni360/siphon-player/issues">open an issue on GitHub</a>.</small>`);

function party(group: string, id: string): string {
  const door = `siphon://party/${group.toLowerCase()}/${id.toLowerCase()}`;
  return page('Join the party', `<meta http-equiv="refresh" content="0;url=${door}">`, `<h1>Join the party</h1>
<p>Encore should be opening. If it doesn't, press the button.</p>
<a class="primary" href="${door}">Open in Encore</a>
<small>Don't have it yet? <a href="/download">Download Encore</a>, then open this link again.</small>`);
}

/** The page for a browser's request, or null when it is the service's to answer. */
export function sitePage(method: string | undefined, url: string | undefined): { status: number; html: string } | null {
  if (method !== 'GET' && method !== 'HEAD') return null;
  const path = new URL(url ?? '/', 'http://localhost').pathname.replace(/\/$/, '');
  if (path === '') return { status: 200, html: home() };
  if (path === '/privacy') return { status: 200, html: privacy() };
  const parts = path.split('/').slice(1);
  if (parts[0] !== 'party') return null;
  if (parts.length === 3 && UUID.test(parts[1]) && UUID.test(parts[2])) return { status: 200, html: party(parts[1], parts[2]) };
  return { status: 404, html: page('Encore', '', `<h1>That link isn't right</h1>
<p>Ask for a fresh invite from whoever sent it.</p>
<a class="primary" href="/">Go to Encore</a>`) };
}

const RELEASES_API = 'https://api.github.com/repos/Senni360/siphon-player/releases?per_page=20';
const ZIP = /^siphon-v[^/]+-x86_64-pc-windows-msvc\.zip$/;
const REMEMBER_MS = 5 * 60 * 1000;
let newest: { url: string; until: number } | null = null;

/** Forgets what was found, between tests. */
export function forgetNewestRelease() { newest = null; }

/**
 * Where the download button goes: the newest release's zip, previews included,
 * as the app's own updater sees them. GitHub's releases page is the way there
 * if it can't be asked. Remembered for a while, since GitHub limits how often
 * this service may ask.
 */
export async function downloadUrl(ask: typeof fetch = fetch, now = Date.now()): Promise<string> {
  if (newest && newest.until > now) return newest.url;
  try {
    const answer = await ask(RELEASES_API, { headers: { accept: 'application/vnd.github+json', 'user-agent': 'siphon-friends' }, signal: AbortSignal.timeout(4000) });
    if (answer.ok) {
      const releases = await answer.json() as { draft?: boolean; published_at?: string; assets?: { name?: string; browser_download_url?: string }[] }[];
      // By date: GitHub's own order puts pre.9 above pre.10.
      const found = releases
        .filter(release => !release.draft && release.published_at)
        .sort((a, b) => Date.parse(b.published_at!) - Date.parse(a.published_at!))
        .map(release => release.assets?.find(asset => ZIP.test(asset.name ?? ''))?.browser_download_url)
        .find(url => url?.startsWith('https://github.com/Senni360/siphon-player/'));
      if (found) {
        newest = { url: found, until: now + REMEMBER_MS };
        return found;
      }
    }
  } catch { /* The releases page still works. */ }
  return RELEASES;
}

/** Styles sit in the page and nothing else loads, so the policy can say so. */
export const SITE_HEADERS = {
  'Content-Type': 'text/html; charset=utf-8',
  'Cache-Control': 'public, max-age=300',
  'Content-Security-Policy': "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
  'X-Content-Type-Options': 'nosniff',
  'Referrer-Policy': 'no-referrer',
};
