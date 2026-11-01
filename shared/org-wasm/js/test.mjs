// The WASM module against the same code built natively, and its budgets.
// SPDX-License-Identifier: GPL-3.0-or-later
//
//   MAE_ORG_WASM=path/to/mae_org_wasm.wasm MAE_ORG_NATIVE=path/to/mae-org-native \
//     node shared/org-wasm/js/test.mjs
//
// (`make org-wasm-test` sets both.) Checks, each a failure on its own:
// 1. size: the artifact is within SIZE_BUDGET bytes;
// 2. differential: every response for every request over the corpus is
//    byte-identical between the module and the native build;
// 3. memory: 10,000 calls leave live allocations and linear memory where
//    they were after warm-up;
// 4. the module loads synchronously and imports nothing; a deep document
//    renders without exhausting its stack; errors are responses; a call
//    that throws for any reason retires its instance; the shadow stack
//    sits below the data.

import { instantiate, MaeOrg } from './mae-org.mjs';
import { readFileSync, writeFileSync, mkdtempSync, statSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

// Measured with rustc 1.98, `--profile wasm` (opt-level "s"): 611,525 bytes
// on 2026-11-01; 648,998 bytes after the pre-tag review fixes (babel
// `:exports`, anchors, special table rows, the splice); 707,647 bytes
// after the parser's own review fixes (Org's split radio regexps, keyword
// reading, `#+LINK` expansion). The first budget, 700,000 (~15% over the
// first measurement), caught that growth as it should; it was raised once,
// to 800,000 (~13% over the tagged size), as recorded in ADR-112's O3
// note. The module runs at build time in Node, so the budget guards
// against accidental bloat (a stray dependency, a generic blow-up), not a
// download size; raising it again is a decision to record there, not a
// number to bump.
const SIZE_BUDGET = 800_000;
const OPS = { semantics: 1, renderHast: 2, renderHtml: 3, text: 4, classifyLink: 5, version: 6 };

const root = join(dirname(fileURLToPath(import.meta.url)), '../../..');
const wasmPath = process.env.MAE_ORG_WASM;
const nativePath = process.env.MAE_ORG_NATIVE;
if (!wasmPath || !nativePath) {
  console.error('set MAE_ORG_WASM and MAE_ORG_NATIVE (see `make org-wasm-test`)');
  process.exit(2);
}

const failures = [];
const fail = (what) => { failures.push(what); console.error(`FAIL ${what}`); };

// 1. Size.
const size = statSync(wasmPath).size;
console.log(`size: ${size} bytes (budget ${SIZE_BUDGET})`);
if (size > SIZE_BUDGET) fail(`size ${size} > budget ${SIZE_BUDGET}`);

// 4. Synchronous load, no imports.
const bytes = readFileSync(wasmPath);
const module = new WebAssembly.Module(bytes);
if (WebAssembly.Module.imports(module).length !== 0) fail('the module imports something');
const org = instantiate(module);

// The corpus: every tracked .org file the Rust tests read.
const listed = spawnSync('git', ['ls-files', '-z', '--', 'assets/*.org', 'tests/*.org', 'shared/org/tests/*.org'], { cwd: root, encoding: 'utf8' });
const files = listed.stdout.split('\0').filter(Boolean);
if (files.length < 300) fail(`corpus has only ${files.length} files`);

/** Resolves every second KB target, and offers a hostile href for some. */
function resolverFor(sem) {
  const targets = {};
  const ids = [...new Set(sem.links.map((l) => l.target).concat(sem.transclusions.map((t) => t.member)))].sort();
  ids.forEach((id, i) => {
    if (i % 3 === 0) targets[id] = { href: `/n/${i}.html`, title: i % 2 ? `T${i}` : null };
    if (i % 3 === 1) targets[id] = { href: 'javascript:alert(1)' };
  });
  return { targets, placeholder: '[?]' };
}

// 2. The differential.
const requests = [];
for (const f of files) {
  const text = readFileSync(join(root, f), 'utf8');
  const sem = org.semantics({ text }).semantics;
  const resolver = resolverFor(sem);
  requests.push([OPS.semantics, { text }]);
  requests.push([OPS.semantics, { text, semantics: { select_tags: ['export'], archived_trees: 'none' } }]);
  requests.push([OPS.renderHast, { text, resolver }]);
  requests.push([OPS.renderHtml, { text, resolver, render: { headline_ids: 'slug', images: true, property_drawers: true } }]);
  requests.push([OPS.text, { text }]);
  if (sem.nodes.length) requests.push([OPS.text, { text, node: sem.nodes[sem.nodes.length - 1].id }]);
  for (const l of sem.links.slice(0, 5)) requests.push([OPS.classifyLink, { path: l.path }]);
}
requests.push([OPS.version, {}]);
requests.push([OPS.renderHast, { text: 'x', bogus: 1 }]);
const lines = requests.map(([op, r]) => `${op}\t${JSON.stringify(r)}`);
const wasmOut = requests.map(([op], i) => org.callRaw(Object.keys(OPS)[op - 1], lines[i].slice(lines[i].indexOf('\t') + 1)));
const dir = mkdtempSync(join(tmpdir(), 'mae-org-diff-'));
try {
  writeFileSync(join(dir, 'requests'), lines.join('\n') + '\n');
  const native = spawnSync(nativePath, [join(dir, 'requests')], { encoding: 'utf8', maxBuffer: 1 << 30 });
  if (native.status !== 0) fail(`native exited ${native.status}: ${native.stderr}`);
  const nativeOut = native.stdout.split('\n').slice(0, -1);
  if (nativeOut.length !== wasmOut.length) fail(`native gave ${nativeOut.length} responses, wasm ${wasmOut.length}`);
  let differ = 0;
  for (let i = 0; i < wasmOut.length; i++) {
    if (wasmOut[i] !== nativeOut[i]) {
      if (differ++ < 3) fail(`response ${i} differs (${lines[i].slice(0, 120)}...):\n wasm   ${wasmOut[i].slice(0, 300)}\n native ${(nativeOut[i] || '').slice(0, 300)}`);
    }
  }
  console.log(`differential: ${wasmOut.length} responses over ${files.length} files, ${differ} differ`);
  if (differ) fail(`${differ} responses differ`);
} finally {
  rmSync(dir, { recursive: true, force: true });
}

// 4. Errors are responses; the hostile resolver href never appears.
const bad = JSON.parse(org.callRaw('renderHast', '{"text": 1}'));
if (bad.ok !== false) fail('a malformed request did not give an error response');
const broken = JSON.parse(org.callRaw('semantics', '{"text": "x"'));
if (broken.ok !== false) fail('broken JSON did not give an error response');
// (Semantics may report a hostile link's path as data; a rendering may not
// carry it in any attribute.)
const rendered = wasmOut.filter((_, i) => requests[i][0] === OPS.renderHast || requests[i][0] === OPS.renderHtml);
if (rendered.some((r) => /href[^,}]*javascript:/i.test(r))) fail('a hostile href reached a rendering');
// The shape mae-org.d.ts declares for a node's metadata.
const meta = org.renderHtml({ text: '* T [[https://x.example][l]]\n:PROPERTIES:\n:ID: t\n:END:\n' }).nodes[0];
for (const k of ['id', 'anchor', 'title', 'title_unlinked', 'aliases', 'properties']) {
  if (!(k in meta)) fail(`node metadata has no ${k}`);
}
if (/<a\b/.test(meta.title_unlinked)) fail(`title_unlinked holds a link: ${meta.title_unlinked}`);
const v = org.version();
console.log(`version: api ${v.api}, crate ${v.crate}`);

// 4. The loader's own input checks (review): a request that is not a
// string is refused before it is converted (its `toString` could call back
// into the loader mid-call and retire the instance the outer call then
// uses), and only the six operations are operations: no inherited name.
{
  let called = false;
  const evil = { toString() { called = true; org.callRaw('version', '{}'); return '{"text": "x"}'; } };
  let threw = null;
  try { org.callRaw('renderHtml', evil); } catch (e) { threw = e; }
  if (!(threw instanceof TypeError)) fail(`a non-string request was not a TypeError: ${threw}`);
  if (called) fail('a non-string request was converted (its toString ran)');
  for (const bad of [12345, null, undefined, { text: 'x' }]) {
    try { org.callRaw('text', bad); fail(`request ${bad} accepted`); } catch (e) {
      if (!(e instanceof TypeError)) fail(`request ${bad}: ${e}`);
    }
  }
  for (const op of ['__proto__', 'toString', 'constructor', 'hasOwnProperty', 'valueOf', '']) {
    try { org.callRaw(op, '{}'); fail(`operation ${op} accepted`); } catch (e) {
      if (!/unknown operation/.test(e.message)) fail(`operation ${op}: ${e}`);
    }
  }
  if (org.broken) fail('an input check retired the instance');
  if (org.callRaw('text', '{"text": "ok"}').indexOf('ok') < 0) fail('the loader is unusable after input checks');
}

// 4. An instance a call failed in is not used again, whatever the call
// threw (a trap, or a RangeError: the host's stack exhausted mid-call).
// Stubs with no module to restart from, so every later call must refuse.
for (const thrown of [new WebAssembly.RuntimeError('unreachable'), new RangeError('stack'), new TypeError('x')]) {
  const failing = new MaeOrg({
    exports: {
      memory: new WebAssembly.Memory({ initial: 1 }),
      mae_org_alloc: () => 0,
      mae_org_call: () => { throw thrown; },
      mae_org_result_ptr: () => 0,
      mae_org_result_free: () => {},
      mae_org_live_bytes: () => 0,
    },
  });
  let refused = 0;
  for (let i = 0; i < 3; i++) {
    try { failing.callRaw('text', '{}'); } catch (e) { refused += /unusable/.test(e.message) ? 1 : 0; }
  }
  if (refused !== 2) fail(`an instance was used again after a call threw ${thrown.constructor.name}`);
}

// 4. The real thing: the host stack exhausted inside the module, in a
// child Node with a small stack (test-stack.mjs). The loader retires the
// instance and the next call runs in a fresh one.
const stack = spawnSync(process.execPath, ['--stack-size=200', join(dirname(fileURLToPath(import.meta.url)), 'test-stack.mjs'), wasmPath], { encoding: 'utf8' });
process.stdout.write(stack.stdout);
if (stack.status !== 0) fail(`host-stack exhaustion: ${stack.stderr.trim() || `exit ${stack.status}`}`);

// 4. Linear memory puts the shadow stack FIRST, below the data: a stack
// overflow runs below address 0 and traps out of bounds rather than
// writing over static data or the heap. Read from the module itself.
{
  const b = bytes;
  let p = 8;
  const uleb = () => { let r = 0, sh = 0, x; do { x = b[p++]; r += (x & 0x7f) * 2 ** sh; sh += 7; } while (x & 0x80); return r; };
  const sleb = () => { let r = 0, sh = 0, x; do { x = b[p++]; r |= (x & 0x7f) << sh; sh += 7; } while (x & 0x80); if (sh < 32 && (x & 0x40)) r |= ~0 << sh; return r; };
  let stackTop = null;
  let lowestData = Infinity;
  while (p < b.length) {
    const id = b[p++];
    const len = uleb();
    const end = p + len;
    if (id === 6 && stackTop === null) {
      // The first global is __stack_pointer: (mut i32) (i32.const N).
      uleb();
      const type = b[p++], mut = b[p++], op = b[p++];
      if (type === 0x7f && mut === 1 && op === 0x41) stackTop = sleb();
    }
    if (id === 11) {
      const n = uleb();
      for (let i = 0; i < n; i++) {
        const flag = uleb();
        if (flag !== 0) break;
        if (b[p++] !== 0x41) break;
        lowestData = Math.min(lowestData, sleb());
        p++; // end
        p += uleb();
      }
    }
    p = end;
  }
  console.log(`layout: stack 0..${stackTop}, data from ${lowestData}`);
  if (stackTop !== 4 << 20) fail(`the shadow stack is ${stackTop} bytes, not the 4 MiB build.rs sets`);
  if (!(stackTop <= lowestData)) fail('the shadow stack is not below the data (no --stack-first)');
}

// 4. A deep document (the parse cap raised to its ceiling) renders.
const deep = Array.from({ length: 1000 }, (_, i) => ' '.repeat(i) + '- *x /y/*\n').join('');
const deepOut = org.renderHtml({ text: deep, parse: { max_depth: 1024 } });
console.log(`deep document: ${deepOut.body.length} bytes of HTML, ${deepOut.diagnostics.length} diagnostics`);

// 3. Memory over 10,000 calls.
const doc = readFileSync(join(root, 'shared/org/tests/render/links.org'), 'utf8');
const req = { text: doc, resolver: { targets: { known: { href: '/k.html' } } } };
for (let i = 0; i < 100; i++) { org.renderHast(req); org.text(req); org.semantics(req); }
const live0 = org.liveBytes();
const mem0 = org.exports.memory.buffer.byteLength;
const t0 = process.hrtime.bigint();
for (let i = 0; i < 10_000; i++) {
  switch (i % 3) {
    case 0: org.renderHast(req); break;
    case 1: org.text(req); break;
    default: org.semantics(req);
  }
}
const ms = Number(process.hrtime.bigint() - t0) / 1e6;
const live1 = org.liveBytes();
const mem1 = org.exports.memory.buffer.byteLength;
console.log(`memory: live ${live0} -> ${live1} bytes, linear memory ${mem0} -> ${mem1} bytes; 10,000 calls in ${ms.toFixed(0)} ms`);
if (live1 !== live0) fail(`live allocations grew by ${live1 - live0} bytes over 10,000 calls`);
if (mem1 !== mem0) fail(`linear memory grew by ${mem1 - mem0} bytes over 10,000 calls`);

if (failures.length) {
  console.error(`${failures.length} failure(s)`);
  process.exit(1);
}
console.log('ok');
