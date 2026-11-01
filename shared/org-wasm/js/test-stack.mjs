// The loader after the HOST stack runs out inside a WASM frame.
// SPDX-License-Identifier: GPL-3.0-or-later
//
//   node --stack-size=200 shared/org-wasm/js/test-stack.mjs path/to/mae_org_wasm.wasm
//
// (`test.mjs` spawns it.) V8 runs WASM frames on the host's own stack and
// throws a `RangeError`, not a `WebAssembly.RuntimeError`, when that stack
// runs out mid-call. The call unwinds without the module's cleanup, so the
// instance is left holding the request, half-built results and a borrowed
// response slot. A loader that keeps using it leaks on every such call and
// can later trap out of bounds (review finding, 87 MB over 50 calls).
//
// The probe burns a growing number of JS frames before a deep call until
// the call throws, proves the throw came from inside the module (the
// retired instance's live allocations grew), repeats it, and then requires
// that every later call runs in a clean instance: live bytes back at a
// fresh instance's level and the output correct. Exit 0 on success.

import { instantiate } from './mae-org.mjs';
import { readFileSync } from 'node:fs';

const wasm = process.argv[2];
const org = instantiate(readFileSync(wasm));
const fail = (what) => { console.error(`FAIL ${what}`); process.exit(1); };

const small = { text: '* Fine\n' };
const expected = org.renderHtml(small).body;
const fresh = org.liveBytes();
const deep = {
  text: Array.from({ length: 1024 }, (_, i) => ' '.repeat(i) + '- *x /y/*\n').join(''),
  parse: { max_depth: 1024 },
};
function burn(d, f) { return d === 0 ? f() : burn(d - 1, f) + 0; }

/** One deep call under `d` burnt frames: 'ok', 'js' (threw before the
 * module allocated anything) or 'wasm' (threw from inside the module). */
function attempt(d) {
  const ex = org.exports;
  const before = ex.mae_org_live_bytes() >>> 0;
  try {
    burn(d, () => org.call('semantics', deep));
    return 'ok';
  } catch (e) {
    if (!(e instanceof RangeError)) fail(`expected a RangeError, got ${e}`);
    // The instance the call ran in: a restart (after an earlier failure)
    // happens inside the call, before the module is entered, so it is the
    // current one, and a fresh instance starts with nothing allocated.
    const ran = org.exports;
    const base = ran === ex ? before : 0;
    return (ran.mae_org_live_bytes() >>> 0) > base ? 'wasm' : 'js';
  }
}

let depth = null;
for (let d = 0; d < 40_000 && depth === null; d += 7) {
  if (attempt(d) === 'wasm') depth = d;
}
if (depth === null) fail('no call ran out of host stack inside the module: the probe is vacuous');

let inWasm = 1;
for (let i = 0; i < 49; i++) {
  if (attempt(depth) === 'wasm') inWasm += 1;
}

const restarts = org.restarts;
// Every call after a failure: a clean instance, and correct output.
const after = org.renderHtml(small).body;
if (after !== expected) fail(`output after the failures differs: ${after}`);
const live = org.liveBytes();
if (live !== fresh) fail(`live bytes ${live} after ${inWasm} failed calls; a fresh instance holds ${fresh}`);
if (org.exports.memory.buffer.byteLength > 64 << 20) fail('linear memory grew past 64 MiB');
const ok = org.call('semantics', { text: '- *x /y/*\n' });
if (!ok.semantics) fail('a later call gave no semantics');
console.log(`stack: ${inWasm} calls ran out of host stack inside the module (at ${depth} burnt frames); ${restarts} restarts; live bytes ${live} (fresh ${fresh})`);
