# ADR-119 spike results (2026-11-18)

Throwaway measurement code, run to prove or kill each architectural claim of ADR-119 before any engine
code. This file is the evidence the ADR cites. It records the method, the numbers and their limits. The
spike code itself is not committed.

## Method and limits

- **Input.** A 48-file sample bundle in the published shape (file-level and heading IDs,
  `CUSTOM_ID`s, `id:` links including `id:X::#Y`, tags, dated folders, src blocks in 10 languages plus
  an unknown one, 5 DOT diagrams, a 10-column table, a 25-heading page). The content is entirely
  synthetic.
- **Scaling.** The sample was copied K times with every UUID remapped per copy (uuid5), giving 1,536,
  5,040 and 10,032 files. Every 7th copy links back to the first.
- **Density.** The synthetic bundles have 3.2 IDs and 2.5 note links per page. The real published
  bundle that motivates the ADR has 5.8 and 11. Lower density can flatter every result below.
- **Machine.** A shared Linux laptop. Work ran through its job queue, one job class with 4 CPUs and an
  8 GB cap. Most spikes ran single runs, and C1 ran 3 runs per configuration.
- **Browsers.** Headless Chromium and Firefox (Playwright), software raster, no CPU throttling unless
  stated.

## C1/C2: render core and one resolver

Pass 1 parses each file with mae-org (`parse`, `semantics`) and builds one site-wide `Resolver`. Pass 2
parses again, renders (`render`, `to_html`) against the shared resolver, and writes each page body.

Not included: sanitising, highlighting, diagrams, layout chrome, backlinks, redirects, search and
graph data.

| Files | 1 thread | 4 threads | Peak RSS |
|---|---|---|---|
| 48 | 0.010 s | 0.004 s | 4 MB |
| 1,536 | 0.38–0.40 s | 0.11 s | 7–9 MB |
| 5,040 | 1.19–1.35 s | 0.32–0.35 s | 13–15 MB |
| 10,032 | 2.36–2.97 s | 0.69–0.74 s | 21–24 MB |

**Resolver.** Built in up to 7 ms at 10,032 files. 0 duplicate IDs, 0 render diagnostics.

**Correctness on the 48-file sample.**
- 121 links resolved to pages, the same count the reference renderer reports for this sample.
- 0 raw `id:` hrefs.
- The link to an unpublished ID rendered as `<span class="org-unresolved">`, with no `<a>`.

**F1 (found here).** `[[id:X::#CUSTOM_ID]]` rendered to X's page **without** the section anchor.
mae-org's link grammar keeps `::#custom` as `LinkClass::search`, but the renderer's KB-link path reads
only the ADR-030 `#fragment`. ADR-119 D5 makes the fix its phase S0.

## C3: incremental correctness

Each output's fingerprint hashes what rendering it reads:
- the page's own source;
- the resolver entry, or its absence, for each ID it references;
- its backlink list.

A tag page's fingerprint hashes its member list. Pass-1 summaries are cached by source hash, and a
per-page resolver holds only the IDs the page references.

**Property test.** Start from the 48-file sample. Apply 300 random edits per seed, for 2 seeds: retitle,
body edit, add a description-less link, remove a link, delete a page, add a page with fresh IDs, move a
file to another folder, retag. After every edit, build incrementally, build clean into a fresh
directory, and compare the set of files and every file's bytes.

- **Seed 1 and seed 2:** equal after every one of the 300 steps.
- **Mutants:**
  - dropping backlinks from the fingerprint diverged at step 9 (a retitle);
  - dropping the referenced entries diverged at step 17 (a delete).
- **Re-renders per edit, median / max:**

| Edit | Seed 1 | Seed 2 |
|---|---|---|
| body | 1 / 1 | 1 / 1 |
| link | 2 / 2 | 2 / 2 |
| unlink | 2 / 2 | 2 / 2 |
| retag | 3 / 4 | 3 / 4 |
| delete | 4 / 11 | 4 / 13 |
| add | 3 / 12 | 4 / 16 |
| retitle | 5 / 19 | 5 / 18 |
| move | 0 / 0 | 0 / 0 |

**Limits.**
- The outputs were pages, backlinks and tag pages only. There were no folder listings or explorer, so
  "move: 0" is vacuous.
- The edit set does not change heading IDs, `CUSTOM_ID`s, configuration or templates, and does not
  create duplicate IDs.
- The test proves consistency between incremental and clean builds, not that a clean build is correct.

## C4: client-side search

Corpora:
- the synthetic bundles above, whose text is 98 % repeated and flatters index size;
- real-prose corpora of up to 5k pages, used for sizing.

Link profiles: localhost; 10 Mbps / 40 ms RTT; 1.6 Mbps / 150 ms RTT.

| Target | Pagefind 1.6.0 (MIT) | Own chunked index (prototype) | Whole-index libraries (MiniSearch, FlexSearch) |
|---|---|---|---|
| Largest single download ≤ 300 KB compressed | 59 KB on a cold load | shards 11–38 KB | fails from about 500 real pages (1.2 MB at 1.5k, 4.4 MB at 5k) |
| Cold first result ≤ 300 ms | 326–329 ms at 10 Mbps; 1.8–1.9 s at 1.6 Mbps | 295–363 ms at 10 Mbps; 1.15–1.33 s at 1.6 Mbps | 1.6–2.0 s at 10 Mbps |
| Warm ≤ 16 ms | 3.6–5.8 ms; 17–19 ms at 5k real | 0.3–6.7 ms with cached shards | 0.2–2.3 ms |
| No rebuild on navigation | init 7–16 ms | init 9–21 ms | re-parses every load; 8 s at 5k real |
| Build peak RSS | 223 MB (1.5k synthetic) to 603 MB (10k synthetic) to 1.57 GB (5k real) | 9–94 MB | — |

Notes:
- Spanish stemming: Pagefind 12/12 probes.
- Tag filters returned the expected counts.
- The own index needs a browser Snowball stemmer that matches the Rust one: 0 mismatches over 56,812
  word forms. It also has a stop-term bug: a query made only of stop words returns nothing.
- Pagefind's RSS and build times came from its command-line build. As a library, `add_custom_record`
  was shown to work without HTML, but its time and memory were not measured.
- A first result needs two serial round trips, so no design here reaches 300 ms on the slow profile.

## C5: graph at 1.5k, 5k and 10k nodes

The chord-graph core was driven directly by our own harness. The harness omits labels, the "here"
marker, the visited band and themes, so it understates the real cost. Graphs were tested sparse
(synthetic links) and dense (11 links per node).

**What does not scale is the drawing, not the layout.**
- The ring layout is a sort plus an edge dedupe: 40 ms at 1.5k and 270 ms at 10k dense, in Node.
- The cost is one SVG element per chord: 122k DOM nodes at 10k dense, about 0.9–1.1 s to a first
  stable frame, and 160–550 ms per hover.
- Precomputing the layout at build time saves only 5–30 % of the first paint.

**What passes every target at every size, desktop, unthrottled:**
- the chord layer baked at build time into one WebP (11–34 KB, decoded in up to 21 ms);
- a canvas overlay for the current, hovered and neighbouring notes, from a compact edge list (94 KB
  compressed at 10k dense);
- hover about 17 ms and navigation about 33 ms. These are frame floors from the reduced harness.

**Open:**
- A baked colour image cannot follow palette or accent changes. One image per theme was not measured.
- A bitmap blurs when zoomed.
- The hierarchical views lay out in the browser in 100–460 ms, and their scene data is too large to
  precompute as is (572 KB compressed at 1.5k sparse; about 19 MB at 10k dense).
- No CPU-throttled (phone-like) runs.

## C5b: the graph's open questions (2026-11-27)

The candidate measured is D15's:
- **The bulk layer.** Every chord is drawn once into an alpha layer, then tinted to the palette's colour with one composite.
- **Notes.** Each note is an SVG element, so it can be focused and named (P-59, P-60).
- **The live layer.** The current note's chords, a hovered note's chords, labels, the visited band and the "here" marker are drawn on an overlay canvas.

**Method**
- **Graphs.** Synthetic graphs at the real bundle's density: 11 links per note, 70% of them between nearby notes. Sizes 1,560 (the test deployment's KB), 5,000 and 10,000 notes (8,580, 27,500 and 55,000 chords).
- **Browsers.** Playwright with Chromium 158 and Firefox 159, headless, software raster, on the same shared laptop as C1–C6.
- **Jobs.** Each run was a job in the machine's queue: 2 CPUs and a memory cap. Single runs.
- **Timing.** Each canvas step is timed to a forced rasterisation (a 1-pixel readback); without it, Chromium's deferred rasterisation makes a bake look like 7 ms.
- **Throttling.** CPU throttling (4×, 6×) is Chromium's CDP emulation. It slows only the main thread, so worker numbers under it are not phone numbers.
- **GPU.** A GPU-raster run was attempted (`--use-angle=gl`, the integrated GPU). It ran out of its 2 GB cap, and the readback above penalises a GPU canvas, so GPU raster is not measured. Real-device runs are still owed.
- **The code.** As before, the harness is not committed.

**The bulk layer is the whole cost, and its resolution decides it.** In both browsers, unthrottled:

| Chords | Bulk at device pixels (2×): bake | Zoom 2× | Bulk at 1×: bake (main / worker) | Zoom 2× |
|---|---|---|---|---|
| 8,580 | 400–450 ms | 300–500 ms | 40 / 35–55 ms | 17–22 ms |
| 55,000 | 4.2–4.5 s | 2–2.8 s | 123–125 / 185–189 ms | 114–193 ms |

Drawing the bulk at one pixel per CSS pixel costs 10–30× less than at device resolution, not 4×: the strokes become sub-pixel. On a 2× screen the 1× bulk is visibly softer but reads as the same texture. Screenshots were compared side by side. The current and hovered chords, labels and markers stay at device resolution on the overlay.

**Everything else passes at every size.** Unthrottled, in both browsers:
- A palette or accent change re-tints the baked layer in 4–18 ms, with no chord redrawn (P-68, P-95).
- A hover redraws only the overlay: p95 3–17 ms. P-57a holds by construction, because the bulk layer is never touched.
- The visited band and "here" marker take 1–6 ms.
- 10,000 note elements take 59–94 ms. At 6× throttling that rises to 170–380 ms, over the 100 ms long-task budget per navigation, so the side-panel graph must keep its note elements across navigations rather than rebuild them.

**What did not work**
- **Baking on the server.** tiny-skia (the editor's rasteriser, not yet a daemon dependency) took 1.2–7 s at 1600 px, and an 8-bit alpha PNG of the layer is 0.6–1.2 MB. That is over the 300 KB download budget, because a dense chord texture does not compress. A colour WebP was 11–34 KB in C5, but it cannot be tinted, and the daemon has no lossy encoder.
- **Drawing visible chords live when zoomed.** At device resolution it costs 0.3–2.8 s at 2×, and is affordable only from about 8× at 1,560 notes. Zoom instead scales the 1× bulk layer and redraws the overlay sharp. The camera moves; nothing else does (P-64).
- **Binning chords by ring position.** Merging chords that join the same pair of ring bins cuts the strokes only 1.0–2.1× (720–2,880 bins). The links are too spread out.

**The hierarchical views do not fit any budget, at any size, as built.** The graph package's `buildScene`, in Node, desktop CPU:

| Notes | Sunburst or balloon build | Copying the scene to or from a worker | Peak memory |
|---|---|---|---|
| 1,560 | 300–340 ms | 220–350 ms | 0.35 GB |
| 5,000 | 1.1–1.3 s | 0.9–1.0 s | 1.1 GB |
| 10,000 | 2.1–2.2 s | 2.1–2.9 s | 1.9 GB |

An earlier run at 5,000 notes ran out of memory under a 1 GB cap.
- On the main thread the build is a long task over the 300 ms target even at 1,560 notes.
- Moving it to a worker costs as much again in copying.
- Its memory rules out phones.

The scene's size, not the layout arithmetic, is the problem. Making these views fit needs a different scene: positions per note (O(notes)), derived on the server and cached with D6, with bundled links drawn as a tinted bulk layer like the chord view's. That is a redesign, not a tuning pass.

**What the parity suite needs.** P-55, P-57, P-57a and P-64 are written as DOM checks on chord elements, such as "highlighted link elements come after all others". A chord drawn on a canvas has no element. With this design, those checks have to assert on the graph's model (which notes and links are lit, and in what role), exposed to tests, plus sampled pixels. The behaviour they describe is unchanged.

**Still open**
- Runs on a real phone and a real laptop browser with GPU raster.
- The real bundle's link structure. Every result here is synthetic, and a more clustered real graph would bin better.

## C6: in-page router

A dependency-free router of about 190 lines: 5,488 B minified, 2,388 B gzip.

**Tests.** 17 tests per browser, Chromium and Firefox, 34/34 pass, and the same on 2 consecutive
re-runs. They cover P-22, P-22a, P-23, P-24, P-25, P-26, P-27, P-28 (the marking half; previews are
not built), P-37, P-62 (Back closes the view) and P-130. The test pages included:
- a sectioned long page with folded sections;
- an inline SVG diagram with `href` and `xlink:href` links;
- a modal dialog holding an SVG link;
- a widget torn down and re-initialised once per navigation;
- per-page backlinks;
- a component counter that must run exactly once per navigation.

**Timing.** Click to commit, median / p95, loopback: Chromium 14.6 / 21.6 ms; Firefox 9 / 16 ms. A full
reload of the same pages took 21 ms in Chromium and 39 ms in Firefox.

**Mutants.** 18 of 19 deliberately broken routers were caught in both browsers. The survivor removes
the `hashchange` handler, which is equivalent because `popstate` already covers it. In Firefox, reading
SVG anchors as `a.href` was caught only by P-25's test, not P-26's.

**Router bugs the tests found:** Back/Forward did not open folded sections; per-page panels lost their
resize handle after a swap. Both were fixed.

## C10: diagrams

Inputs: the 5 sample DOT blocks, plus synthetic graphs of 50, 200 and 500 nodes, one of them neato with
`start=random`. The baseline renderer runs Graphviz WASM in one child process per diagram: about
340 ms each.

| Option | 49 diagrams | Notes |
|---|---|---|
| Graphviz compiled to WASI, under wasmtime | 34 ms (fresh instance each), 23 ms (reused) | One-time compile 923 ms. Real blocks 0.4–1.4 ms each; 500 nodes 121 ms. Fuel and epoch deadlines bound time (a 4,000-node graph trapped at 231 ms). `dot` output is byte-stable. neato `start=random` varies on a reused instance (the guest C library's random state), so use a fresh instance or `start=N`. Output byte-identical to the baseline on 4 of 5 blocks apart from a version comment. The 5th lost its HTML-like labels: the only prebuilt module (a third-party package, EPL-2.0, unverified provenance) lacks expat. Memory cap not measured. |
| System `dot` | 1,058 ms (spawn per diagram), 113 ms (one long-lived process), 56 ms (one batch call) | HTML labels work. `start=random7` is stable. Output depends on host fonts. Killing it bounds time but loses the pending stream. |
| layout-rs (pure Rust) | rejected | Drops `URL` links, fails to parse one sample, 13.7 s at 500 nodes. |

Graphviz is EPL-1.0, which the FSF lists as GPL-incompatible. ADR-119 D13 makes a licence opinion a
precondition.

## C11: site components

The client's four pure components (a state badge, a "what's new" link, a version footer, a
last-updated line) were rewritten for each mechanism.

| | minijinja 2.24 (Apache-2.0) | MAE's embedded Scheme |
|---|---|---|
| Lines (4 components) | 24 | 55 |
| 1,536 pages × 4 components, with sanitising | 86 ms | 191 ms |
| Safety | arbitrary include/extends fail; recursion limit, range cap and strict undefined fire; fuel not measured | file read, environment read and `exit` work; `load` is an ungated opcode; no step limit; no public API to call a function with data |

**Sanitiser:** ammonia 4.2 (MIT OR Apache-2.0). It strips `<script>`, `on*` attributes and
`javascript:` URLs. It must be configured to keep `class`, `role` and `time[datetime]`.

## C12: code highlighting

- **Grammars:** tree-sitter-highlight with the editor's grammars (bash, hcl, json, python, toml, yaml),
  plus tree-sitter-powershell 0.26.6 (MIT), tree-sitter-ini 1.4.0 (Apache-2.0), tree-sitter-sequel
  0.3.13 (MIT) and tree-sitter-nginx 1.0.1 (MIT). All build on tree-sitter 0.26 and ship
  `highlights.scm`.
- **Queries:** nginx's query must be vendored. HCL has no upstream query, so the editor's is reused.
- **Byte-exact:** 28/28 blocks in supported languages, with no fallback. That includes tabs, trailing
  spaces, CRLF, emoji, markup-like text, lines over 2,000 characters, empty blocks and syntax errors.
  The check caught 7/7 deliberately corrupted outputs.
- **Speed:** 430 shell blocks in 23 ms; 430 mixed-language blocks in 112 ms; warm 10–125 µs per block.
  A language's first use takes 0.1–10 ms, except SQL at 61 ms.
- **Size:** 0.32 MB with no grammars, 4.4 MB with the editor's six, 18.9 MB with all ten.
- **syntect's default set** covers 5 of the 10 languages.
