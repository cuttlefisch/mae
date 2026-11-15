// mae-crypto's WASM loader: synchronous, dependency-free, host-neutral.
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The module's ABI (shared/crypto-wasm/src/lib.rs) is five exports over
// linear memory and one import, `mae_crypto_host.random_fill(ptr, len)`,
// which this loader backs with `crypto.getRandomValues`. No secure source
// is an error at load time, never a fallback. A request is a plain object
// (serialised with JSON.stringify), a response is parsed JSON, and an
// `{"ok": false}` response throws a MaeCryptoError carrying its `code`.
//
// The contract (operations, fields, codes, the backup format) is in
// docs/E2E_ENCRYPTION.md, "Browser key material", and mae-crypto.d.ts.

// A Map, not an object: `OPS['__proto__']` must not name an operation.
const OPS = new Map([
  ['version', 1],
  ['generate', 2],
  ['importSeed', 3],
  ['exportSeed', 4],
  ['forget', 5],
  ['keyInfo', 6],
  ['fingerprint', 7],
  ['checkChallenge', 8],
  ['signChallenge', 9],
  ['backupSeal', 10],
  ['backupOpen', 11],
  ['backupParams', 12],
  ['contentKeyGenerate', 13],
  ['contentSeal', 14],
  ['contentOpen', 15],
  ['contentWrap', 16],
  ['contentUnwrap', 17],
]);

const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });

/** An `{"ok": false}` response: `code` is the stable, machine-readable reason. */
export class MaeCryptoError extends Error {
  constructor(op, code, message) {
    super(`mae-crypto ${op}: ${message}`);
    this.name = 'MaeCryptoError';
    this.op = op;
    this.code = code;
  }
}

function hostRandom() {
  const c = globalThis.crypto;
  if (!c || typeof c.getRandomValues !== 'function') {
    throw new Error('mae-crypto needs crypto.getRandomValues');
  }
  return (view) => {
    // getRandomValues refuses more than 65536 bytes per call.
    for (let i = 0; i < view.length; i += 65536) {
      c.getRandomValues(view.subarray(i, Math.min(view.length, i + 65536)));
    }
  };
}

/**
 * Instantiates the module from its bytes (or a compiled module), synchronously.
 * `randomFill` replaces the entropy source: for tests only.
 */
export function instantiate(bytes, { randomFill } = {}) {
  const module = bytes instanceof WebAssembly.Module ? bytes : new WebAssembly.Module(bytes);
  const fill = randomFill ?? hostRandom();
  let memory = null;
  const imports = {
    mae_crypto_host: {
      random_fill(ptr, len) {
        try {
          fill(new Uint8Array(memory.buffer, ptr >>> 0, len >>> 0));
          return 0;
        } catch {
          return 1;
        }
      },
    },
  };
  const instance = new WebAssembly.Instance(module, imports);
  memory = instance.exports.memory;
  return new MaeCrypto(instance.exports);
}

/**
 * One module instance and the member keys it holds (by handle). A call
 * that throws for any reason other than an error response (a trap, the
 * host's stack running out) RETIRES the instance: every later call throws.
 * It is not restarted behind the caller's back, because a fresh instance
 * holds none of the keys and reissues their handles to new keys.
 */
export class MaeCrypto {
  constructor(exports) {
    this.exports = exports;
    this.broken = null;
  }

  /** Runs operation `op` on a request string; returns the response string. */
  callRaw(op, request) {
    if (typeof request !== 'string') {
      throw new TypeError(`mae-crypto: a request must be a string, not ${typeof request}`);
    }
    const code = typeof op === 'string' ? OPS.get(op) : undefined;
    if (code === undefined) {
      throw new Error(`unknown operation ${typeof op === 'string' ? op : typeof op}`);
    }
    if (this.broken) {
      throw new Error(`mae-crypto instance retired after a failed call: ${this.broken.message}`);
    }
    const input = encoder.encode(request);
    const e = this.exports;
    try {
      // Pointers and lengths are u32: `>>> 0` reads them unsigned past 2 GiB.
      const ptr = e.mae_crypto_alloc(input.length) >>> 0;
      // Views are taken after every call into the module: memory may grow.
      new Uint8Array(e.memory.buffer, ptr, input.length).set(input);
      input.fill(0);
      const len = e.mae_crypto_call(code, ptr, input.length) >>> 0;
      try {
        const at = e.mae_crypto_result_ptr() >>> 0;
        return decoder.decode(new Uint8Array(e.memory.buffer, at, len));
      } finally {
        e.mae_crypto_result_free();
      }
    } catch (err) {
      this.broken = err;
      throw err;
    }
  }

  /** Runs `op` on a request object; returns the result, or throws. */
  call(op, request = {}) {
    const response = JSON.parse(this.callRaw(op, JSON.stringify(request)));
    if (!response.ok) throw new MaeCryptoError(op, response.code, response.error);
    return response.result;
  }

  version() { return this.call('version'); }

  // Member keys, by handle. The seed stays in the module; it leaves only
  // through exportSeed (an explicit export) or backupSeal.
  generate() { return this.call('generate'); }
  importSeed(seed) { return this.call('importSeed', { seed }); }
  exportSeed(key) { return this.call('exportSeed', { key }).seed; }
  forget(key) { return this.call('forget', { key }).forgotten; }
  keyInfo(key) { return this.call('keyInfo', { key }); }
  fingerprint(publicKey) { return this.call('fingerprint', { public_key: publicKey }).fingerprint; }

  // The identity-binding challenge: signed only if canonical, for this key,
  // unexpired, and issued to the issuer and subject stated here. `now` is
  // unix seconds (default: this host's clock).
  checkChallenge({ key, issuer, subject, challenge, now = nowSeconds() }) {
    return this.call('checkChallenge', { key, issuer, subject, challenge, now }).challenge;
  }
  signChallenge({ key, issuer, subject, challenge, now = nowSeconds() }) {
    return this.call('signChallenge', { key, issuer, subject, challenge, now });
  }

  // The key backup (envelope v1). The passphrase is NFC-normalised here, as
  // the format requires, so the same typed passphrase opens it anywhere.
  backupSeal({ key, passphrase, kdf }) {
    const req = { key, passphrase: nfc(passphrase) };
    if (kdf !== undefined) req.kdf = kdf;
    return this.call('backupSeal', req).envelope;
  }
  backupOpen({ envelope, passphrase }) {
    return this.call('backupOpen', { envelope, passphrase: nfc(passphrase) });
  }
  backupParams(envelope) { return this.call('backupParams', { envelope }); }

  // ADR-037 content crypto (for later use): base64 in, base64 out.
  contentKeyGenerate() { return this.call('contentKeyGenerate').content_key; }
  contentSeal(contentKey, plaintext) {
    return this.call('contentSeal', { content_key: contentKey, plaintext }).sealed;
  }
  contentOpen(contentKey, sealed) {
    return this.call('contentOpen', { content_key: contentKey, sealed }).plaintext;
  }
  contentWrap(contentKey, wrapPublic) {
    return this.call('contentWrap', { content_key: contentKey, wrap_public: wrapPublic }).wrapped;
  }
  contentUnwrap(key, wrapped) {
    return this.call('contentUnwrap', { key, wrapped }).content_key;
  }

  /** Bytes the module has allocated and not freed (for leak checks). */
  liveBytes() { return this.exports.mae_crypto_live_bytes() >>> 0; }
}

function nowSeconds() {
  return Math.floor(Date.now() / 1000);
}

function nfc(passphrase) {
  if (typeof passphrase !== 'string') {
    throw new TypeError('mae-crypto: a passphrase must be a string');
  }
  return passphrase.normalize('NFC');
}
