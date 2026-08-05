# Where the browser gets org structure: ADR-100 D4 spike

**Status:** Run, 2026-08-05, against `main` at `e4a6f77d`. **Partial** — bundle size is
unmeasured, see Blocked below.
**Gates:** ADR-100 D4.
**Artifacts:** `shared/kb/examples/org_parse_latency.rs` (the latency measurement, reproducible).

**Bottom line up front.** D4 posed a three-way choice — JS `uniorg`, daemon-served decoration
ranges, or MAE's parser compiled to WASM. Measuring changed the question:

1. **Latency is a non-issue.** Native parse is **microseconds**, not milliseconds. Even at 3× WASM
   overhead the worst real node parses in well under 0.15 ms. This eliminates option (b) outright:
   a network round-trip is three orders of magnitude worse than the parse it would replace.
2. **The "reuse the existing parser" premise was half wrong.** `shared/kb/src/org.rs` has **no
   inline-emphasis scanner at all**, so it cannot produce most of ADR-100 D3's decoration set on
   its own.
3. **But the missing half already exists elsewhere, as offsets.** `mae-export`'s
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

## Blocked: bundle size is unmeasured

`wasm32-unknown-unknown` std is **not installed** on this machine, and the toolchain is Fedora's
distro `rustc` (1.96.1) rather than rustup — so there is no `rustup target add`. The matching
package exists (`rust-std-static-wasm32-unknown-unknown 1.96.1-1.fc44`) but installing it needs
root, which this spike did not assume.

So the size question is **open, and deliberately not estimated here**. A pure byte-scanner with no
heavy dependencies should be small, but "should be" is not a measurement, and this arc has already
had one design premise (a `tree-sitter-org` dependency, #657) fail because it was plausible rather
than checked.

**Two consequences to carry forward:**

- CI would need a `wasm32-unknown-unknown` target added to `.github/actions/setup-rust` for any build or size gate. That is a real CI change, and it should land with the extraction rather than after it.
- The size measurement is a prerequisite for adopting the recommendation, not a formality. If the extracted crate plus `wasm-bindgen` glue turns out to be large enough to hurt initial page load, the trade against a JS inline scanner reopens — for the *cosmetic* layer only; the semantic layer's argument does not depend on size.

## Reproducing

```bash
cargo run --release --example org_parse_latency -p mae-kb
```

Run from the repo root (it reads `assets/devpractices` and `assets/practices` by default; pass
directories as arguments to measure a different corpus).
