# Where the browser gets org structure: ADR-100 D4 spike

**Status:** Run, 2026-08-05, against `main` at `e4a6f77d`. Complete — both open conditions
(latency, bundle size) are now measured.
**Gates:** ADR-100 D4.
**Artifacts:** `shared/kb/examples/org_parse_latency.rs` (the latency measurement, reproducible).

**Bottom line up front.** D4 posed a three-way choice — JS `uniorg`, daemon-served decoration
ranges, or MAE's parser compiled to WASM. Measuring changed the question:

1. **Neither latency nor bundle size constrains the choice.** Parse is microseconds; the scanning
   core is ~16 KB gzipped against a 115-byte empty-crate control.
2. **Latency specifically eliminates option (b).** Native parse is **microseconds** — even at 3× WASM overhead the
   worst real node parses in well under 0.15 ms, while a network round-trip is three orders of
   magnitude worse than the parse it would replace.
3. **The "reuse the existing parser" premise was half wrong.** `shared/kb/src/org.rs` has **no
   inline-emphasis scanner at all**, so it cannot produce most of ADR-100 D3's decoration set on
   its own.
4. **But the missing half already exists elsewhere, as offsets.** `mae-export`'s
   `find_markup_end_str` returns `Option<(usize, &str)>` — the inline logic is already
   offset-based; only its public wrapper formats to HTML.

So the answer is neither "reuse org.rs" nor "write it in JS", but **extract the scanners both Rust
parsers already contain into a WASM-compilable leaf crate** — which is also the fix for the
duplication that made D4 hard in the first place.

## What was measured

`cargo run --release --example org_parse_latency -p mae-kb`, over the real bundled corpus (98 org
files, 319,092 bytes, largest 11,717), 200 reps per file after a warm-up:

| Operation | p50 | p95 | max |
|---|---|---|---|
| `parse_org_multi_result` (full structure) | 8.6 µs | 21.7 µs | **39.3 µs** |
| `parse_typed_links` | 3.2 µs | 4.8 µs | 10.7 µs |
| `rewrite_links_with_types` (link scan) | 1.9 µs | 2.9 µs | 6.2 µs |
| `compute_code_block_ranges` | 1.0 µs | 1.4 µs | 3.4 µs |

An incidental but useful observation: the slowest structure parse is a **1,371-byte** file, not the
11,717-byte one. Structure-parse cost tracks heading/node count, not length — so a long flat note
is cheap and a short deeply-headed hub note is not. A budget expressed per-kilobyte would be the
wrong shape.

**Interpretation.** A debounced live-preview decorator re-parses on keystrokes within a 16 ms frame
budget. At 39 µs native, even a pessimistic 3× WASM factor leaves ~0.12 ms — roughly 1% of a frame.
Latency does not constrain this decision, which is worth stating plainly because it was the reason
option (b) existed.

## What the code says about reuse

**`org.rs` is a structure parser, not a markup parser.** Its only contact with `*`/`=`/`~` is
detecting heading stars (`heading_level`) and *skipping* links inside verbatim/code spans. There is
no `*bold*`, `/italic/`, `+strike+` recognition anywhere in its 2,518 lines. So compiling it to WASM
would deliver headings, drawers, properties, typed links and code-block ranges — and none of the
emphasis decoration D3 calls for.

**`mae-export` has the inline half, already as offsets.** `is_markup_start(text, pos)` and
`find_markup_end_str(text, start, marker) -> Option<(usize, &str)>` are exactly a range scanner;
`convert_inline_markup_str` merely wraps them in HTML formatting. Extracting a range-producing API
is a refactor of the public surface, not new parsing logic — which is a much better position than
"write a new inline scanner and keep it in sync".

**Neither crate is WASM-ready as-is.** `mae-kb` pulls `cozo`, `reqwest`, `notify`, `walkdir`;
`mae-export` pulls `mae-babel`, which exists to spawn processes. Both are hostile to
`wasm32-unknown-unknown`. The pure scanners are, however, genuinely separable: `org.rs`'s only
crate-internal dependencies are `KnowledgeBase`/`Node`/`NodeKind` (used by the file-import path,
not the scanners) and `compute_code_block_ranges`.

## The recommendation

**Extract a leaf crate — provisionally `shared/org-scan` — containing the pure text scanners from
both parsers, and compile *that* to WASM.**

- **Block structure, links, drawers, properties → from the extracted crate.** Drift here corrupts KB *semantics*: a mis-parsed `[[id:…]]` link or `:PROPERTIES:` drawer produces a wrong graph, not a wrong-looking one. This is exactly where a third independent implementation (option (a), `uniorg`) is most dangerous.
- **Inline emphasis → from the same crate**, via a range-producing API extracted from `mae-export`'s existing offset scanners.
- **Option (a) `uniorg` is rejected** for the semantic layer. If it were ever used for cosmetic emphasis only, the blast radius of a disagreement would be "text looks wrong", not "the graph is wrong" — but with the inline logic already existing in Rust as offsets, there is no reason to take even that.
- **Option (b) daemon-served ranges is rejected** on the latency measurement above.

**The extraction is the real work, and it is also a principle #8 win.** MAE currently has two
hand-written org parsers with overlapping responsibilities. This consolidates the scanning core
into one place that native code and the browser both consume — the browser requirement paying for a
cleanup that was already overdue, rather than adding a third implementation.

**It is not free.** Extracting from two shipped, heavily-tested parsers carries real regression
risk, and it must be gated by a conformance test running the extracted crate and the current
implementations over the full bundled corpus and asserting identical output — before either call
site switches.

## Bundle size: measured, and not a constraint

The `wasm32-unknown-unknown` target was installed after the first pass of this spike, so the size
question is now answered rather than deferred.

**Method.** A probe crate containing the **real** scanner sources — extracted verbatim from
`shared/kb/src/{lib,org}.rs` and `crates/export/src/lib.rs`, not retyped or approximated — built as
a `cdylib` for `wasm32-unknown-unknown` with `opt-level="z"`, LTO, `codegen-units=1`,
`panic="abort"` and `strip=true`. A `#[no_mangle]` entry point calls every extracted function so
the optimiser cannot delete the code being measured.

**Control first.** An otherwise-identical crate whose entry point only returns a string length
compiles to **115 bytes**. So the figures below are essentially all real code rather than
toolchain floor — without this control the measurement would be worthless.

| Build | Size |
|---|---|
| Empty baseline (control) | **115 B** |
| Org scanning core | **35,687 B** (34.8 KiB) |
| Org scanning core, gzipped | **16,294 B** (15.9 KiB) |

Roughly 16 KB over the wire. For comparison, that is a fraction of a typical web font. **Bundle
size does not constrain this decision.**

**What the figure does and does not cover — stated so it is not over-read:**

- **Covered:** `compute_code_block_ranges`, `heading_level`, `next_link_span` + `LinkSpanMatch`, `rewrite_links_with_types`, `is_kb_node_id`, `split_link_target`, and `mae-export`'s `is_markup_start` / `find_markup_end_str`. That is the decoration core.
- **Not covered:** `parse_org_multi_result` (full structure — needs `Node`/`NodeKind`), drawer scanning, and `parse_typed_links` (needs `ParsedLink`). The real extracted crate will be larger, plausibly by a factor of two or three — which still lands well under 50 KB gzipped.
- **Not included:** `wasm-bindgen` glue (a JS shim plus some wasm), and `wasm-opt -Oz`, which was unavailable (`binaryen` not installed) and would *reduce* these numbers. The two effects push in opposite directions.

So this is a well-founded estimate of the right order of magnitude, not a final artifact size —
and the margin is large enough that the conclusion is not sensitive to the uncertainty.

**One consequence still to carry forward:** CI needs a `wasm32-unknown-unknown` target added to
`.github/actions/setup-rust` for any build or size gate, and that should land with the extraction
rather than after it.

## Reproducing

```bash
cargo run --release --example org_parse_latency -p mae-kb
```

Run from the repo root (it reads `assets/devpractices` and `assets/practices` by default; pass
directories as arguments to measure a different corpus).
