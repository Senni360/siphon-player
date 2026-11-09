import { createHmac, randomUUID } from 'node:crypto';

// Edge bearer tokens, in the exact wire format that `services/edge/src/tokens.mjs`
// mints and verifies:
//
//   e1.<base64url(payload)>.<base64url(HMAC-SHA256(payload segment))>
//
// The group service and the always-on edge share one HMAC secret, so a token
// made here is accepted there. `test/edge.test.ts` checks that with the edge's
// own verifier. Nothing here is ever logged, and no secret is stored.

export const EDGE_URL = 'https://edge.encore.senni.dev';
/** Short enough that a copied URL goes stale quickly. */
export const EDGE_TOKEN_TTL_SECONDS = 300;
const TOKEN_PREFIX = 'e1';
export const EDGE_ROLES = Object.freeze(['phone', 'computer'] as const);
export type EdgeRole = (typeof EDGE_ROLES)[number];
export type EdgeScope = 'live' | 'media' | 'sync';
const MIN_SECRET_LENGTH = 32;

/** A usable secret is the same minimum the edge itself enforces. */
export const edgeConfigured = (secret: string | undefined): secret is string =>
  typeof secret === 'string' && secret.length >= MIN_SECRET_LENGTH;

export interface EdgeClaims {
  sub: string;
  role: EdgeRole;
  scope: EdgeScope;
  exp: number;
  jti?: string;
}

const base64url = (bytes: Buffer) => bytes.toString('base64url');

/** Signs one token. The payload keys and order match the edge's `signToken`. */
export function signEdgeToken({ sub, role, scope, exp, jti }: EdgeClaims, secret: string): string {
  if (!edgeConfigured(secret)) throw new Error('edge secret is not configured');
  const payload = { v: 1, sub, role, scope, exp, jti: jti ?? randomUUID().replaceAll('-', '') };
  const segment = base64url(Buffer.from(JSON.stringify(payload), 'utf8'));
  const signature = base64url(createHmac('sha256', secret).update(segment).digest());
  return `${TOKEN_PREFIX}.${segment}.${signature}`;
}

/** A token that expires `ttlSeconds` from `now`, or at `exp` when given. */
export function mintEdgeToken(
  { sub, role, scope, exp, ttlSeconds = EDGE_TOKEN_TTL_SECONDS, jti }: Omit<EdgeClaims, 'exp'> & { exp?: number; ttlSeconds?: number },
  secret: string,
  now = Date.now(),
): string {
  return signEdgeToken({ sub, role, scope, exp: exp ?? Math.floor(now / 1000) + ttlSeconds, jti }, secret);
}
