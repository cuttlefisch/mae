// SPIKE: the WASM module against mae-sync's native content_crypto.
// SPDX-License-Identifier: GPL-3.0-or-later
//
//   MAE_CRYPTO_WASM=... MAE_CRYPTO_NATIVE=... node shared/crypto-wasm/js/test.mjs
//
// Inputs are fresh random keys, seeds and plaintexts each run (no fixed
// vectors to pass by construction), at the boundaries: empty, one byte, a
// block edge, and above 64 KiB.

import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import assert from 'node:assert/strict';
import { instantiate, CryptoError } from './mae-crypto.mjs';

const wasmBytes = readFileSync(process.env.MAE_CRYPTO_WASM);
const nativeBin = process.env.MAE_CRYPTO_NATIVE;
const wasm = instantiate(wasmBytes);

const hex = (b) => Buffer.from(b).toString('hex');
const unhex = (s) => new Uint8Array(Buffer.from(s, 'hex'));
const cat = (a, b) => new Uint8Array(Buffer.concat([a, b]));

/** One native process for a batch of [op, request] pairs. */
function native(batch) {
  const input = batch.map(([op, req]) => `${op}\t${hex(req)}`).join('\n') + '\n';
  const lines = execFileSync(nativeBin, [], { input, maxBuffer: 1 << 28 })
    .toString()
    .trimEnd()
    .split('\n');
  assert.equal(lines.length, batch.length);
  return lines.map((l) => (l.startsWith('ERR ') ? Number(l.slice(4)) : unhex(l)));
}

let checks = 0;
const ok = (cond, msg) => {
  assert.ok(cond, msg);
  checks++;
};

const SIZES = [0, 1, 63, 64, 1000, 70000];
const keys = Array.from({ length: 6 }, () => randomBytes(32));
const seeds = Array.from({ length: 6 }, () => randomBytes(32));

// 1. The published wrap key: deterministic, so the two builds must agree exactly.
{
  const n = native(seeds.map((s) => [5, s]));
  seeds.forEach((s, i) => ok(hex(wasm.wrapPublic(s)) === hex(n[i]), 'wrapPublic differs'));
}

// 2. WASM seals, native opens.
{
  const cases = keys.flatMap((k) => SIZES.map((n) => [k, randomBytes(n)]));
  const sealed = cases.map(([k, pt]) => wasm.seal(k, pt));
  const opened = native(cases.map(([k], i) => [2, cat(k, sealed[i])]));
  cases.forEach(([, pt], i) => ok(hex(opened[i]) === hex(pt), `native open of wasm seal #${i}`));
}

// 3. Native seals, WASM opens.
{
  const cases = keys.flatMap((k) => SIZES.map((n) => [k, randomBytes(n)]));
  const sealed = native(cases.map(([k, pt]) => [1, cat(k, pt)]));
  cases.forEach(([k, pt], i) => ok(hex(wasm.open(k, sealed[i])) === hex(pt), `wasm open #${i}`));
}

// 4. Key wrap, both directions, to each recipient's published wrap key.
{
  const pubs = native(seeds.map((s) => [5, s]));
  const cks = seeds.map(() => randomBytes(32));
  const wasmWrapped = cks.map((ck, i) => wasm.wrap(ck, pubs[i]));
  const nUnwrapped = native(wasmWrapped.map((w, i) => [4, cat(seeds[i], w)]));
  cks.forEach((ck, i) => ok(hex(nUnwrapped[i]) === hex(ck), 'native unwrap of wasm wrap'));
  const nWrapped = native(cks.map((ck, i) => [3, cat(ck, pubs[i])]));
  cks.forEach((ck, i) => ok(hex(wasm.unwrap(seeds[i], nWrapped[i])) === hex(ck), 'wasm unwrap'));
  // Wrapped to recipient i, refused by every other recipient, on both sides.
  for (let i = 0; i < seeds.length; i++) {
    const j = (i + 1) % seeds.length;
    assert.throws(() => wasm.unwrap(seeds[j], wasmWrapped[i]), CryptoError);
    ok(native([[4, cat(seeds[j], wasmWrapped[i])]])[0] === -1, 'native refuses wrong recipient');
    checks++;
  }
  // A low-order ephemeral point (all zero) is refused, not used.
  const lowOrder = cat(new Uint8Array(32), wasmWrapped[0].subarray(32));
  assert.throws(() => wasm.unwrap(seeds[0], lowOrder), (e) => e.code === 'malformed');
  checks++;
}

// 5. Tamper: every flipped byte of a sealed blob is refused by both builds.
{
  const k = keys[0];
  const blob = wasm.seal(k, randomBytes(200));
  const batch = [];
  for (let pos = 0; pos < blob.length; pos++) {
    const t = blob.slice();
    t[pos] ^= 1 << pos % 8;
    assert.throws(() => wasm.open(k, t), (e) => e.code === 'decrypt', `wasm tamper @${pos}`);
    batch.push([2, cat(k, t)]);
    checks++;
  }
  native(batch).forEach((r, pos) => ok(r === -1, `native tamper @${pos}`));
  assert.throws(() => wasm.open(keys[1], blob), (e) => e.code === 'decrypt');
  assert.throws(() => wasm.open(k, blob.subarray(0, 23)), (e) => e.code === 'malformed');
  checks += 2;
}

// 6. Entropy reaches the nonce: same key + plaintext twice -> different blobs.
{
  const pt = randomBytes(64);
  const a = wasm.seal(keys[0], pt);
  const b = wasm.seal(keys[0], pt);
  ok(hex(a.subarray(0, 24)) !== hex(b.subarray(0, 24)), 'nonce repeated');
}

// 7. No entropy fails closed: the module traps rather than seal.
{
  const dead = instantiate(wasmBytes, () => {
    throw new Error('no entropy');
  });
  assert.throws(() => dead.seal(keys[0], randomBytes(8)), WebAssembly.RuntimeError);
  checks++;
}

// 8. The module trusts the host's bytes: two instances fed the same
//    "random" bytes repeat nonces. It cannot detect a bad source.
{
  const zeros = () => (v) => v.fill(0);
  const a = instantiate(wasmBytes, zeros()).seal(keys[0], randomBytes(8));
  const b = instantiate(wasmBytes, zeros()).seal(keys[0], randomBytes(8));
  ok(hex(a.subarray(0, 24)) === hex(b.subarray(0, 24)), 'expected a repeated nonce');
}

// 9. How often the module asks the host for entropy (rand's ThreadRng:
//    seeded once, reseeded after 64 KiB of output).
let fills = 0;
{
  const real = (v) => {
    fills++;
    globalThis.crypto.getRandomValues(v);
  };
  const m = instantiate(wasmBytes, real);
  for (let i = 0; i < 5000; i++) m.seal(keys[0], randomBytes(16));
  ok(fills >= 1, 'host was never asked');
}

// 10. No leak across many calls.
{
  const before = wasm.liveBytes();
  for (let i = 0; i < 2000; i++) {
    const s = wasm.seal(keys[i % 6], randomBytes(i % 300));
    wasm.open(keys[i % 6], s);
    try {
      wasm.open(keys[(i + 1) % 6], s);
    } catch {}
  }
  ok(wasm.liveBytes() === before, `leak: ${before} -> ${wasm.liveBytes()}`);
}

// 11. Residue: after each operation returns, no copy of the key, seed,
//     content key or plaintext is left anywhere in linear memory (heap,
//     shadow stack, result slot). JS-side copies are out of scope here.
{
  const copies = (m, needle) => {
    const h = Buffer.from(m.x.memory.buffer);
    let n = 0;
    for (let i = h.indexOf(needle); i !== -1; i = h.indexOf(needle, i + 1)) n++;
    return n;
  };
  const fresh = () => instantiate(wasmBytes);
  let m = fresh();
  let k = randomBytes(32);
  m.seal(k, randomBytes(100));
  ok(copies(m, k) === 0, 'seal left the key');
  m = fresh();
  k = randomBytes(32);
  const pt = randomBytes(100);
  m.open(k, m.seal(k, pt));
  ok(copies(m, k) === 0 && copies(m, pt) === 0, 'open left the key or plaintext');
  m = fresh();
  const seed = randomBytes(32);
  const ck = randomBytes(32);
  m.unwrap(seed, m.wrap(ck, m.wrapPublic(seed)));
  ok(copies(m, seed) === 0, 'unwrap left the seed');
  ok(copies(m, ck) === 0, 'unwrap left the content key');
}

console.log(`ok: ${checks} checks; ${wasmBytes.length} bytes; host fills over 5000 seals: ${fills}`);
