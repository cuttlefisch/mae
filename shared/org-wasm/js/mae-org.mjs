// mae-org's WASM loader: synchronous, dependency-free, host-neutral.
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The module's ABI (shared/org-wasm/src/lib.rs) is five exports over
// linear memory. This file is the only code that drives it; a request is a
// plain object (serialised with JSON.stringify), a response is parsed
// JSON, and an `{"ok": false}` response throws.

const OPS = Object.freeze({
  semantics: 1,
  renderHast: 2,
  renderHtml: 3,
  text: 4,
  classifyLink: 5,
  version: 6,
});

const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });

/**
 * Instantiates the module from its bytes, synchronously.
 * @param {BufferSource | WebAssembly.Module} bytes the .wasm file, or a compiled module
 * @returns {MaeOrg}
 */
export function instantiate(bytes) {
  const module = bytes instanceof WebAssembly.Module ? bytes : new WebAssembly.Module(bytes);
  return new MaeOrg(new WebAssembly.Instance(module, {}));
}

export class MaeOrg {
  constructor(instance) {
    this.exports = instance.exports;
    this.broken = null;
  }

  /** Runs operation `op` on a request string; returns the response string. */
  callRaw(op, request) {
    const code = OPS[op];
    if (code === undefined) throw new Error(`unknown operation ${op}`);
    // A trap (a panic, or the stack exhausted) leaves the module's memory
    // in whatever state it reached: the instance is not used again.
    if (this.broken) throw new Error(`mae-org instance unusable after a trap: ${this.broken.message}`);
    const e = this.exports;
    const input = encoder.encode(request);
    try {
      // Pointers and lengths are u32: `>>> 0` reads them unsigned past 2 GiB.
      const ptr = e.mae_org_alloc(input.length) >>> 0;
      // Views are taken after every call into the module: memory may grow.
      new Uint8Array(e.memory.buffer, ptr, input.length).set(input);
      const len = e.mae_org_call(code, ptr, input.length) >>> 0;
      try {
        const at = e.mae_org_result_ptr() >>> 0;
        return decoder.decode(new Uint8Array(e.memory.buffer, at, len));
      } finally {
        e.mae_org_result_free();
      }
    } catch (err) {
      if (err instanceof WebAssembly.RuntimeError) this.broken = err;
      throw err;
    }
  }

  /** Runs `op` on a request object; returns the result, or throws. */
  call(op, request = {}) {
    const response = JSON.parse(this.callRaw(op, JSON.stringify(request)));
    if (!response.ok) throw new Error(`mae-org ${op}: ${response.error}`);
    return response.result;
  }

  semantics(request) { return this.call('semantics', request); }
  renderHast(request) { return this.call('renderHast', request); }
  renderHtml(request) { return this.call('renderHtml', request); }
  text(request) { return this.call('text', request); }
  classifyLink(request) { return this.call('classifyLink', request); }
  version() { return this.call('version'); }

  /** Bytes the module has allocated and not freed (for leak checks). */
  liveBytes() { return this.exports.mae_org_live_bytes() >>> 0; }
}
