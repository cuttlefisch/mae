// mae-org's WASM loader: synchronous, dependency-free, host-neutral.
// SPDX-License-Identifier: GPL-3.0-or-later
//
// The module's ABI (shared/org-wasm/src/lib.rs) is six exports over
// linear memory. This file is the only code that drives it; a request is a
// plain object (serialised with JSON.stringify), a response is parsed
// JSON, and an `{"ok": false}` response throws.
//
// Every span in a response is a UTF-8 BYTE offset into the request's
// `text`, not a JavaScript string index (UTF-16 code units): convert with
// `new TextEncoder().encode(text)` before slicing.

// A Map, not an object: `OPS['__proto__']` or `OPS.toString` must not
// name an operation.
const OPS = new Map([
  ['semantics', 1],
  ['renderHast', 2],
  ['renderHtml', 3],
  ['text', 4],
  ['classifyLink', 5],
  ['version', 6],
  ['syntax', 7],
  ['syntaxSection', 8],
  ['headingsMissingCustomId', 9],
  ['customIdEdits', 10],
  ['duplicateCustomIds', 11],
  ['replaceCustomId', 12],
]);

const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });

/**
 * Instantiates the module from its bytes, synchronously.
 * @param {BufferSource | WebAssembly.Module} bytes the .wasm file, or a compiled module
 * @returns {MaeOrg}
 */
export function instantiate(bytes) {
  const module = bytes instanceof WebAssembly.Module ? bytes : new WebAssembly.Module(bytes);
  return new MaeOrg(seeded(new WebAssembly.Instance(module, {})), module);
}

/**
 * Seeds an instance's hash-map keys with 128 random bits: the module keys
 * maps by document text, and its own randomness is fixed, so it refuses to
 * run unseeded. No secure source of randomness is an error, not a
 * fallback.
 */
function seeded(instance) {
  const crypto = globalThis.crypto;
  if (!crypto || typeof crypto.getRandomValues !== 'function') {
    throw new Error('mae-org needs crypto.getRandomValues to seed the module');
  }
  const keys = crypto.getRandomValues(new BigUint64Array(2));
  instance.exports.mae_org_seed(keys[0], keys[1]);
  return instance;
}

/**
 * One module instance at a time. A call that throws for ANY reason
 * retires the instance it ran in: a trap (`WebAssembly.RuntimeError`),
 * but also the host's own stack running out inside a WASM frame (V8
 * throws a `RangeError` there) or anything else that unwinds out of the
 * module mid-call. Such an exit skips the module's own cleanup, so its
 * memory holds leaked allocations and possibly a borrowed result slot;
 * reusing it leaks on every call and can later trap out of bounds.
 *
 * The failing call rethrows its error. The NEXT call runs in a fresh
 * instance of the same compiled module (`restarts` counts them), so one
 * pathological document costs one failed call, not the loader. An object
 * built from a bare instance (no module) cannot restart: after a failed
 * call every later call throws.
 */
export class MaeOrg {
  constructor(instance, module = null) {
    this.module = module;
    this.exports = instance.exports;
    this.broken = null;
    this.restarts = 0;
  }

  /**
   * Runs operation `op` on a request string; returns the response string.
   * A request that is not a string is a `TypeError`, thrown before
   * anything else: converting it would run the caller's code (`toString`),
   * which could call back into this object mid-call.
   */
  callRaw(op, request) {
    if (typeof request !== 'string') {
      throw new TypeError(`mae-org: a request must be a string, not ${typeof request}`);
    }
    const code = typeof op === 'string' ? OPS.get(op) : undefined;
    if (code === undefined) {
      throw new Error(`unknown operation ${typeof op === 'string' ? op : typeof op}`);
    }
    // Encoded before the instance is chosen, so nothing after this point
    // runs while a call is in flight.
    const input = encoder.encode(request);
    if (this.broken) {
      if (!this.module) {
        throw new Error(`mae-org instance unusable after a failed call: ${this.broken.message}`);
      }
      this.exports = seeded(new WebAssembly.Instance(this.module, {})).exports;
      this.broken = null;
      this.restarts += 1;
    }
    const e = this.exports;
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
      // Whatever unwound out of the module, its state is not to be trusted.
      this.broken = err;
      throw err;
    }
  }

  /** Runs `op` on a request object; returns the result, or throws. */
  call(op, request = {}) {
    const response = JSON.parse(this.callRaw(op, JSON.stringify(request)));
    if (!response.ok) throw new Error(`mae-org ${op}: ${response.error}`);
    return response.result;
  }

  // KB data, not display text: titles, aliases, property values and link
  // descriptions are the source, hidden constructs spelled out. Show a
  // render's `nodes` (title_unlinked …) or `text` instead.
  semantics(request) { return this.call('semantics', request); }
  renderHast(request) { return this.call('renderHast', request); }
  renderHtml(request) { return this.call('renderHtml', request); }
  text(request) { return this.call('text', request); }
  classifyLink(request) { return this.call('classifyLink', request); }
  version() { return this.call('version'); }
  // Highlighting spans for an editor (flat [kind, start, end] triples, UTF-8
  // byte offsets) and the regions syntaxSection can re-highlight alone.
  syntax(request) { return this.call('syntax', request); }
  syntaxSection(request) { return this.call('syntaxSection', request); }
  // Stable heading anchors (UTF-8 byte offsets, like every span): the
  // headline lines needing a CUSTOM_ID; the edits giving them one (`uuids`,
  // lowercase UUID v4s, at least as many as there are headlines; edits
  // descending); the CUSTOM_IDs several headlines share; and the edit that
  // replaces one of them.
  headingsMissingCustomId(request) { return this.call('headingsMissingCustomId', request); }
  customIdEdits(request) { return this.call('customIdEdits', request); }
  duplicateCustomIds(request) { return this.call('duplicateCustomIds', request); }
  replaceCustomId(request) { return this.call('replaceCustomId', request); }

  /** Bytes the module has allocated and not freed (for leak checks). */
  liveBytes() { return this.exports.mae_org_live_bytes() >>> 0; }
}
