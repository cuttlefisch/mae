# Research for ADR-112: should MAE adopt, fork or build its org parser? (2026-10-30)

Research for the "build MAE's own org parser" decision (ADR-112, phase O0). The brief was to **try to refute** "build our own": find the strongest case for adopting or
forking an existing parser, then report honestly. The project owner added mid-research: **upstream
maintenance health and code quality are a GATE, not one criterion among many.** A candidate that fails
the gate is not recommended for adoption or forking, whatever its features.

## Verdict in one paragraph

**Build MAE's own, and borrow designs (not code) from two of the candidates.** Every existing Rust
parser fails the health gate. The two that fit the requirements technically also fail it, for opposite
reasons:
- **orgize 0.10-alpha** is a mature lossless design whose upstream has **stalled**. No commit on the
  default branch in 27 months, and 13 consecutive issues and PRs since 2024-07 have had no maintainer
  reply.
- **orgcst 0.1.1** is the closest technical match found: lossless rowan CST, diagnostics, `#+TODO`,
  wasm, parity harness and fuzzing. It is **one week old**, has one author, 8 commits and no track
  record.

Both also failed hands-on checks MAE cares about:
- orgize: a stack-overflow abort, a debug-build panic, and super-cubic time on a 24 KB line (15.6 s).
- orgcst: a column-0 heading is swallowed by an open block, against Emacs and the spec.

The one parser that matches Emacs almost exactly (organic) is nightly-only, does file I/O while
parsing, and overflows the stack on a 6 KB line.

## Method

- **Scratch workspace.** One harness crate per candidate, a shared target directory, and driver
  scripts, outside the repository.
- **Toolchain.** Fedora rustc/cargo 1.98.0, wasm32-unknown-unknown std installed, Node for the wasm
  runs, Emacs 30.2 with Org 9.7.11.
- **Corpora:**
  - **mae**: 340 `.org` files under `assets/` and `tests/` of the MAE repo, read-only. All LF, all
    UTF-8.
  - **crlf**: the same 340 files converted to CRLF, as a lossless stress test.
  - **ext**: 338 `.org` files from shallow clones of Org's own repo (org-mode, 2026-10-30) and Worg
    (2026-10-21). They include 3 CRLF files and large files such as `org-manual.org` (~890 KB).
  - **syn**: 31 adversarial files written for this evaluation (`scripts/mk_synthetic.py` plus a
    deep-nesting addendum):
    - heading and structure edge cases: indented `* x`, `*\tx`, a BOM, CRLF and mixed EOL, an empty
      file, a lone `*`, no trailing newline;
    - blocks: comment, verse, custom, quote, center, example, export, indented src, an unterminated
      src, empty greater blocks, a heading inside a quote block or an example block;
    - drawers, lists and footnotes: a list-item `:ID:` drawer, an unclosed drawer, deep lists (60
      levels, and 5000 levels in a 12.5 MB file), a footnote definition;
    - links and keywords: ADR-030 typed links, `[[a|b]]`, a `:noexport:` subtree, `#+TODO: A B | C`,
      MAE file keywords plus `#+TRANSCLUDE`, affiliated keywords, planning and clock, a COMMENT
      heading, unicode;
    - adversarial size: an 868 KB file of very long lines, 2000 nested quote blocks, 5000-deep
      emphasis, 50000 `[[`, 20000 headings.
  - **probes**: 21 size-scaling inputs that double in size, used to find the complexity class: `*a `×N,
    `[[`×N, nested emphasis, nested lists, nested quotes, and one 678 KB line of words, markup and
    links.
- **Round trip.** Each file runs as its own subprocess with a 60 s timeout, because a stack overflow
  is an abort, not a catchable panic. The harness checks parse→print == input byte for byte and records
  the first divergence.
  - For parsers with no printer, it checks instead that the root span covers the input exactly: tree
    text for rowan, source slices for organic, root `end` for the others.
  - Release and debug builds both run, because debug builds fire `debug_assert!`.
- **Spans.** An Emacs oracle (`scripts/oracle.el`) runs `org-element-parse-buffer 'object` per file.
  - It emits `(type, begin, end)` as **byte** offsets: `position-bytes`, plus a newline correction for
    files decoded as DOS, plus 3 for a BOM.
  - It covers 23 element types plus links. `scripts/cmp.py` compares each candidate per kind on begin,
    exact span, span with trailing blank lines trimmed, and end only.
  - **Exclusions.** 6 ext files where the oracle's own headline offsets fail a sanity check
    (`src[begin] == '*'`) are excluded. This is an encoding-detection issue in the oracle, not in the
    candidates.
  - **Falsifiability check on the harness:**
    - the round-trip check does fail: orgize 0.9 passes 160 of 340;
    - the comparator does discriminate: organic matches Emacs on 1024 of 1029 files, while the
      tree-sitter crate matches on 1;
    - the oracle mapping is right, because organic reproduces Emacs exactly, CRLF variants included.
- **wasm.** Each candidate builds as a minimal `cdylib` exporting alloc plus parse→serialize.
  - Build settings: no wasm-bindgen, `opt-level="s"`, LTO, `panic=abort`, strip. No wasm-opt.
  - The module is loaded synchronously in Node (`new WebAssembly.Module`) and round-trips all 1046
    non-pathological files.
- **Performance.** Parse+serialize over mae+ext (676 files, 8.05 MB), in process, 5 iterations, one
  Linux laptop.
- **Health.** crates.io API, the GitHub API (`gh`), and the Gitea API for organic. Commits are counted
  per 12-month window ending 2026-10-30. Issue and PR response time is measured to the first comment by
  someone other than the author, or to the close.

## Candidates

| Candidate | What it is | Lic. | Evaluated |
|---|---|---|---|
| **orgize 0.10.0-alpha.10** | rowan lossless CST, nom combinators, 14k lines | MIT | fully |
| orgize 0.9.0 | last stable release; typed AST plus a normalising writer | MIT | round trip only |
| **orgcst 0.1.1** | rowan lossless CST, hand-written recursive descent, typed views, structural edits, diagnostics; 77k lines | MIT/Apache-2.0 | fully |
| organic 0.1.16 | typed AST of `&str` slices, built to mirror org-element; 30k lines | 0BSD | fully, with `RUSTC_BOOTSTRAP=1` and a patched manifest |
| org-rust-parser 0.1.8 | node pool with start/end, plus an HTML exporter; 10k lines | MIT | fully |
| org-element 0.1.0 (crate) | wrapper over a bundled tree-sitter-org grammar, tree-sitter 0.26; 6k lines | MIT/Apache-2.0 | fully except wasm |
| starsector 1.0.1 | headline-only structural parser and emitter on a rope | MIT | round trip and headline count |
| tree-sitter-org (crate 1.3.3) | milisims grammar, **archived** 2024; requires tree-sitter `>=0.19,<0.21` | MIT | metadata only: incompatible with MAE's tree-sitter 0.26 |
| nvim-orgmode/tree-sitter-org | the maintained grammar fork (2.0.4, 2026-05); its Rust binding still declares tree-sitter `<0.21` and it is not on crates.io | MIT | metadata only |
| uniorg (JS), pandoc org reader | reference points only | GPL-3 / GPL-2+ | not hands-on |
| others seen, not evaluated | pullorg (published yesterday), tftio-org, oak-org-mode, windancer (GPL-3.0), mdroots-syntax, org-spec 0.0.1 | | none claims losslessness plus spans, or they are under a month old with no users |

## Criterion tables (measured)

### 1. Round trip: files where parse→print equals the input byte for byte

| | mae (340) | crlf (340) | ext (338) | syn (31) | probes (21) | First divergence / failure |
|---|---|---|---|---|---|---|
| orgize 0.10a10 | **340** | **340** | **338** | 28 | 21 | syn: s27 aborts (stack overflow, 5000-deep list); s12 and s29 >60 s. Debug build also panics on s15 (`debug_assert!(!input.is_empty())`, empty greater block; upstream issue #95) |
| orgize 0.9 | 160 | **0** | 24 | 15 | n/a | normalising writer: `#+begin_example` is written as `#+BEGIN_EXAMPLE`; every CRLF file fails |
| orgcst 0.1.1 | **340** | **340** | **338** | **31** | **21** | none, in release or debug (debug only exceeds the 60 s limit on s12) |
| organic¹ | 340 | 340 | 330 | 28 | 16 | ext: 8 files **fail to parse** because it opens `#+INCLUDE`/`#+SETUPFILE` targets on disk. syn/probes: 7 stack-overflow aborts, starting at `*a `×2000 (6 KB), and 1 timeout |
| org-rust-parser¹ | 340 | 340 | 336 | 28 | 21 | **panics** on 2 real Worg files (`comment.rs:20` slice index); stack overflow on s27; root span runs **past EOF** for files without a trailing newline |
| org-element (ts)¹ | 340 | 340 | 338 | 31 | 21 | no crash. 22.4 s for one 678 KB line: its `add_child` deep-clones the node on every insert |
| starsector | 340 | 340 | 338 | 31 | 21 | none, but it parses headlines only |

¹ No printer. The figure is root coverage, not proven losslessness.

### 2. Spans: agreement with Emacs org-element (1037 files; Emacs counts → candidate matches)

Every candidate exposes byte offsets. For orgize and orgcst each token's text **is** the source slice,
so spans agree with the source by construction. The question is whether they agree with **Org**.
"end=" compares the trimmed end only. It is the fairest recognition measure, because the begin offset
depends on a representation choice: whether affiliated keywords such as `#+name:` sit inside the
element's span, as they do in Emacs and organic, or before it, as in orgize and orgcst.

| kind (Emacs count) | orgize 0.10 end= | orgcst end= | organic¹ end= | org-rust end= | org-element (ts) end= |
|---|---|---|---|---|---|
| headline (7243) | 7240 | 7234 | 6469/6472 | 7212 | 143 (it spans the line only) |
| item (11882) | 11846 | **11882** | 10043/10043 | 11132 | 11846 |
| plain-list (2869) | 2854 | **2869** | all | 2544 | 2855 |
| footnote-definition (189) | **70** | **189** | all | 187 | 0 |
| link (7962) | **7304** (no plain or angle links) | 7944 | all | 7813 | 6621 |
| src-block (2078) | 2077 | 2078 | all | 1760 | 1975 |
| table (466) | 439 | 464 | 386/388 | n/a | 416 |
| dynamic-block (4) | **0** | 3 | all | 0 | 0 |
| keyword (7398) | 7380 | 7397 | 4352/4353 | 7362 | **0** |
| property-drawer (1883) | 1883 | 1883 | all | **0** (not a node) | 1198 |
| paragraph (26410) | 26304 (31474 emitted) | 26384 | 21981/21982 | 26160 | 25610 |
| **files whose structural nodes all agree on begin and trimmed end** | **71** | **872** | **1024 / 1029** | 2 | 1 |

¹ organic is compared over 1029 files, because 8 failed to parse. Its denominators are therefore
smaller.

Remaining orgcst differences, by cause:
- representation: affiliated keywords sit before the element, and the section starts after
  planning/properties;
- configuration: 15+ stars are an inline task;
- **one real conformance bug**, covered in table 3.

### 3. Spec conformance on the synthetic set (uppercase = same begin offsets as Emacs)

H = headlines, B = blocks and drawers, I = lists and items, L = links.

| file | orgize 0.10 | orgcst | organic | org-rust | org-element (ts) |
|---|---|---|---|---|---|
| s01 indented `* x`, `\t* x`, `*x`, `*\tx` | HBIL | HBIL | **h**BIL (`*\t` taken as a heading) | HBiL | hBiL |
| s04 `* line` inside `#+begin_example` | HBIL | **hb**IL | HBIL | hbIL | hbIL |
| s21 `* line` inside `#+begin_quote` | HBIL | **hb**IL | HBIL | HbIL | hbIL |
| s06 typed / store / plain / angle links | HBI**l** (no plain or angle links) | HBIL | HBIL | HBIL | HBI**l** |
| s15 empty greater blocks | HBIL (but panics in debug) | HBIL | Hb**IL** | HBIL | HBIL |
| s07 unterminated src block, heading after it | HBIL | HBIL (+ `UnterminatedBlock` diagnostic) | HBIL | HBIL | **h**BIL |
| other 19 files | all HBIL | all HBIL | all HBIL | various b/i | various |

Column-0 rule. Emacs treats a column-0 `* x` inside any block as a heading: the block is unterminated
and becomes paragraph text. **orgcst instead lets the open block swallow the heading.** A 4-line probe
reproduces it: `* A`, `#+begin_src sh`, `* B`, `#+end_src` yields one outline in orgcst and two
headlines in orgize and Emacs. This is exactly the column-0 rule MAE's requirement (e) names, and it
moves KB node boundaries.

Per-file `#+TODO`:
- orgcst: native, including `SETTING_KEYWORD`.
- organic: native.
- orgize: not honoured. `ParseConfig.todo_keywords` is caller-set, so a pre-scan plus a reparse is
  needed. Emulated here, it then works.
- org-rust: no.

Diagnostics: only orgcst reports them, for example `UnterminatedBlock`, `UnterminatedDrawer`,
`UnterminatedLink` and `NestingLimitExceeded`.

Totality, and adversarial scaling of parse time:

| input | orgize 0.10 | orgcst | organic | org-element (ts) |
|---|---|---|---|---|
| `*a `×1000 / 2000 / 4000 / 8000 (single line) | 54 ms / 0.32 s / 2.1 s / **15.6 s** (~n^2.8) | 2.7 / 8.7 / 31 / 95 ms (~n^1.8) | **timeout / stack overflow from 2000** | ~linear, 25 ms |
| `[[`×8000 | 156 ms (~n^1.7) | 123 ms (~n^1.8) | 49 ms | 15 ms |
| 2000 nested quote blocks | 209 ms (~n^2) | 8 ms | 102 ms | 74 ms |
| 5000-deep list | **abort (stack overflow)** | 134 ms, depth capped at 128 with a diagnostic | abort | 10 s |
| 868 KB long-line file (s12) | **>60 s** | 3.7 s | abort | 24.7 s |

None of these parsers is linear-time on hostile inline input. MAE re-ingests attached KBs whenever a
file changes, so this is a denial-of-service property, not a micro-benchmark. Whatever MAE builds needs
a scaling test like this one as a gate.

### 4. Extensibility for MAE's extensions

Every requirement maps onto spans in the two rowan candidates, so a post-pass over spans covers them.
None needs a fork:

| extension | orgize 0.10 | orgcst | how |
|---|---|---|---|
| list-item `:ID:` drawer | a `DRAWER` inside `LIST_ITEM` (its content is a paragraph) | a `DRAWER` inside `ITEM`, with `DRAWER_NAME` | post-pass: read `:KEY: value` lines inside the drawer span. This matches Emacs, which also sees a plain drawer there |
| `[[id:x?rel=..&w=..][d]]` | `LINK`, path `id:x?rel=extends&w=0.8` | `LINK` / `LINK_PATH` | post-pass that splits the path query |
| `[[T\|D]]` store form | link with path `a\|b` | link with path `a\|b` | post-pass |
| `#+TRANSCLUDE:`, `#+kind`, `#+aliases`, … | `KEYWORD` | `KEYWORD` / `SETTING_KEYWORD` | post-pass |
| `:KIND:` / `:ALIASES:` properties | `NODE_PROPERTY` | `NODE_PROPERTY_KEY` / `_VALUE` | native |
| per-file `#+TODO` | **no**: needs a pre-scan plus a second parse | native | |
| diagnostics (M10) | **none**: needs a fork or a parallel validator | native | |
| fragments (M4, `parse_fragment` at a heading level) | no API: needs a fork | no API: needs a fork or wrapping | |

So extensions do not force a fork. **Fixing upstream defects does force one**, and an unresponsive
upstream makes that fork permanent. For orgize that means the overflow, the scaling, plain links,
affiliated keywords, footnote extents, dynamic blocks, `#+TODO`, diagnostics and fragments. For orgcst
it means the column-0 bug, fragments, and its MSRV.

### 5. wasm32-unknown-unknown (minimal parse+serialize cdylib; Node sync load verified)

| | builds | .wasm | gzip -9 | Node round trip (1046 files) |
|---|---|---|---|---|
| orgize 0.10 | yes | 191 KB | 67 KB | 1046/1046, 533 ms |
| orgcst | yes (with `--ignore-rust-version`: it declares MSRV **1.99**, MAE's is 1.95) | 234 KB | 88 KB | 1046/1046, 796 ms |
| org-rust-parser | yes | 1067 KB (regex, phf) | n/a | not run |
| org-element (ts) | **no**: tree-sitter 0.26 + tree-sitter-language 0.1.9 refuse `wasm32-unknown-unknown`, and a C-to-wasm clang is required | | | |
| organic | not tried | | | |

These are reference sizes for a rowan-based crate. wasm-bindgen glue and wasm-opt are not included.

### 6. Performance (676 files, 8.05 MB, parse+serialize, per pass)

| orgize 0.9 | orgize 0.10 | starsector | org-rust | orgcst | org-element (ts) | organic |
|---|---|---|---|---|---|---|
| 82 ms | **147 ms (55 MB/s)** | 150 ms | 294 ms | **342 ms (24 MB/s)** | 3.0 s | 5.3 s |

Notes:
- org-rust-parser printed 185 `dbg!` lines to stderr during this run: `dbg!` is left in library code
  (24 sites).
- MAE's current `org.rs` was not measured.

### 7. Health and code quality: THE GATE

Pass criteria applied:
- at least 2 people committed or merged in the last 12 months, or one maintainer with a demonstrated
  responsive record over years;
- a release in the last 12 months, and not stuck on a long-lived alpha;
- issues answered;
- tests that check round trip or conformance, plus fuzzing;
- no panic or abort on malformed input in our runs;
- builds on stable Rust within MAE's MSRV and toolchain.

| | commits/yr (to 2024-10 / 2025-10 / 2026-10) | active committers, last 12 mo | last release | issues & PRs | tests / fuzz | unsafe, panics | deps | **gate** |
|---|---|---|---|---|---|---|---|---|
| **orgize** | 113 / **0** / **0** (last commit 2024-07-22) | **0** (all-time: 344 of 353 commits by one author) | 0.10.0-alpha.10, 2024-06. **No stable release since 0.9 (2021)**; 10 alphas over 7 months, then stopped | 77 items. Median first response 4.3 d historically, but **13 of 13 items since 2024-07 have no maintainer reply**, including 5 PRs. Oldest open: a fuzz PR from 2020 (6.5 y); empty-section panic bug #83 open 26 months | 51 `#[test]`, mostly snapshots; no fuzz target in tree (a user started fuzzing it externally in 2026-10 and found the debug panic) | 1 unsafe line; abort at a 5000-deep list; debug panic; super-cubic inline | 13 crates (nom 7, rowan 0.15, which upstream has moved past) | **FAIL**: stalled, unresponsive, single maintainer |
| **orgcst** | 0 / 0 / 8 (whole history: an initial 76k-line commit on 2026-10-23, then 7 more) | 1 | 0.1.1, 2026-10-26 | none filed yet: responsiveness is unmeasurable | 1688 `#[test]`; fuzz targets parse/roundtrip/apply; an org-element parity harness (pinned Org 10.0-pre in Docker, with an exceptions register) | 2 unsafe lines (a rowan transmute, allowed locally); no panic in our runs; nesting cap | 5 crates (rowan only) | **FAIL**: no track record (1 week, 1 author, 20 downloads), MSRV above MAE's, unknown provenance. Its quality signals are the best seen, but they are claims we cannot yet weigh against history |
| organic | 259 / 4 / 20 (2025–26 commits are CI only) | 1 | 0.1.16, 2024-04 | Gitea: 0 open | 53 tests plus an Emacs compare tool | **nightly-only** (`#![feature]` and a `cargo-features` manifest); does **file I/O while parsing**; stack overflow on a 6 KB line | 32 | **FAIL** |
| org-rust-parser | 40 / 17 / 7 | 1 (+1 drive-by) | 0.1.8, 2026-05 | 13 items, median first response 0 d | 164 tests, no fuzzing | **panics on 2 real Worg files**; `dbg!` in library code; spans past EOF | 26 | **FAIL**: correctness |
| org-element (ts crate) | 0 / 0 / 11 | 1 | 0.1.0, 2026-07 | none | 89 tests | quadratic deep-clone on every insert; parent pointers always dangle (`Rc::downgrade` of a temporary); no wasm | 19 + C | **FAIL** |
| starsector | 1 / 0 / 1 | 1 | 1.0.1, 2022 | 2 items (median 260 d) | 59 tests + fuzz vs orgize | does **not compile** with current semver-compatible `indextree` (needs a pin) or with `--no-default-features` | 51 | **FAIL** (scope too: headlines only) |
| tree-sitter-org | archived 2024 / fork: 5 / 5 / 0 on main, releases from a branch | fork: 1 | crate 1.3.3 (2022) | n/a | grammar corpus tests | C scanner; Rust binding pins tree-sitter <0.21 | n/a | **FAIL** for Rust/wasm use |
| *rowan (substrate, not a parser)* | active (0.17 2026-08, 0.18-alpha 2026-10) | ≥2 | 2026-10 | — | — | — | small | **PASSES**. Caveat: API churn, 0.15 → 0.18 in a year |

## Strongest argument FOR adopting or forking

orgize 0.10-alpha and orgcst already demonstrate the hardest property MAE needs, a lossless typed tree
with byte spans and wasm. They do it at production-grade round-trip fidelity: **1018/1018 real files
and their CRLF variants byte-identical**, 1046/1046 through wasm in Node. orgcst additionally has
diagnostics, `#+TODO`, a depth cap, fuzzing and an org-element parity harness. On structural
recognition it matches Emacs on 872/1037 files, and nearly all its remaining differences are
representation.

Every MAE extension maps onto their spans with a post-pass. No grammar change is needed. MAE's own
O1–O3 is weeks of work that these crates have already done, and both licences are GPL-compatible.

## Strongest argument AGAINST

**MAE writes through this tree.** Body edits are span splices, subtrees move to archive files, and
attached KBs must keep every other byte. The parser will also run in a daemon that re-ingests on every
change. Each of these turns a parser defect into data loss or a denial of service, and MAE carries an
extended syntax on top. So whatever MAE adopts it effectively owns, and the question becomes whose
codebase it would rather own.

- **orgize**: an upstream silent for 27 months, with known open correctness bugs. We measured a
  stack-overflow abort, a debug panic, 15.6 s on a 24 KB line and missing plain links. Its nom
  combinator style is not one MAE's code uses.
- **orgcst**: 77k lines, larger than all of MAE's org code, from a one-week-old single author. It
  already breaks the column-0 heading rule in exactly the case MAE's requirement (e) names.

A fork of either is permanent from day one, and adding MAE's extensions only widens the gap from
upstream. Neither offers the fragment API (M4) that the body contract needs.

## Recommendation

**Build own, borrowing designs but taking no candidate's code.** The health gate decides it. Take one
dependency: **rowan**, the healthy substrate both lossless parsers use. Alternatively write an
equivalent span tree; that choice belongs to the ADR.

Designs to borrow, with their sources:
1. **Lossless green/red tree with typed views on top** (orgize, orgcst). The invariant "every byte in
   exactly one token" gets a debug assertion per production, as orgize's `lossless_parser!` does.
2. From **orgcst**:
   - **Parse-time config only for settings that change the tree's shape** (TODO sequences, link types,
     the depth limit). Everything else is read from the tree.
   - **Event grammar → builder.**
   - **A nesting cap that degrades to a diagnostic**, never to recursion.
   - **Fuzz targets parse / roundtrip / apply.**
   - **A pinned-Org parity harness with an exceptions file.** This is MAE's D5 divergence register,
     already designed.
3. From **organic**: give element extents in org-element's representation (affiliated keywords and
   post-blank inside the element). The Emacs differential then needs no mapping layer. organic hit
   1024/1029 because of this.
4. From **starsector**: an edit model that preserves unmodified subtrees byte for byte and **rejects**
   a splice that would change structure unintentionally. This is the right shape for body ops and
   archive moves.
5. **Gates** taken from this evaluation:
   - wasm size. Reference points: 191–234 KB raw, 67–88 KB gzipped.
   - **A scaling test on hostile inline input** (`*a `×N, `[[`×N, nesting), requiring roughly linear
     growth. Every candidate failed this.
   - Totality under fuzzing.
   - Column-0 headings inside blocks, CRLF/BOM round trip, and the Emacs oracle diff.

   The evaluation harness (an Emacs oracle, a comparator, a driver and the probes) is a working seed
   for the O1 differential harness. Note: the oracle's byte mapping fails on 6 Worg
   files and needs fixing before reuse.

**Cost comparison (order of magnitude, not an estimate):**
- *Adopt orgize*: an immediate fork of a 14k-line nom codebase, plus at least 9 known defect fixes and
  2 missing APIs (diagnostics, fragments).
- *Adopt orgcst*: taking ownership of 77k lines with no history, an MSRV bump from 1.95 to 1.99, and a
  conformance fix in its core outline/block interaction.
- *Build own*: MAE-scoped O1–O3 on rowan. For scale, full-syntax recognisers here run from 14k lines
  (orgize) to 77k (orgcst), and MAE's M13 scoping (recognise everything, render by measured use) keeps
  it toward the low end.

**Re-evaluation trigger, so the decision stays falsifiable.** Revisit orgcst before `mae-org-v0.1.0`
is frozen, and again around 2027-Q2, if all of these hold:
- ≥2 active committers;
- ≥6 months of releases;
- demonstrated issue response;
- the column-0 heading-in-block bug fixed;
- MSRV ≤ MAE's.

If they do, building `mae-org` as a thin MAE layer (extensions, semantics, renderers, wasm facade) on
top of orgcst becomes the cheaper path.

## What could NOT be verified

- orgcst's own fuzzing and parity claims. Its fuzz and parity harnesses need nightly and Docker and
  were not run. Its provenance is unknown: the initial commit is 76k lines.
- Whether the nvim-orgmode grammar works with tree-sitter ≥0.25 if its bindings were regenerated. Its
  wasm32-unknown-unknown path was not resolved, because the tree-sitter 0.26 runtime refused that
  target.
- uniorg and pandoc, which were not run hands-on. Health data only for uniorg: GPL-3.0, 3 active
  committers, median first response 0.2 days.
- Emacs differential coverage:
  - element types plus links only (no other inline objects);
  - 6 Worg files excluded for an oracle offset-mapping problem;
  - Org 9.7.11 only (orgcst targets 10.0-pre);
  - `org-inlinetask` not loaded.
- wasm with wasm-bindgen glue and wasm-opt. Only raw cdylib sizes were measured.
- macOS/Windows behaviour and timing. All numbers come from one Linux machine.
- MAE's current parser performance, as a baseline.
- Whether orgize's in-repo snapshot tests check conformance or only snapshot stability. They were
  counted, not read.
