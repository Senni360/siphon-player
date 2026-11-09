// HMAC-SHA256 bearer tokens. The same format is minted by the Vercel group
// service, so this module stays dependency-free and importable on its own.
//
// Wire form: e1.<base64url(payload)>.<base64url(hmac)>
// Payload:   { v, sub, role, scope, exp, jti[, key] }

import { createHmac, randomUUID, timingSafeEqual } from "node:crypto";

export const TOKEN_PREFIX = "e1";
export const ROLES = Object.freeze(["phone", "computer"]);
export const SCOPES = Object.freeze(["live", "media", "sync"]);
export const MAX_TOKEN_LENGTH = 4096;
export const MIN_SECRET_LENGTH = 32;

const SUBJECT = /^[A-Za-z0-9._@:-]{1,128}$/;
const JTI = /^[A-Za-z0-9_-]{8,64}$/;
const KEY = /^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/;
const SEGMENT = /^[A-Za-z0-9_-]+$/;

export class TokenError extends Error {
  constructor(reason) {
    super(reason);
    this.name = "TokenError";
    this.reason = reason;
  }
}

export function base64url(bytes) {
  return Buffer.from(bytes).toString("base64url");
}

function payloadBytes(payload) {
  return Buffer.from(JSON.stringify(payload), "utf8");
}

function signature(payloadSegment, secret) {
  return createHmac("sha256", secret).update(payloadSegment).digest();
}

/** Signs a token. Shared with the Vercel service through this exact function. */
export function signToken(claims, secret) {
  if (typeof secret !== "string" || secret.length < MIN_SECRET_LENGTH) {
    throw new TokenError("bad_secret");
  }
  const payload = {
    v: 1,
    sub: claims.sub,
    role: claims.role,
    scope: claims.scope,
    exp: claims.exp,
    jti: claims.jti ?? randomUUID().replaceAll("-", ""),
  };
  if (claims.key !== undefined) payload.key = claims.key;
  const segment = base64url(payloadBytes(payload));
  return `${TOKEN_PREFIX}.${segment}.${base64url(signature(segment, secret))}`;
}

/**
 * Mints a token. `exp` is absolute seconds; `ttlSeconds` is a convenience.
 * No token is ever logged by this module.
 */
export function mintToken({ sub, role, scope, key, exp, ttlSeconds = 300, jti }, secret, now = Date.now()) {
  const expires = exp ?? Math.floor(now / 1000) + ttlSeconds;
  return signToken({ sub, role, scope, exp: expires, jti, key }, secret);
}

function decodeJson(segment) {
  let text;
  try {
    text = Buffer.from(segment, "base64url").toString("utf8");
  } catch {
    throw new TokenError("malformed");
  }
  if (text.length === 0 || text.length > 2048) throw new TokenError("malformed");
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    throw new TokenError("malformed");
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new TokenError("malformed");
  return value;
}

/**
 * Verifies a token. Rejects malformed input, a bad signature, a wrong role or
 * scope, an expired token and claims outside the small allowed grammar.
 * `scope`/`role` are required expectations unless passed as null.
 */
export function verifyToken(token, secret, { role = null, scope = null, now = Date.now() } = {}) {
  if (typeof secret !== "string" || secret.length < MIN_SECRET_LENGTH) {
    throw new TokenError("bad_secret");
  }
  if (typeof token !== "string" || token.length === 0 || token.length > MAX_TOKEN_LENGTH) {
    throw new TokenError("malformed");
  }
  const parts = token.split(".");
  if (parts.length !== 3 || parts[0] !== TOKEN_PREFIX) throw new TokenError("malformed");
  const [, segment, providedSegment] = parts;
  if (!SEGMENT.test(segment) || !SEGMENT.test(providedSegment)) throw new TokenError("malformed");

  const provided = Buffer.from(providedSegment, "base64url");
  const expected = signature(segment, secret);
  // timingSafeEqual needs equal lengths. A wrong-length signature is rejected
  // before comparing, and every authentic signature is exactly 32 bytes.
  if (provided.length !== expected.length || !timingSafeEqual(provided, expected)) {
    throw new TokenError("bad_signature");
  }

  const payload = decodeJson(segment);
  if (payload.v !== 1) throw new TokenError("bad_version");
  if (typeof payload.sub !== "string" || !SUBJECT.test(payload.sub)) throw new TokenError("bad_subject");
  if (!ROLES.includes(payload.role)) throw new TokenError("bad_role");
  if (!SCOPES.includes(payload.scope)) throw new TokenError("bad_scope");
  if (role !== null && payload.role !== role) throw new TokenError("wrong_role");
  if (scope !== null && payload.scope !== scope) throw new TokenError("wrong_scope");
  if (!Number.isSafeInteger(payload.exp) || payload.exp <= 0) throw new TokenError("bad_expiry");
  if (payload.exp <= Math.floor(now / 1000)) throw new TokenError("expired");
  if (typeof payload.jti !== "string" || !JTI.test(payload.jti)) throw new TokenError("bad_jti");
  if (payload.key !== undefined && (typeof payload.key !== "string" || !KEY.test(payload.key))) {
    throw new TokenError("bad_key");
  }
  return Object.freeze({ ...payload });
}

/** Constant-time comparison for the separate admin token. */
export function sameSecret(provided, expected) {
  if (typeof provided !== "string" || typeof expected !== "string" || expected.length === 0) return false;
  const left = Buffer.from(provided, "utf8");
  const right = Buffer.from(expected, "utf8");
  if (left.length !== right.length) return false;
  return timingSafeEqual(left, right);
}
