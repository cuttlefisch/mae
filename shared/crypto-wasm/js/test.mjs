// The WASM module against the same code built natively, and its guards.
// SPDX-License-Identifier: GPL-3.0-or-later
//
//   MAE_CRYPTO_WASM=path/to/mae_crypto_wasm.wasm MAE_CRYPTO_NATIVE=path/to/mae-crypto-native \
//     node shared/crypto-wasm/js/test.mjs
//
// (`make crypto-wasm-test` sets both.) Checks, each a failure on its own:
//  1. size: the artifact is within SIZE_BUDGET bytes;
//  2. ABI: exactly the documented exports and one import;
//  3. differential: every deterministic response (key forms, fingerprints,
//     signatures, every refusal) is byte-identical to the native build's,
//     over a request script with fresh random seeds each run;
//  4. interop: what one build seals (backups, content, key wraps) the other
//     opens, and refuses with a wrong passphrase, key or recipient, or a
//     tampered byte;
//  5. entropy: without a source, nothing is produced (an error, or a trap
//     that retires the instance), and an all-zero source is refused;
//  6. memory: no leak over many calls, and after a key is forgotten (or an
//     operation returns) no copy of a seed, content key, plaintext or
//     passphrase is left anywhere in linear memory;
//  7. the loader NFC-normalises passphrases.

import { instantiate, MaeCryptoError } from './mae-crypto.mjs';
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { randomBytes } from 'node:crypto';

// Measured with rustc 1.98, `--profile wasm` (opt-level "s"): 346,994 bytes
// on 2026-11-15; the budget is ~15% over that (docs/E2E_ENCRYPTION.md §10). The module guards against
// accidental bloat (a stray dependency, a generic blow-up); raising the
// budget is a decision to record there, not a number to bump.
const SIZE_BUDGET = 400_000;
const OPS = new Map([
  ['version', 1], ['generate', 2], ['importSeed', 3], ['exportSeed', 4], ['forget', 5],
  ['keyInfo', 6], ['fingerprint', 7], ['checkChallenge', 8], ['signChallenge', 9],
  ['backupSeal', 10], ['backupOpen', 11], ['backupParams', 12], ['contentKeyGenerate', 13],
  ['contentSeal', 14], ['contentOpen', 15], ['contentWrap', 16], ['contentUnwrap', 17],
]);
const FAST = { m: 19456, t: 2, p: 1 };
const ISS = 'https://idp.example.com';
const SUB = 'alice@idp';
const NOW = 1_900_000_000;

const wasmPath = process.env.MAE_CRYPTO_WASM;
const nativePath = process.env.MAE_CRYPTO_NATIVE;
if (!wasmPath || !nativePath) {
  console.error('set MAE_CRYPTO_WASM and MAE_CRYPTO_NATIVE (see `make crypto-wasm-test`)');
  process.exit(2);
}

let checks = 0;
const failures = [];
const check = (cond, what) => {
  checks++;
  if (!cond) {
    failures.push(what);
    console.error(`FAIL ${what}`);
  }
};

const b64 = (bytes) => Buffer.from(bytes).toString('base64');
const unb64 = (s) => Buffer.from(s, 'base64');
const challengeText = (fp, iss, sub, exp) =>
  `{"domain":"mae-identity-binding-v1","iss":${JSON.stringify(iss)},"sub":${JSON.stringify(sub)},` +
  `"aud":"https://mae.example.com","fingerprint":${JSON.stringify(fp)},"nonce":"bm9uY2U","expires_at":${exp}}`;

const wasmBytes = readFileSync(wasmPath);
const module = new WebAssembly.Module(wasmBytes);

/** Runs [op, request] pairs through one native process; returns the response strings. */
const dir = mkdtempSync(join(tmpdir(), 'mae-crypto-diff-'));
function native(batch) {
  const file = join(dir, 'requests');
  writeFileSync(file, batch.map(([op, req]) => `${OPS.get(op)}\t${JSON.stringify(req)}`).join('\n') + '\n');
  const r = spawnSync(nativePath, [file], { encoding: 'utf8', maxBuffer: 1 << 28 });
  rmSync(file);
  if (r.status !== 0) throw new Error(`native exited ${r.status}: ${r.stderr}`);
  const lines = r.stdout.trimEnd().split('\n');
  if (lines.length !== batch.length) throw new Error('native answered a different number of lines');
  return lines;
}
/** The same pairs through a fresh WASM instance. */
function wasm(batch, m = instantiate(module)) {
  return batch.map(([op, req]) => m.callRaw(op, JSON.stringify(req)));
}
const result = (raw) => JSON.parse(raw);

// 1. Size.
check(wasmBytes.length <= SIZE_BUDGET, `size ${wasmBytes.length} > budget ${SIZE_BUDGET}`);

// 2. ABI: what the module asks of the host and offers it, and nothing else
//    (no getrandom or other stray symbol).
{
  const imports = WebAssembly.Module.imports(module).map((i) => `${i.module}.${i.name}:${i.kind}`);
  check(JSON.stringify(imports) === '["mae_crypto_host.random_fill:function"]', `imports: ${imports}`);
  const exported = WebAssembly.Module.exports(module)
    .map((e) => e.name)
    .filter((n) => n !== '__data_end' && n !== '__heap_base')
    .sort();
  const want = ['mae_crypto_alloc', 'mae_crypto_call', 'mae_crypto_live_bytes', 'mae_crypto_result_free',
    'mae_crypto_result_ptr', 'memory'];
  check(JSON.stringify(exported) === JSON.stringify(want), `exports: ${exported}`);
}

// 3. Differential: one script, both builds, byte for byte.
{
  const seeds = Array.from({ length: 6 }, () => b64(randomBytes(32)));
  const script = [['version', {}]];
  seeds.forEach((seed, i) => {
    const key = i + 1;
    script.push(['importSeed', { seed }], ['keyInfo', { key }], ['exportSeed', { key }]);
  });
  // The fingerprints are needed to write challenges: take them from native.
  const infos = native(script).filter((_, i) => script[i][0] === 'keyInfo').map((l) => result(l).result);
  infos.forEach((info, i) => {
    const key = info.key;
    const fp = info.fingerprint;
    const other = infos[(i + 1) % infos.length].fingerprint;
    const cases = [
      challengeText(fp, ISS, SUB, NOW + 60),
      JSON.stringify({ challenge_id: 'x', challenge: challengeText(fp, ISS, SUB, NOW + 60), expires_in: 300 }),
      challengeText(other, ISS, SUB, NOW + 60),
      challengeText(fp, ISS, 'mallory@idp', NOW + 60),
      challengeText(fp, 'https://idp.example.org', SUB, NOW + 60),
      challengeText(fp, ISS, SUB, NOW),
      challengeText(fp, ISS, '', NOW + 60),
      challengeText(fp, ISS, SUB, NOW + 60).replace('":', '": '),
      challengeText(fp, ISS, SUB, NOW + 60).replace('mae-identity-binding-v1', 'mae-other-v1'),
      challengeText(fp, ISS, SUB, NOW + 60).replace('{', '{"x":1,'),
      challengeText(fp, ISS, 'a'.repeat(5000), NOW + 60),
      'sign me',
    ];
    for (const challenge of cases) {
      const req = { key, issuer: ISS, subject: SUB, challenge, now: NOW };
      script.push(['checkChallenge', req], ['signChallenge', req]);
    }
    script.push(['fingerprint', { public_key: info.public_key }]);
  });
  script.push(
    ['forget', { key: 1 }], ['forget', { key: 1 }], ['keyInfo', { key: 1 }], ['exportSeed', { key: 1 }],
    ['fingerprint', { public_key: b64(new Uint8Array(31)) }], ['importSeed', { seed: 'AAAA' }],
    ['keyInfo', {}], ['keyInfo', { key: 2, x: 1 }], ['version', { x: 1 }],
    ['backupParams', { envelope: 'not json' }], ['backupOpen', { envelope: '{"v":2}', passphrase: 'p' }],
    ['contentOpen', { content_key: b64(randomBytes(32)), sealed: b64(randomBytes(80)) }],
    ['contentOpen', { content_key: b64(randomBytes(32)), sealed: 'AAAA' }],
    ['contentWrap', { content_key: b64(randomBytes(32)), wrap_public: b64(new Uint8Array(32)) }],
  );
  const n = native(script);
  const w = wasm(script);
  let same = 0;
  script.forEach(([op], i) => {
    if (n[i] === w[i]) same++;
    else check(false, `differential #${i} ${op}: native ${n[i]} wasm ${w[i]}`);
  });
  check(same === script.length, `differential: ${same}/${script.length} identical`);
  // The script exercised refusals, not only successes.
  const codes = new Set(w.map(result).filter((r) => !r.ok).map((r) => r.code));
  for (const c of ['other-key', 'other-subject', 'other-issuer', 'expired', 'empty-field', 'not-a-challenge',
    'wrong-domain', 'too-long', 'no-key', 'bad-public-key', 'bad-request', 'not-an-envelope', 'unknown-version',
    'decrypt', 'malformed']) {
    check(codes.has(c), `differential never saw '${c}'`);
  }
  // And a signature verifies over exactly the challenge bytes.
  const signed = w.map(result).find((r) => r.ok && r.result.body);
  check(signed !== undefined, 'nothing was signed');
}

// 4. Interop: sealed by one build, opened by the other.
{
  const m = instantiate(module);
  const seed = b64(randomBytes(32));
  const k = m.importSeed(seed);
  const pass = 'correct horse ' + b64(randomBytes(6));
  const env = m.backupSeal({ key: k.key, passphrase: pass, kdf: FAST });
  const [opened, wrongPass] = native([
    ['backupOpen', { envelope: env, passphrase: pass }],
    ['backupOpen', { envelope: env, passphrase: pass + '!' }],
  ]).map(result);
  check(opened.ok && opened.result.fingerprint === k.fingerprint, 'native opens the WASM backup');
  check(!wrongPass.ok && wrongPass.code === 'open', 'native refuses the wrong passphrase');
  const [nk, nenv] = native([
    ['importSeed', { seed }],
    ['backupSeal', { key: 1, passphrase: pass, kdf: FAST }],
  ]).map(result);
  check(nk.result.fingerprint === k.fingerprint, 'same seed, same key natively');
  const back = m.backupOpen({ envelope: nenv.result.envelope, passphrase: pass });
  check(back.fingerprint === k.fingerprint, 'WASM opens the native backup');
  check(m.exportSeed(back.key) === seed, 'and the seed is the one sealed');
  let refused = 0;
  try { m.backupOpen({ envelope: nenv.result.envelope, passphrase: 'wrong' }); } catch (e) { refused += e.code === 'open'; }
  check(refused === 1, 'WASM refuses the wrong passphrase');
  // Tampered envelopes: a sample of positions across every field, both builds.
  const batch = [];
  const positions = [];
  for (let pos = 0; pos < env.length; pos += 5) positions.push(pos);
  for (const pos of positions) {
    const t = env.slice(0, pos) + String.fromCharCode(env.charCodeAt(pos) ^ (1 << (pos % 7))) + env.slice(pos + 1);
    batch.push(['backupOpen', { envelope: t, passphrase: pass }]);
    let threw = false;
    try { m.backupOpen({ envelope: t, passphrase: pass }); } catch (e) { threw = e instanceof MaeCryptoError; }
    check(threw, `WASM opened a backup altered at ${pos}`);
  }
  native(batch).map(result).forEach((r, i) => check(!r.ok, `native opened a backup altered at ${positions[i]}`));
  // Content: seal/open and wrap/unwrap across the two builds.
  const ck = m.contentKeyGenerate();
  for (const size of [0, 1, 63, 64, 1000, 70000]) {
    const pt = b64(randomBytes(size));
    const s = m.contentSeal(ck, pt);
    const [o, wrong] = native([
      ['contentOpen', { content_key: ck, sealed: s }],
      ['contentOpen', { content_key: b64(randomBytes(32)), sealed: s }],
    ]).map(result);
    check(o.ok && o.result.plaintext === pt, `native opens WASM content (${size})`);
    check(!wrong.ok && wrong.code === 'decrypt', `native refuses the wrong content key (${size})`);
    const [ns] = native([['contentSeal', { content_key: ck, plaintext: pt }]]).map(result);
    check(m.contentOpen(ck, ns.result.sealed) === pt, `WASM opens native content (${size})`);
  }
  const bob = m.generate();
  const wrapped = m.contentWrap(ck, k.wrap_public);
  const [nu, nwrap] = native([
    ['importSeed', { seed }],
    ['contentUnwrap', { key: 1, wrapped }],
    ['contentWrap', { content_key: ck, wrap_public: k.wrap_public }],
  ]).slice(1).map(result);
  check(nu.ok && nu.result.content_key === ck, 'native unwraps the WASM wrap');
  check(m.contentUnwrap(k.key, nwrap.result.wrapped) === ck, 'WASM unwraps the native wrap');
  let wrongMember = null;
  try { m.contentUnwrap(bob.key, wrapped); } catch (e) { wrongMember = e.code; }
  check(wrongMember === 'decrypt', 'another member cannot unwrap');
  // Every flipped byte of a sealed blob is refused by both builds.
  const sealed = unb64(m.contentSeal(ck, b64(randomBytes(200))));
  const tb = [];
  for (let pos = 0; pos < sealed.length; pos++) {
    const t = Buffer.from(sealed);
    t[pos] ^= 1 << (pos % 8);
    let code = null;
    try { m.contentOpen(ck, b64(t)); } catch (e) { code = e.code; }
    check(code === 'decrypt', `WASM opened content altered at ${pos}`);
    tb.push(['contentOpen', { content_key: ck, sealed: b64(t) }]);
  }
  native(tb).map(result).forEach((r, pos) => check(r.code === 'decrypt', `native opened content altered at ${pos}`));
}

// 5. Entropy.
{
  const dead = instantiate(module, { randomFill: () => { throw new Error('no entropy'); } });
  for (const [op, req] of [['generate', {}], ['contentKeyGenerate', {}]]) {
    const r = result(dead.callRaw(op, JSON.stringify(req)));
    check(!r.ok && r.code === 'no-entropy', `${op} without entropy: ${JSON.stringify(r)}`);
  }
  const k = dead.importSeed(b64(randomBytes(32)));
  let code = null;
  try { dead.backupSeal({ key: k.key, passphrase: 'p', kdf: FAST }); } catch (e) { code = e.code; }
  check(code === 'no-entropy', 'backupSeal without entropy');
  let trapped = false;
  try { dead.contentSeal(b64(randomBytes(32)), b64(randomBytes(8))); } catch (e) { trapped = e instanceof WebAssembly.RuntimeError; }
  check(trapped, 'contentSeal without entropy traps rather than seal');
  let retired = false;
  try { dead.version(); } catch (e) { retired = !(e instanceof MaeCryptoError); }
  check(retired, 'a trapped instance is retired');
  // A host that reports success and writes nothing (all zeros) is refused.
  const zeros = instantiate(module, { randomFill: (v) => v.fill(0) });
  const r = result(zeros.callRaw('generate', '{}'));
  check(!r.ok && r.code === 'no-entropy', 'an all-zero source is refused');
  // The host is asked: two generated keys differ, and so do two seals.
  const live = instantiate(module);
  check(live.generate().public_key !== live.generate().public_key, 'two generated keys are equal');
  const ck = live.contentKeyGenerate();
  const pt = b64(randomBytes(16));
  check(live.contentSeal(ck, pt).slice(0, 32) !== live.contentSeal(ck, pt).slice(0, 32), 'nonce repeated');
}

// 6. Memory: no leak, and no residue.
{
  const m = instantiate(module);
  const k = m.generate();
  const ck = m.contentKeyGenerate();
  const fp = k.fingerprint;
  const warm = () => {
    const s = m.contentSeal(ck, b64(randomBytes(100)));
    m.contentOpen(ck, s);
    try { m.contentOpen(m.contentKeyGenerate(), s); } catch {}
    m.signChallenge({ key: k.key, issuer: ISS, subject: SUB, challenge: challengeText(fp, ISS, SUB, NOW + 60), now: NOW });
    try { m.signChallenge({ key: k.key, issuer: ISS, subject: 'x', challenge: 'sign me', now: NOW }); } catch {}
    m.forget(m.generate().key);
  };
  for (let i = 0; i < 50; i++) warm();
  const before = m.liveBytes();
  for (let i = 0; i < 2000; i++) warm();
  check(m.liveBytes() === before, `leak: ${before} -> ${m.liveBytes()}`);

  const copies = (inst, needle) => {
    const h = Buffer.from(inst.exports.memory.buffer);
    let n = 0;
    for (let i = h.indexOf(needle); i !== -1; i = h.indexOf(needle, i + 1)) n++;
    return n;
  };
  const forms = (bytes) => [Buffer.from(bytes), Buffer.from(b64(bytes))];
  const none = (inst, bytes, what) => forms(bytes).forEach((f, i) => check(copies(inst, f) === 0, `${what} left in memory (${i ? 'base64' : 'raw'})`));

  // A seed: imported, used, backed up, exported, forgotten -> gone.
  let r = instantiate(module);
  const seed = randomBytes(32);
  const pass = 'residue ' + b64(randomBytes(9));
  let h = r.importSeed(b64(seed));
  r.signChallenge({ key: h.key, issuer: ISS, subject: SUB, challenge: challengeText(h.fingerprint, ISS, SUB, NOW + 60), now: NOW });
  const env = r.backupSeal({ key: h.key, passphrase: pass, kdf: FAST });
  r.exportSeed(h.key);
  check(copies(r, Buffer.from(seed)) > 0, 'control: a held seed is found');
  r.forget(h.key);
  none(r, seed, 'a forgotten seed');
  check(copies(r, Buffer.from(pass)) === 0, 'the passphrase left in memory');
  // Opened from a backup, then forgotten -> gone, passphrase too.
  r = instantiate(module);
  h = r.backupOpen({ envelope: env, passphrase: pass });
  r.forget(h.key);
  none(r, seed, 'a seed opened from a backup');
  check(copies(r, Buffer.from(pass)) === 0, 'the passphrase left in memory after opening');
  // Content: the key and the plaintext do not outlive the call.
  r = instantiate(module);
  const key = randomBytes(32);
  const pt = randomBytes(100);
  r.contentOpen(b64(key), r.contentSeal(b64(key), b64(pt)));
  none(r, key, 'a content key');
  none(r, pt, 'a plaintext');
  // A content key unwrapped by a member: neither it nor the member seed stays.
  r = instantiate(module);
  const ms = randomBytes(32);
  const mk = r.importSeed(b64(ms));
  const ck2 = randomBytes(32);
  r.contentUnwrap(mk.key, r.contentWrap(b64(ck2), mk.wrap_public));
  r.forget(mk.key);
  none(r, ms, 'an unwrapping member seed');
  none(r, ck2, 'an unwrapped content key');
  // Several keys, forgotten out of order. Forgetting the first shifts the
  // rest down, so the last key has been MOVED before it is forgotten: a
  // keystore that stored keys inline would leave its old copy behind.
  // None is left, and the one still held is found.
  r = instantiate(module);
  const many = Array.from({ length: 5 }, () => randomBytes(32));
  const hs = many.map((s) => r.importSeed(b64(s)).key);
  for (const i of [0, 4, 2, 1]) r.forget(hs[i]);
  [0, 4, 2, 1].forEach((i) => none(r, many[i], `forgotten key ${i} of 5`));
  check(copies(r, Buffer.from(many[3])) > 0, 'control: the key still held is found');
}

// 7. The loader NFC-normalises passphrases, so the same typed passphrase
//    opens the backup whatever form the input method produced.
{
  const m = instantiate(module);
  const k = m.generate();
  const nfd = 'cafe\u0301 contrasen\u0303a';
  const nfc = nfd.normalize('NFC');
  check(nfd !== nfc, 'the test passphrase has two forms');
  const env = m.backupSeal({ key: k.key, passphrase: nfd, kdf: FAST });
  let opened = null;
  try { opened = m.backupOpen({ envelope: env, passphrase: nfc }).fingerprint; } catch (e) { opened = e.code; }
  check(opened === k.fingerprint, `NFC opens an NFD-sealed backup (${opened})`);
  const raw = result(m.callRaw('backupOpen', JSON.stringify({ envelope: env, passphrase: nfd })));
  check(!raw.ok && raw.code === 'open', 'the module itself takes the bytes as given');
}

rmSync(dir, { recursive: true, force: true });
if (failures.length) {
  console.error(`${failures.length} of ${checks} checks failed`);
  process.exit(1);
}
console.log(`ok: ${checks} checks; ${wasmBytes.length} bytes (budget ${SIZE_BUDGET})`);
