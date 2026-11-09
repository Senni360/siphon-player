import assert from "node:assert/strict";
import test from "node:test";
import { ConfigError, loadConfig } from "../src/config.mjs";
import { mintToken, sameSecret, signToken, TokenError, verifyToken } from "../src/tokens.mjs";

const SECRET = "unit-test-secret-0123456789abcdef";
const OTHER = "unit-test-secret-fedcba9876543210";

test("signs and verifies a short-lived token", () => {
  const token = mintToken({ sub: "listener@example", role: "phone", scope: "live", ttlSeconds: 120 }, SECRET);
  assert.equal(token.split(".").length, 3);
  assert.equal(token.startsWith("e1."), true);
  const claims = verifyToken(token, SECRET, { scope: "live" });
  assert.equal(claims.sub, "listener@example");
  assert.equal(claims.role, "phone");
  assert.equal(claims.scope, "live");
  assert.equal(typeof claims.jti, "string");
  assert.equal(claims.exp > Math.floor(Date.now() / 1000), true);
});

test("refuses an expired token", () => {
  const token = mintToken({ sub: "u1", role: "phone", scope: "media", exp: Math.floor(Date.now() / 1000) - 1 }, SECRET);
  assert.throws(() => verifyToken(token, SECRET), (error) => error instanceof TokenError && error.reason === "expired");
});

test("refuses a wrong scope and a wrong role", () => {
  const token = mintToken({ sub: "u1", role: "phone", scope: "sync" }, SECRET);
  assert.throws(() => verifyToken(token, SECRET, { scope: "live" }), (error) => error.reason === "wrong_scope");
  assert.throws(() => verifyToken(token, SECRET, { role: "computer" }), (error) => error.reason === "wrong_role");
  assert.equal(verifyToken(token, SECRET, { scope: "sync", role: "phone" }).scope, "sync");
});

test("refuses a tampered payload and a tampered signature", () => {
  const token = mintToken({ sub: "u1", role: "phone", scope: "live" }, SECRET);
  const [prefix, payload, signature] = token.split(".");
  const claims = JSON.parse(Buffer.from(payload, "base64url").toString("utf8"));
  claims.sub = "u2";
  const forged = `${prefix}.${Buffer.from(JSON.stringify(claims)).toString("base64url")}.${signature}`;
  assert.throws(() => verifyToken(forged, SECRET), (error) => error.reason === "bad_signature");
  const flipped = `${prefix}.${payload}.${signature.slice(0, -2)}AA`;
  assert.throws(() => verifyToken(flipped, SECRET), (error) => error.reason === "bad_signature");
  assert.throws(() => verifyToken(token, OTHER), (error) => error.reason === "bad_signature");
});

test("refuses malformed claims and malformed input", () => {
  for (const bad of ["", "x", "e1.a.b", "e2.abc.def", "e1.."]) {
    assert.throws(() => verifyToken(bad, SECRET), TokenError);
  }
  const badSub = signToken({ sub: "../etc/passwd", role: "phone", scope: "live", exp: future(), jti: "abcdefgh" }, SECRET);
  assert.throws(() => verifyToken(badSub, SECRET), (error) => error.reason === "bad_subject");
  const badRole = signToken({ sub: "u1", role: "admin", scope: "live", exp: future(), jti: "abcdefgh" }, SECRET);
  assert.throws(() => verifyToken(badRole, SECRET), (error) => error.reason === "bad_role");
  const badScope = signToken({ sub: "u1", role: "phone", scope: "root", exp: future(), jti: "abcdefgh" }, SECRET);
  assert.throws(() => verifyToken(badScope, SECRET), (error) => error.reason === "bad_scope");
});

test("carries and checks the media key claim", () => {
  const token = mintToken({ sub: "u1", role: "computer", scope: "media", key: "c0123456789abcdef0123456789abcdef0123456789abcdef01234567" }, SECRET);
  assert.equal(verifyToken(token, SECRET, { scope: "media" }).key.length, 57);
  assert.throws(() => mintToken({ sub: "u1", role: "phone", scope: "media", key: "../x" }, SECRET) && verifyToken(
    signToken({ sub: "u1", role: "phone", scope: "media", key: "../x", exp: future(), jti: "abcdefgh" }, SECRET),
    SECRET,
  ), (error) => error.reason === "bad_key");
});

test("the same signing function can be used by the Vercel service", () => {
  const minted = signToken({ sub: "vercel-user", role: "computer", scope: "sync", exp: future(), jti: "vercel01" }, SECRET);
  const claims = verifyToken(minted, SECRET, { scope: "sync" });
  assert.equal(claims.sub, "vercel-user");
});

test("refuses to start without a token secret", () => {
  assert.throws(() => loadConfig({}), (error) => error instanceof ConfigError);
  assert.throws(() => loadConfig({ EDGE_TOKEN_SECRET: "short" }), ConfigError);
  assert.throws(() => loadConfig({ EDGE_TOKEN_SECRET: SECRET, EDGE_ALLOWED_ORIGINS: "not a url" }), ConfigError);
  assert.doesNotThrow(() => loadConfig({ EDGE_TOKEN_SECRET: SECRET }));
});

test("admin token comparison is length-safe", () => {
  assert.equal(sameSecret("abc", "abc"), true);
  assert.equal(sameSecret("abc", "abcd"), false);
  assert.equal(sameSecret("", ""), false);
  assert.equal(sameSecret(undefined, "abc"), false);
});

function future() {
  return Math.floor(Date.now() / 1000) + 60;
}
