// SPIKE: loader for MAE's envelope crypto WASM module (shared/crypto-wasm).
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The module imports one function, `mae_crypto_host.random_fill(ptr, len)`,
// and this loader backs it with `crypto.getRandomValues`. No secure source
// is an error at load time, never a fallback.

const OPS = { seal: 1, open: 2, wrap: 3, unwrap: 4, wrapPublic: 5 };
const ERRORS = new Map([
  [-1, 'decrypt'],
  [-2, 'malformed'],
  [-3, 'bad-key'],
  [-4, 'short'],
  [-5, 'op'],
]);

export class CryptoError extends Error {
  constructor(code) {
    super(`mae-crypto: ${ERRORS.get(code) ?? code}`);
    this.code = ERRORS.get(code) ?? String(code);
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
 * @param {BufferSource} bytes the .wasm file
 * @param {(view: Uint8Array) => void} [fill] entropy source; tests only
 */
export function instantiate(bytes, fill = hostRandom()) {
  let memory = null;
  const imports = {
    mae_crypto_host: {
      random_fill(ptr, len) {
        try {
          fill(new Uint8Array(memory.buffer, ptr, len));
          return 0;
        } catch {
          return 1;
        }
      },
    },
  };
  const instance = new WebAssembly.Instance(new WebAssembly.Module(bytes), imports);
  memory = instance.exports.memory;
  return new MaeCrypto(instance.exports);
}

export class MaeCrypto {
  constructor(exports) {
    this.x = exports;
  }

  /** Runs `op` on `request` (Uint8Array); returns a copy of the response. */
  call(op, request) {
    const x = this.x;
    const ptr = x.mae_crypto_alloc(request.length);
    new Uint8Array(x.memory.buffer, ptr, request.length).set(request);
    const n = x.mae_crypto_call(op, ptr, request.length);
    if (n < 0) throw new CryptoError(n);
    const out = new Uint8Array(x.memory.buffer, x.mae_crypto_result_ptr(), n).slice();
    x.mae_crypto_result_free();
    return out;
  }

  seal(key, plaintext) {
    return this.call(OPS.seal, concat(key, plaintext));
  }
  open(key, sealed) {
    return this.call(OPS.open, concat(key, sealed));
  }
  wrap(contentKey, wrapPublic) {
    return this.call(OPS.wrap, concat(contentKey, wrapPublic));
  }
  unwrap(seed, wrapped) {
    return this.call(OPS.unwrap, concat(seed, wrapped));
  }
  wrapPublic(seed) {
    return this.call(OPS.wrapPublic, seed);
  }
  liveBytes() {
    return this.x.mae_crypto_live_bytes();
  }
}

function concat(a, b) {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}
