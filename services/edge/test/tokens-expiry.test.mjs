// Token lifetime edges and every tampering shape the verifier claims to refuse.
import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import test from "node:test";
import { MAX_TOKEN_LENGTH, mintToken, signToken, TokenError, verifyToken } from "../src/tokens.mjs";

/** Signs any payload object, including shapes `signToken` would never mint. */
function rawSign(payload) {
  const segment = Buffer.from(JSON.stringify(payload), "utf8").toString("base64url");
  const signature = createHmac("sha256", SECRET).update(segment).digest("base64url");
  return `e1.${segment}.${signature}`;
}

const SECRET = "unit-test-secret-0123456789abcdef";
const OTHER = "unit-test-secret-fedcba9876543210";
const reason = (fn) => {
  try {
    fn();
  } catch (error) {
    return error instanceof TokenError ? error.reason : error?.name;
  }
  return "accepted";
};

test("a token is alive until the last millisecond of its expiry second", () => {
  const exp = 1_700_000_000;
  const token = mintToken({ sub: "u1", role: "phone", scope: "live", exp }, SECRET);
  assert.equal(verifyToken(token, SECRET, { now: exp * 1000 - 1 }).sub, "u1");
  assert.equal(reason(() => verifyToken(token, SECRET, { now: exp * 1000 })), "expired");
  assert.equal(reason(() => verifyToken(token, SECRET, { now: exp * 1000 + 1 })), "expired");
});

test("minting floors the clock, so a ttl from mid-second does not lose or gain a second", () => {
  const now = 1_700_000_000_999;
  const token = mintToken({ sub: "u1", role: "phone", scope: "sync", ttlSeconds: 1 }, SECRET, now);
  const claims = verifyToken(token, SECRET, { now: 1_700_000_001_000 - 1 });
  assert.equal(claims.exp, 1_700_000_001);
  assert.equal(reason(() => verifyToken(token, SECRET, { now: 1_700_000_001_000 })), "expired");
});

test("every part of the token is covered by the signature", () => {
  const token = mintToken({ sub: "u1", role: "phone", scope: "live", exp: Math.floor(Date.now() / 1000) + 60 }, SECRET);
  const [prefix, payload, signature] = token.split(".");
  const claims = JSON.parse(Buffer.from(payload, "base64url").toString("utf8"));
  const forge = (change) => {
    const copy = { ...claims };
    change(copy);
    return `${prefix}.${Buffer.from(JSON.stringify(copy)).toString("base64url")}.${signature}`;
  };
  assert.equal(reason(() => verifyToken(forge((copy) => { copy.sub = "u2"; }), SECRET)), "bad_signature");
  assert.equal(reason(() => verifyToken(forge((copy) => { copy.exp += 60; }), SECRET)), "bad_signature");
  assert.equal(reason(() => verifyToken(forge((copy) => { copy.role = "computer"; }), SECRET)), "bad_signature");
  assert.equal(reason(() => verifyToken(`${prefix}.${payload}.${signature.slice(0, -1)}`, SECRET)), "bad_signature");
  assert.equal(reason(() => verifyToken(`${prefix}.${payload}.${signature}.x`, SECRET)), "malformed");
  assert.equal(reason(() => verifyToken(token, OTHER)), "bad_signature");
  // A signature lifted from another token is not the signature for this payload.
  const other = mintToken({ sub: "u1", role: "phone", scope: "live", exp: Math.floor(Date.now() / 1000) + 60 }, SECRET);
  assert.equal(reason(() => verifyToken(`${prefix}.${payload}.${other.split(".")[2]}`, SECRET)), "bad_signature");
});

test("a signed payload still has to obey the claim grammar", () => {
  const exp = Math.floor(Date.now() / 1000) + 60;
  const signed = (overrides) => signToken({ sub: "u1", role: "phone", scope: "live", exp, jti: "abcdefgh", ...overrides }, SECRET);
  assert.equal(reason(() => verifyToken(rawSign({ v: 2, sub: "u1", role: "phone", scope: "live", exp, jti: "abcdefgh" }), SECRET)), "bad_version");
  for (const [name, claims] of [
    ["subject", { sub: "" }],
    ["subject with a slash", { sub: "a/b" }],
    ["subject too long", { sub: "a".repeat(129) }],
    ["role", { role: "admin" }],
    ["scope", { scope: "root" }],
    ["zero expiry", { exp: 0 }],
    ["fractional expiry", { exp: 1.5 }],
    ["string expiry", { exp: String(exp) }],
    ["short jti", { jti: "abc" }],
    ["jti punctuation", { jti: "abc def" }],
    ["bad key", { key: "../etc/passwd" }],
  ]) {
    assert.notEqual(reason(() => verifyToken(signed(claims), SECRET)), "accepted", name);
  }
  assert.equal(verifyToken(signed({ key: "a-song" }), SECRET).key, "a-song");
});

test("malformed wire forms and oversized tokens are refused before any crypto", () => {
  const exp = Math.floor(Date.now() / 1000) + 60;
  const valid = signToken({ sub: "u1", role: "phone", scope: "live", exp, jti: "abcdefgh" }, SECRET);
  const [, payload, signature] = valid.split(".");
  for (const token of [
    "", "e1", "e1.a", "e1..", "e2.abc.def", `e1.${payload}.${signature}=`,
    `e1.${payload}==.${signature}`, `e1.${payload}.${signature}.extra`,
    `e1.${Buffer.from("[]").toString("base64url")}.${signature}`,
    `e1.${Buffer.from("null").toString("base64url")}.${signature}`,
    `e1.${Buffer.from("{").toString("base64url")}.${signature}`,
    "e1." + "A".repeat(MAX_TOKEN_LENGTH) + "." + signature,
    "x".repeat(MAX_TOKEN_LENGTH + 1),
  ]) {
    assert.notEqual(reason(() => verifyToken(token, SECRET)), "accepted", JSON.stringify(token.slice(0, 24)));
  }
});

test("a secret shorter than the minimum is refused on both sides", () => {
  assert.equal(reason(() => signToken({ sub: "u1", role: "phone", scope: "live", exp: 1 }, "short")), "bad_secret");
  const valid = mintToken({ sub: "u1", role: "phone", scope: "live" }, SECRET);
  assert.equal(reason(() => verifyToken(valid, "short")), "bad_secret");
  assert.equal(reason(() => verifyToken(valid, undefined)), "bad_secret");
});
