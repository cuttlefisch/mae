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
//    renders without exhausting its stack; errors are responses.

import { instantiate } from './mae-org.mjs';
import { readFileSync, writeFileSync, mkdtempSync, statSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

// Measured with rustc 1.98, `--profile wasm` (opt-level "s"): 607,475 bytes
// on 2026-10-31. The budget leaves ~15% for growth; raising it is a
// decision to record in ADR-112's O3 note, not a number to bump.
const SIZE_BUDGET = 700_000;
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
const v = org.version();
console.log(`version: api ${v.api}, crate ${v.crate}`);

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
