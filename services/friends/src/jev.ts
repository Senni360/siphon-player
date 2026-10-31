// Jev, TypeSafe AI's decision model, answers typed questions with
// probabilities instead of writing text. With TYPESAFE_API_KEY the service
// calls TypeSafe directly; otherwise it uses Vercel AI Gateway's
// TypeSafe-compatible route with AI_GATEWAY_API_KEY or the deployment's OIDC
// token. Keys live in the deployment only, never in the player. Callers treat a missing
// answer as "no opinion": Jev only improves an order or a guess, it never
// gates a feature or shows the user a verdict.

export const TYPESAFE_ENDPOINT = 'https://api.typesafe.ai/v1/systemone';
export const GATEWAY_ENDPOINT = 'https://ai-gateway.vercel.sh/typesafe/v1/systemone';
// Pinned: TypeSafe's aliases can move to a new version at any time.
const TYPESAFE_MODEL = 'jev-1.14.0';
const GATEWAY_MODEL = 'typesafe-ai/jev';
const MAX_RESPONSE_BYTES = 256 * 1024;
const TIMEOUT_MS = 2500;

export type Question =
  | { type: 'noul'; instructions: string }
  | { type: 'choice'; instructions: string; criteria: Record<string, string> };
export type Answer =
  | { type: 'noul'; probability: number }
  | { type: 'choice'; choice: string; probabilities: Record<string, number> };
/** Answers by question id, or null when Jev is unconfigured, slow or failing. */
export type Ask = (state: string, questions: Record<string, Question>, headers: Headers, signal: AbortSignal) => Promise<Record<string, Answer> | null>;

const unit = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= 1;

function answer(question: Question, value: unknown): Answer | null {
  if (!value || typeof value !== 'object') return null;
  const raw = value as Record<string, unknown>;
  if (question.type === 'noul') return unit(raw.noul) ? { type: 'noul', probability: raw.noul } : null;
  if (typeof raw.choice !== 'string' || !(raw.choice in question.criteria)) return null;
  const probabilities: Record<string, number> = {};
  if (raw.probabilities && typeof raw.probabilities === 'object') {
    for (const key of Object.keys(question.criteria)) {
      const p = (raw.probabilities as Record<string, unknown>)[key];
      if (unit(p)) probabilities[key] = p;
    }
  }
  return { type: 'choice', choice: raw.choice, probabilities };
}

export function createJev(env: NodeJS.ProcessEnv, fetcher: typeof fetch = fetch): Ask {
  return async (state, questions, headers, caller) => {
    // A TypeSafe key wins; otherwise a gateway key or the per-request OIDC
    // token Vercel supplies to the function. None is ever logged or echoed.
    const direct = env.TYPESAFE_API_KEY;
    const credential = direct || env.AI_GATEWAY_API_KEY || headers.get('x-vercel-oidc-token') || env.VERCEL_OIDC_TOKEN;
    if (!credential || !Object.keys(questions).length) return null;
    const signal = AbortSignal.any([caller, AbortSignal.timeout(TIMEOUT_MS)]);
    try {
      const response = await fetcher(direct ? TYPESAFE_ENDPOINT : GATEWAY_ENDPOINT, {
        method: 'POST', redirect: 'error', signal,
        headers: { Authorization: `Bearer ${credential}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ model: direct ? TYPESAFE_MODEL : GATEWAY_MODEL, state, questions }),
      });
      if (!response.ok || Number(response.headers.get('content-length')) > MAX_RESPONSE_BYTES) {
        await response.body?.cancel();
        return null;
      }
      const text = await response.text();
      if (Buffer.byteLength(text) > MAX_RESPONSE_BYTES) return null;
      const answers = (JSON.parse(text) as { answers?: Record<string, unknown> } | null)?.answers;
      if (!answers || typeof answers !== 'object') return null;
      const result: Record<string, Answer> = {};
      for (const [id, question] of Object.entries(questions)) {
        const parsed = answer(question, answers[id]);
        if (parsed) result[id] = parsed;
      }
      return result;
    } catch { return null; }
  };
}

/** Jev disabled: every question goes unanswered. */
export const noJev: Ask = async () => null;

export interface Track { title: string; artists: string[] }
export const describe = (track: Track) => `"${track.title}" by ${track.artists.join(', ') || 'an unknown artist'}`;

const NOISE = /\b(official|music video|video|audio|lyrics?|visuali[sz]er|free ?(dl|download)|prod\.?|produced by|hq|hd|4k|out now|premiere|m\/v|mv|full song)\b/i;
const SEPARATORS = [' - ', ' – ', ' — ', ' | ', ' // ', ' ~ '];

/**
 * Readings of an upload title as artist and song, most literal first. Jev only
 * picks between these; it never writes a name. Brackets that carry the song's
 * identity, such as a remix, a feature or a live version, stay in the title.
 */
export function titleReadings(title: string, uploader: string): { artist: string; title: string }[] {
  const tidy = (text: string) => text.replace(/\s+/g, ' ').replace(/^[\s\-–—|:~]+|[\s\-–—|:~]+$/g, '').trim();
  const clean = tidy(title.replace(/[([{]([^)\]}]*)[)\]}]/g, (whole, inner: string) => NOISE.test(inner) ? ' ' : whole).replace(/\bfree (dl|download)\b/gi, ' '));
  const readings: { artist: string; title: string }[] = [];
  const add = (artist: string, song: string) => {
    artist = tidy(artist); song = tidy(song);
    if (artist && song && artist.length <= 200 && song.length <= 300 &&
      !readings.some(r => r.artist.toLowerCase() === artist.toLowerCase() && r.title.toLowerCase() === song.toLowerCase())) readings.push({ artist, title: song });
  };
  for (const separator of SEPARATORS) {
    const at = clean.indexOf(separator);
    if (at < 0) continue;
    const left = clean.slice(0, at), right = clean.slice(at + separator.length);
    add(left, right);
    add(right, left);
  }
  if (uploader.trim()) add(uploader.replace(/\s*-\s*topic$/i, ''), clean);
  return readings.slice(0, 12);
}
