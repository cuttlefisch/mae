//! Gate 3 (ADR-112 D5): hostile inputs that double in size must take
//! roughly linearly more time. Every parser the research evaluated failed
//! this somewhere (`docs/research/112-org-parser-prior-art.md`), and MAE
//! re-parses on every change, so super-linear time is a denial of service.
//!
//! The TIMING assertion runs only in release builds (`cargo test -p mae-org
//! --release --test scaling -- --nocapture`); debug builds parse every input
//! once for totality, so debug CI cannot be flaky. Each size is timed as the
//! minimum of several runs. The bound is on growth per input byte: with B the
//! byte ratio of the two inputs, t(8N)/t(N) · 8/B < 24, where linear gives ~8,
//! n·log n ~9–10 and quadratic 64.
//!
//! `semantics()` is held to the same bound (the "sem:" families after the
//! parse ones, timed alone on a document parsed outside the clock): the
//! projection runs on every peer and the daemon on every change too. Their
//! measurements are listed after the parse families' below.
//!
//! Measured on the development machine (Linux x86_64, release, 2026-10-31,
//! with every O2 inline parser; absolute times vary ~2x with machine load,
//! ratios do not), t(N) → t(8N), the byte ratio, and the normalised ratio
//! (two runs agreed within ±0.5 except "property-like lines", "drawer
//! openers" and "blank lines", which swing between x4 and x13 from run to
//! run at these sizes: allocation, not complexity, as they are linear per
//! doubling above 100k lines). The inline families below "N #+TODO lines"
//! are one paragraph line each; the radio families include a 5,000-word
//! target whose prefix matches the whole text, which a match attempt per
//! position (Emacs's regexp search) makes quadratic, and objects before a run
//! of rejected candidates (removing the per-frame cursor makes it x60):
//! - *a x N (one line): 5.38 ms -> 26.63 ms for x8 bytes, normalised x4.9
//! - [[ x N: 5.09 ms -> 38.65 ms for x8 bytes, normalised x7.6
//! - nested same-name quotes: 4.37 ms -> 36.47 ms for x8 bytes, normalised x8.4
//! - nested distinct blocks (capped): 1.39 ms -> 10.54 ms for x9 bytes, normalised x7.1
//! - deep list: 0.76 ms -> 48.13 ms for x62 bytes, normalised x8.2
//! - one long line of words: 9.56 ms -> 90.68 ms for x8 bytes, normalised x9.5
//! - headings: 2.32 ms -> 20.58 ms for x9 bytes, normalised x8.1
//! - affiliated orphans: 3.58 ms -> 32.15 ms for x8 bytes, normalised x9.0
//! - table.el near-misses: 12.91 ms -> 120.81 ms for x8 bytes, normalised x9.4
//! - unterminated begins: 6.41 ms -> 54.48 ms for x8 bytes, normalised x8.5
//! - property-like lines: 4.12 ms -> 34.57 ms for x8 bytes, normalised x8.4
//! - drawer openers: 2.74 ms -> 23.33 ms for x8 bytes, normalised x8.5
//! - footnotes: 4.67 ms -> 42.24 ms for x8 bytes, normalised x9.1
//! - blank lines: 3.53 ms -> 38.15 ms for x8 bytes, normalised x10.8
//! - TODO: N keywords, N headlines: 2.17 ms -> 19.13 ms for x9 bytes, normalised x7.9
//! - N #+TODO lines: 4.68 ms -> 42.38 ms for x9 bytes, normalised x8.4
//! - = and ~ storm: 2.93 ms -> 23.33 ms for x8 bytes, normalised x8.0
//! - openers, no closers: 3.53 ms -> 28.27 ms for x8 bytes, normalised x8.0
//! - plain-link-like: 3.03 ms -> 24.21 ms for x8 bytes, normalised x8.0
//! - backslash run: 0.51 ms -> 4.14 ms for x8 bytes, normalised x8.2
//! - {{{a unclosed: 1.53 ms -> 12.71 ms for x8 bytes, normalised x8.3
//! - $ storm: 2.22 ms -> 19.34 ms for x8 bytes, normalised x8.7
//! - <1 run: 5.82 ms -> 47.25 ms for x8 bytes, normalised x8.1
//! - [1 run: 3.61 ms -> 29.51 ms for x8 bytes, normalised x8.2
//! - unclosed timestamps: 3.26 ms -> 26.29 ms for x8 bytes, normalised x8.1
//! - repeater candidates, one far >: 1.52 ms -> 12.13 ms for x8 bytes, normalised x8.0
//!   (before the indexed digit run: 24.38 ms -> 1,377.78 ms, x56.5)
//! - [fn: unclosed: 3.00 ms -> 23.97 ms for x8 bytes, normalised x8.0
//! - src_x[ unclosed: 6.18 ms -> 51.80 ms for x8 bytes, normalised x8.4
//! - call_x( unclosed: 6.46 ms -> 53.85 ms for x8 bytes, normalised x8.3
//! - @@a: unclosed: 3.58 ms -> 30.53 ms for x8 bytes, normalised x8.5
//! - <<< unclosed: 4.65 ms -> 37.19 ms for x8 bytes, normalised x8.0
//! - \alpha storm: 3.73 ms -> 30.08 ms for x8 bytes, normalised x8.1
//! - ^{ _{ unclosed: 4.29 ms -> 35.22 ms for x8 bytes, normalised x8.2
//! - [cite: unclosed: 3.33 ms -> 26.86 ms for x8 bytes, normalised x8.1
//! - \( unclosed: 1.72 ms -> 14.11 ms for x8 bytes, normalised x8.2
//! - table cells: 3.09 ms -> 26.85 ms for x8 bytes, normalised x8.7
//!
//! The radio families, re-measured 2026-11-01 after the fix round (review
//! F1-F3, F10). From "nested radio targets" on, the larger input's targets
//! pass Org's 8192-char regexp limit, so it is matched as split regexps
//! (`org-target-link-regexps`) and the smaller is not: the ratio includes
//! that change of mode. "distinct mixed targets on one node" spends the work
//! budget (RadioLinkBudget) by design; time stays linear.
//! - radio links: 7.54 ms -> 65.15 ms for x8 bytes, normalised x8.6
//! - long radio target: 9.65 ms -> 85.33 ms for x8 bytes, normalised x8.9
//! - radio candidates rejected: 4.02 ms -> 34.35 ms for x8 bytes, normalised x8.5
//! - blank-start radio target: 2.71 ms -> 21.60 ms for x8 bytes, normalised x8.0
//!   (before the linear matcher: 680 ms -> 43,105 ms, x63.5)
//! - nested radio targets: 2.69 ms -> 23.68 ms for x8 bytes, normalised x9.0
//!   (before: 18.24 ms -> 524.87 ms, x29.5)
//! - nested tab radio targets: 3.10 ms -> 26.85 ms for x8 bytes, normalised x8.9
//! - nested mixed radio targets: 3.75 ms -> 42.56 ms for x8 bytes, normalised x11.5
//! - mixed radio targets failing late: 32.11 ms -> 184.84 ms for x8 bytes, normalised x5.9
//! - case-variant blank-start mixed targets: 5.10 ms -> 42.36 ms for x8 bytes, normalised x8.3
//! - nested blank-start mixed targets, lead fails: 3.94 ms -> 33.41 ms for x8 bytes, normalised x8.7
//! - nested blank-start mixed targets, lead matches: 3.94 ms -> 33.76 ms for x8 bytes, normalised x8.7
//! - distinct mixed targets on one node: 34.89 ms -> 417.62 ms for x14 bytes, normalised x6.7
//! - target of N distinct ascending chars: 3.26 ms -> 27.34 ms for x8 bytes, normalised x8.4
//!   (before: a sorted transition list per state, 2.86 ms -> 102.11 ms, x35.7)
//! - split radio regexps (glossary): 27.19 ms -> 253.21 ms for x8 bytes, normalised x8.9
//! - radio link before [[ (Emacs loops): 9.96 ms -> 87.96 ms for x8 bytes, normalised x8.8
//! - split regexps, objects before a long run (review B3, 2026-11-01):
//!   6.57 ms -> 27.09 ms for x5 bytes, normalised x6.8 (before the
//!   incremental split search: 1.8 s at 33 KB, 7.6 s at 58 KB, 32 s at
//!   108 KB, with no diagnostic)
//! - split regexps, objects before a long target: 6.31 ms -> 23.32 ms for
//!   x5 bytes, normalised x6.4 (before: 14.8 s at 192 KB)
//!
//! The per-candidate work on mixed targets (review F3 a, b) grew as N^1.5,
//! which at these sizes is only x22.6 per 8x and hides under this bound; it
//! is gated exactly, by counting visits, in `inline::radio`'s
//! `mixed_target_work_per_candidate_is_bounded`, and the split search's
//! steps per input byte by `radio_work_per_input_byte_is_bounded`.
//!
//! The `semantics()` families (2026-10-31, after the review fixes; before
//! them the first six were x47–x391: a scan of the item's children per
//! drawer, of the tags so far per tag, of the keys so far per key, of the
//! exclude tags per tag, and one set of every file tag per headline; "N
//! footnotes" swings x4–x8 between runs, allocation again):
//! - sem: item with N plain drawers: 5.11 ms -> 40.98 ms for x8 bytes, normalised x8.0
//! - sem: N filetags, N headline nodes: 3.12 ms -> 24.83 ms for x9 bytes, normalised x7.5
//! - sem: one drawer, N distinct keys: 20.83 ms -> 189.11 ms for x9 bytes, normalised x8.3
//! - sem: one headline, N tags: 1.03 ms -> 8.27 ms for x9 bytes, normalised x6.9
//! - sem: one file node, N filetags: 2.20 ms -> 18.73 ms for x9 bytes, normalised x7.3
//! - sem: N EXCLUDE_TAGS, N tagged headlines: 1.71 ms -> 14.02 ms for x9 bytes, normalised x7.4
//! - sem: N footnotes, each a link: 6.94 ms -> 31.98 ms for x9 bytes, normalised x4.3
//! - sem: footnote chain of N: 3.33 ms -> 31.89 ms for x9 bytes, normalised x8.9
//! - sem: N headline ids, half duplicates: 6.41 ms -> 54.41 ms for x8 bytes, normalised x8.3
//! - sem: N links in one paragraph: 8.79 ms -> 68.77 ms for x8 bytes, normalised x7.8
//! - sem: one drawer, N KEY+ appends: 10.46 ms -> 85.37 ms for x8 bytes, normalised x8.2
//! - sem: N transclusions: 25.99 ms -> 218.40 ms for x8 bytes, normalised x8.4
//! - sem: N links and N kept notes in one unrendered region (review F4, 2026-11-01):
//!   10.54 ms -> 88.12 ms for x9 bytes, normalised x7.8 (before: a scan of every
//!   kept definition per link, 17.90 ms -> 601.17 ms, x31.3)
//! - sem: N chained tag groups (review F5): 26.01 ms -> 219.46 ms for x9 bytes,
//!   normalised x7.9
//!
//! The renderer families (2026-10-31; `render()`, `to_html` and `to_hast`
//! timed together on a parsed, projected document). "deep list" is cut by
//! the parser's nesting cap (128 levels) at 8N, so its large input renders
//! no deeper than its small one: that is the cap working. Before anchors
//! kept a next-suffix per base, "N headlines, one title" scanned `-2`, `-3`
//! … per repeat, which is quadratic:
//! - render: deep list: 0.19 ms -> 0.19 ms for x60 bytes, normalised x0.1
//! - render: nested quotes: 4.11 ms -> 33.73 ms for x8 bytes, normalised x8.2
//! - render: N footnotes: 16.39 ms -> 174.95 ms for x9 bytes, normalised x9.9
//! - render: footnote chain of N: 16.79 ms -> 202.65 ms for x9 bytes, normalised x11.3
//! - render: N links in one paragraph: 17.11 ms -> 150.34 ms for x8 bytes, normalised x8.6
//! - render: large table: 15.74 ms -> 148.76 ms for x8 bytes, normalised x9.5
//! - render: long paragraph: 34.16 ms -> 296.81 ms for x8 bytes, normalised x8.7
//! - render: N headlines, one title: 10.59 ms -> 95.50 ms for x8 bytes, normalised x9.0
//! - render: N radio links and targets: 17.23 ms -> 147.56 ms for x8 bytes, normalised x8.6
//! - render: N headline nodes with titles: 17.25 ms -> 167.52 ms for x8 bytes, normalised x9.2
//!
//! `replace_body` (review round 3, 2026-11-01; before, its rendering check
//! rendered every node's page and 8,000 nodes took 12.8 s):
//! - splice: N nodes with links: 60.52 ms -> 654.95 ms for x9 bytes, normalised x10.1

use mae_org::{parse, render, semantics, ParseOptions, RenderOptions, Resolved, Resolver};
#[cfg(not(debug_assertions))]
use std::time::{Duration, Instant};

type Family = (&'static str, fn(usize) -> String, usize);

/// Each family: a name, a generator for size N, and a base N.
const FAMILIES: &[Family] = &[
    ("*a x N (one line)", |n| "*a ".repeat(n), 20_000),
    ("[[ x N", |n| "[[".repeat(n), 50_000),
    (
        "nested same-name quotes",
        |n| {
            format!(
                "{}x\n{}",
                "#+begin_quote\n".repeat(n),
                "#+end_quote\n".repeat(n)
            )
        },
        2_000,
    ),
    ("nested distinct blocks (capped)", nested_blocks, 2_000),
    // Depth d needs d columns of indentation, so bytes grow as N²; the
    // bound below is on time per byte, which is what a DoS budget is in.
    (
        "deep list",
        |n| {
            (0..n)
                .map(|i| format!("{}- {i}\n", " ".repeat(i)))
                .collect()
        },
        250,
    ),
    (
        "one long line of words",
        |n| "word *b* /i/ [[l][d]] ".repeat(n),
        6_000,
    ),
    (
        "headings",
        |n| {
            (0..n)
                .map(|i| format!("{} h{i}\n", "*".repeat(1 + i % 5)))
                .collect()
        },
        2_500,
    ),
    (
        "affiliated orphans",
        |n| format!("{}\n", "#+NAME: x\n".repeat(n)),
        5_000,
    ),
    ("table.el near-misses", |n| "+-+\n+ x\n".repeat(n), 5_000),
    (
        "unterminated begins",
        |n| "#+begin_src\ntext\n".repeat(n),
        5_000,
    ),
    (
        "property-like lines",
        |n| format!("* h\n:PROPERTIES:\n{}", ":a: b\n".repeat(n)),
        10_000,
    ),
    ("drawer openers", |n| ":D:\nx\n".repeat(n), 5_000),
    ("footnotes", |n| "[fn:1] a\n".repeat(n), 5_000),
    ("blank lines", |n| "\n".repeat(n), 50_000),
    // One `#+TODO:` with N keywords, then N headlines (review finding: the
    // keyword match scanned every keyword per headline, x4 per doubling).
    ("TODO: N keywords, N headlines", todo_many_keywords, 2_000),
    // N `#+TODO:` lines, each followed by a headline.
    (
        "N #+TODO lines",
        |n| {
            (0..n)
                .map(|i| format!("#+TODO: A{i}\n* B{i} t\n"))
                .collect()
        },
        2_000,
    ),
    // Inline objects (O2). Each is one paragraph line unless noted: the
    // shapes on which a forward search per opener would go quadratic.
    ("= and ~ storm", |n| "=a ~b ".repeat(n), 10_000),
    (
        "openers, no closers",
        |n| "*a /b _c +d =e ~f ".repeat(n),
        4_000,
    ),
    ("plain-link-like", |n| "http:x ".repeat(n), 10_000),
    ("backslash run", |n| "\\".repeat(n), 50_000),
    ("{{{a unclosed", |n| "{{{a(".repeat(n), 10_000),
    ("$ storm", |n| "$a ".repeat(n), 20_000),
    ("<1 run", |n| "<1".repeat(n), 30_000),
    ("[1 run", |n| "[1".repeat(n), 30_000),
    ("unclosed timestamps", |n| "<2026-01-01 a ".repeat(n), 5_000),
    // Repeater candidates sharing one far `>` after a long digit run (review
    // finding: each candidate rescanned the run backwards, x30-x60).
    (
        "repeater candidates, one far >",
        |n| format!("{} {}d>", "<1-1-1".repeat(n), "9".repeat(6 * n)),
        3_000,
    ),
    ("[fn: unclosed", |n| "[fn:a: ".repeat(n), 10_000),
    ("src_x[ unclosed", |n| "src_x[a ".repeat(n), 10_000),
    ("call_x( unclosed", |n| "call_x(a ".repeat(n), 10_000),
    ("@@a: unclosed", |n| "@@a:b ".repeat(n), 10_000),
    ("<<< unclosed", |n| "<<<a ".repeat(n), 10_000),
    ("\\alpha storm", |n| "\\alpha".repeat(n), 10_000),
    ("^{ _{ unclosed", |n| "x^{a x_{b ".repeat(n), 10_000),
    ("[cite: unclosed", |n| "[cite:@a ".repeat(n), 10_000),
    ("\\( unclosed", |n| "\\(a ".repeat(n), 10_000),
    (
        "table cells",
        |n| format!("|{}\n", " a |".repeat(n)),
        10_000,
    ),
    // Radio links: many short ones, and a long target whose prefix matches
    // the whole text (a match attempt per position would be quadratic).
    (
        "radio links",
        |n| format!("<<<ab>>>\n{}\n", "ab ".repeat(n)),
        10_000,
    ),
    ("long radio target", long_radio_target, 5_000),
    // Objects before a run of rejected radio candidates (`xab` has no
    // border): each object's search must not re-examine the run.
    (
        "radio candidates rejected",
        rejected_radio_candidates,
        3_000,
    ),
    // A target starting with a no-break space before a long run of them
    // that ends in a failing match (review: 32 KB took 10.7 s, x63-x89).
    ("blank-start radio target", blank_start_radio, 4_000),
    // Targets `a`, `a a`, … (√N of them) before `a `×N: every word starts
    // a match of every target (review: memory S^1.5; see scaling_memory.rs).
    ("nested radio targets", |n| nested_radio(n, " "), 4_000),
    ("nested tab radio targets", |n| nested_radio(n, "\t"), 4_000),
    // A tab next to a space: mixed targets, matched by backtracking within
    // a work budget (past it: RadioLinkBudget, tests/inline.rs).
    (
        "nested mixed radio targets",
        |n| nested_radio(n, "\t "),
        4_000,
    ),
    (
        "mixed radio targets failing late",
        mixed_failing_late,
        4_000,
    ),
    // Review F2/F3: many case variants of one blank-start mixed target on
    // one trie node (each was tried per candidate, and its failed states
    // memoised: 2.4 MB of input took 1.33 GB).
    (
        "case-variant blank-start mixed targets",
        case_variant_targets,
        4_000,
    ),
    // Nested blank-start mixed targets: the walk up the chain was O(√N) per
    // candidate. Their leading stretch fails (the text has a tab where the
    // targets have a no-break space), and passes.
    (
        "nested blank-start mixed targets, lead fails",
        |n| nested_blank_start(n, "\t a"),
        4_000,
    ),
    (
        "nested blank-start mixed targets, lead matches",
        |n| nested_blank_start(n, "\u{a0} a"),
        4_000,
    ),
    // Distinct mixed targets on one node (they differ only in a stretch),
    // over text that holds tabs: each candidate tries them until the work
    // budget is spent (RadioLinkBudget), so time stays linear.
    (
        "distinct mixed targets on one node",
        distinct_mixed_on_one_node,
        4_000,
    ),
    // One target of N distinct chars in ascending order: building the suffix
    // automaton inserted into a sorted list per state (9.8 s at 1.2 MB).
    (
        "target of N distinct ascending chars",
        distinct_char_target,
        4_000,
    ),
    // Enough targets that Org splits its regexp (org-target-link-regexps):
    // the search takes each regexp's leftmost match and the earliest end.
    ("split radio regexps (glossary)", glossary, 2_000),
    // The shape on which Emacs never terminates, repeated.
    (
        "radio link before [[ (Emacs loops)",
        |n| format!("<<<x>>>\n\n{}\n", "x[[x]] x[[/^\n".repeat(n)),
        3_000,
    ),
    // Review B3: split regexps (an 8150-char target), and N objects before
    // a match across a long run whose every char is a candidate (a lone
    // no-break-space target): every lexer call rescanned the run (33 KB
    // took 1.8 s, 108 KB 32 s).
    (
        "split regexps, objects before a long run",
        split_long_run,
        2_000,
    ),
    // The same before a long target sharing its regexp with `a`.
    (
        "split regexps, objects before a long target",
        split_long_target,
        2_000,
    ),
];

fn split_long_run(n: usize) -> String {
    format!(
        "<<<{}>>>\n\n<<<a b>>> <<<\u{a0}>>>\n\n{}a{}b\n",
        "q".repeat(8150),
        "*x* ".repeat(n),
        " ".repeat(n)
    )
}

fn split_long_target(n: usize) -> String {
    let target = vec!["a"; n / 10 + 1].join(" ");
    format!(
        "<<<{}>>>\n\n<<<{target}>>> <<<a>>>\n\n{}{target}\n",
        "q".repeat(8150),
        "*x* ".repeat(n)
    )
}

/// `<<<\u{a0} abcdefghijklmnop>>>` in N/40 case variants, then the word
/// after two ASCII spaces, N/2 times.
fn case_variant_targets(n: usize) -> String {
    let word = "abcdefghijklmnop";
    let targets: String = (0..(n / 40).clamp(2, 1 << 16))
        .map(|v| {
            let w: String = word
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    if v >> i & 1 == 1 {
                        c.to_ascii_uppercase()
                    } else {
                        c
                    }
                })
                .collect();
            format!("<<<\u{a0} {w}>>>\n")
        })
        .collect();
    format!("{targets}\n{}\n", "  abcdefghijklmnop".repeat(n / 2))
}

/// Targets `\u{a0} a`, `\u{a0} a a`, … (√N of them), then `unit` N times.
fn nested_blank_start(n: usize, unit: &str) -> String {
    let k = (n as f64).sqrt() as usize;
    let targets: String = (1..=k)
        .map(|j| format!("<<<\u{a0}{}>>>\n", " a".repeat(j)))
        .collect();
    format!("{targets}\n{}\n", unit.repeat(n))
}

/// `x` + space + i tabs + `y`, for i up to N/50: one run-collapsed shape,
/// N/50 distinct targets; then `x \t\ty ` N times (none matches it).
fn distinct_mixed_on_one_node(n: usize) -> String {
    let targets: String = (3..3 + n / 50)
        .map(|i| format!("<<<x {}y>>>\n", "\t".repeat(i)))
        .collect();
    format!("{targets}\n{}\n", "x \t\ty ".repeat(n))
}

/// One radio target of N distinct chars, ascending, then the same text.
fn distinct_char_target(n: usize) -> String {
    let t: String = (0..n as u32)
        .filter_map(|i| char::from_u32(0x4E00 + i))
        .collect();
    format!("<<<{t}>>>\n\n{t}\n")
}

/// N two-word glossary terms (their regexp is far past Org's 8192-char
/// limit), then text using them, with words that are not terms between.
fn glossary(n: usize) -> String {
    let word = |i: usize| format!("w{}", i % 211);
    let term = |i: usize| format!("{} t{i}", word(i * 7));
    let defs: String = (0..n)
        .map(|i| format!("- <<<{}>>> :: d\n", term(i)))
        .collect();
    let uses: String = (0..n)
        .map(|i| format!("{} {} and {} ", word(i), term((i * 13) % n), term(i)))
        .collect();
    format!("{defs}\n{uses}\n")
}

/// Mixed targets whose last stretch fails at every word: bounded by the
/// work budget, not by targets × length per word.
fn mixed_failing_late(n: usize) -> String {
    let k = (n as f64).sqrt() as usize;
    let targets: String = (1..=k)
        .map(|j| format!("<<<{}a \ta>>>\n", "a\t ".repeat(j)))
        .collect();
    format!("{targets}\n{}\n", "a\t ".repeat(n))
}

fn blank_start_radio(n: usize) -> String {
    format!("<<<\u{a0} x>>>\n\n{}xa\n", "\u{a0}".repeat(n))
}

fn nested_radio(n: usize, sep: &str) -> String {
    let k = (n as f64).sqrt() as usize;
    let targets: String = (1..=k)
        .map(|j| format!("<<<{}>>>\n", vec!["a"; j].join(sep)))
        .collect();
    format!("{targets}\n{}\n", format!("a{sep}").repeat(n))
}

fn rejected_radio_candidates(n: usize) -> String {
    format!("<<<ab>>>\n{}{} ab\n", "*a* ".repeat(n), "xab".repeat(n))
}

fn long_radio_target(n: usize) -> String {
    format!("<<<{}b>>>\n{}\n", "a ".repeat(n), "a ".repeat(n))
}

fn todo_many_keywords(n: usize) -> String {
    let kws: Vec<String> = (0..n).map(|i| format!("K{i}")).collect();
    let heads: String = (0..n).map(|i| format!("* K{} t\n", n - 1 - i)).collect();
    format!("#+TODO: {} | D\n{heads}", kws.join(" "))
}

fn nested_blocks(n: usize) -> String {
    let open: String = (0..n).map(|i| format!("#+begin_b{i}\n")).collect();
    let close: String = (0..n).rev().map(|i| format!("#+end_b{i}\n")).collect();
    format!("{open}core\n{close}")
}

/// `semantics()` families (ADR-112 D5 gate 3 applied to the projection):
/// timed alone, on a document parsed once outside the clock. Each is a
/// shape on which a per-element scan of a growing collection (the item's
/// children, the tags so far, the keys so far, the exclude tags) went
/// quadratic or worse before the review that added them.
const SEMANTIC_FAMILIES: &[Family] = &[
    ("sem: item with N plain drawers", item_with_drawers, 2_000),
    ("sem: N filetags, N headline nodes", filetags_and_nodes, 500),
    ("sem: one drawer, N distinct keys", drawer_with_keys, 8_000),
    ("sem: one headline, N tags", headline_with_tags, 4_000),
    (
        "sem: one file node, N filetags",
        file_node_with_filetags,
        4_000,
    ),
    (
        "sem: N EXCLUDE_TAGS, N tagged headlines",
        exclude_tags_and_headlines,
        1_000,
    ),
    ("sem: N footnotes, each a link", footnotes_with_links, 2_000),
    ("sem: footnote chain of N", footnote_chain, 2_000),
    ("sem: N headline ids, half duplicates", headline_ids, 2_000),
    (
        "sem: N links in one paragraph",
        |n| {
            format!(
                ":PROPERTIES:\n:ID: f\n:END:\n{}\n",
                "[[id:a?rel=r#f][d]] ".repeat(n)
            )
        },
        5_000,
    ),
    (
        "sem: one drawer, N KEY+ appends",
        |n| {
            format!(
                ":PROPERTIES:\n:ID: f\n:K: v\n{}:END:\n",
                ":K+: w\n".repeat(n)
            )
        },
        5_000,
    ),
    // Review F4: N links and N kept footnote definitions in one unrendered
    // region (each link was checked against every kept definition).
    (
        "sem: N links and N kept notes in one unrendered region",
        links_and_kept_notes,
        2_000,
    ),
    // Review F5: N chained tag groups expanded from N exclude tags (each
    // tag's expansion must not walk the chain again).
    ("sem: N chained tag groups", chained_tag_groups, 2_000),
    (
        "sem: N transclusions",
        |n| {
            format!(
                ":PROPERTIES:\n:ID: f\n:END:\n{}",
                "#+TRANSCLUDE: [[id:m][M]] :level 2\n".repeat(n)
            )
        },
        5_000,
    ),
];

fn links_and_kept_notes(n: usize) -> String {
    let refs: String = (0..n).map(|i| format!("r[fn:{i}] ")).collect();
    let links: String = (0..n).map(|i| format!("[[id:x{i}]]\n")).collect();
    let defs: String = (0..n).map(|i| format!("[fn:{i}] d\n\n")).collect();
    format!(":PROPERTIES:\n:ID: f\n:END:\n{refs}\n* H :noexport:\n{links}\n{defs}")
}

fn chained_tag_groups(n: usize) -> String {
    let groups: String = (0..n)
        .map(|i| format!("#+TAGS: [ g{i} : g{} ]\n", i + 1))
        .collect();
    let exclude: Vec<String> = (0..n).map(|i| format!("g{i}")).collect();
    let heads: String = (0..n)
        .map(|i| format!("* h{i} :q{i}:\n:PROPERTIES:\n:ID: h{i}\n:END:\n"))
        .collect();
    format!("#+EXCLUDE_TAGS: {}\n{groups}{heads}", exclude.join(" "))
}

fn item_with_drawers(n: usize) -> String {
    format!(
        "- item\n  :PROPERTIES:\n  :ID: i\n  :END:\n{}",
        "  :D:\n  [[id:x]]\n  :END:\n".repeat(n)
    )
}

fn tag_run(prefix: &str, n: usize) -> String {
    let tags: Vec<String> = (0..n).map(|i| format!("{prefix}{i}")).collect();
    format!(":{}:", tags.join(":"))
}

fn filetags_and_nodes(n: usize) -> String {
    let nodes: String = (0..n)
        .map(|i| format!("* h{i}\n:PROPERTIES:\n:ID: h{i}\n:END:\n"))
        .collect();
    format!(
        ":PROPERTIES:\n:ID: f\n:END:\n#+filetags: {}\n{nodes}",
        tag_run("t", n)
    )
}

fn drawer_with_keys(n: usize) -> String {
    let keys: String = (0..n).map(|i| format!(":K{i}: v\n")).collect();
    format!(":PROPERTIES:\n:ID: f\n{keys}:END:\n")
}

fn headline_with_tags(n: usize) -> String {
    format!("* h {}\n:PROPERTIES:\n:ID: h\n:END:\n", tag_run("t", n))
}

fn file_node_with_filetags(n: usize) -> String {
    format!(
        ":PROPERTIES:\n:ID: f\n:END:\n#+filetags: {}\n",
        tag_run("t", n)
    )
}

fn exclude_tags_and_headlines(n: usize) -> String {
    let words: Vec<String> = (0..n).map(|i| format!("x{i}")).collect();
    let heads: String = (0..n).map(|i| format!("* h :y{i}:\nt\n")).collect();
    format!("#+EXCLUDE_TAGS: {}\n{heads}", words.join(" "))
}

fn footnotes_with_links(n: usize) -> String {
    let refs: String = (0..n).map(|i| format!("r[fn:{i}] ")).collect();
    let defs: String = (0..n)
        .map(|i| format!("[fn:{i}] [[id:t{i}]]\n\n"))
        .collect();
    format!(":PROPERTIES:\n:ID: f\n:END:\n{refs}\n\n{defs}")
}

fn footnote_chain(n: usize) -> String {
    let defs: String = (0..n)
        .map(|i| format!("[fn:{i}] [[id:t{i}]] next[fn:{}]\n\n", i + 1))
        .collect();
    format!(":PROPERTIES:\n:ID: f\n:END:\nr[fn:0]\n\n{defs}")
}

fn headline_ids(n: usize) -> String {
    (0..n)
        .map(|i| format!("* h\n:PROPERTIES:\n:ID: i{}\n:END:\n[[id:x]]\n", i / 2))
        .collect()
}

/// Renderer families (ADR-112 D3 under gate 3): `render()` and both
/// serialisations, timed alone on a document parsed and projected outside
/// the clock. Each is a shape on which a per-element scan of a growing
/// collection would go quadratic: anchors claimed so far (N headlines with
/// one title), notes numbered so far, links resolved so far, cells.
/// `replace_body` on the first node of N (review round 3: its rendering
/// check rendered every node's page, 8,000 nodes took 12.8 s); the walks
/// per edit are also counted, exactly, in `render::source`'s unit test.
const SPLICE_FAMILIES: &[Family] = &[
    ("splice: N nodes with links", splice_nodes, 500),
    // Review round 4 (M4): every node linking the first, the reviewer's
    // shape (32,000 nodes took 11.2 s; one render of them, 1 s).
    (
        "splice: N nodes linking one",
        splice_nodes_one_target,
        2_000,
    ),
];

fn splice_nodes_one_target(n: usize) -> String {
    (0..n)
        .map(|i| format!("* N{i}\n:PROPERTIES:\n:ID: n{i}\n:END:\nbody {i} [[id:n0]]\n"))
        .collect()
}

fn splice_nodes(n: usize) -> String {
    // The edited node (the first) holds no note: removing one numbered
    // before the others' renumbers theirs, and is refused.
    (0..n)
        .map(|i| match i {
            0 => "* N0\n:PROPERTIES:\n:ID: n0\n:END:\nbody 0 [[id:n1]]\n".to_string(),
            _ => format!("* N{i}\n:PROPERTIES:\n:ID: n{i}\n:END:\nbody {i} [[id:n{}]] note[fn:{i}]\n\n[fn:{i}] note {i}\n", (i + 1) % n),
        })
        .collect()
}

#[cfg(not(debug_assertions))]
fn time_splice(text: &str) -> Duration {
    let (popts, sopts) = (
        ParseOptions::default(),
        mae_org::SemanticsOptions::default(),
    );
    let doc = parse(text, &popts);
    let sem = semantics(&doc);
    let mut best = Duration::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        let out = mae_org::replace_body(text, &doc, &sem.nodes[0], "new body\n", &popts, &sopts);
        let d = t.elapsed();
        assert!(out.is_ok(), "{out:?}");
        best = best.min(d);
    }
    best
}

const RENDER_FAMILIES: &[Family] = &[
    (
        "render: deep list",
        |n| {
            (0..n)
                .map(|i| format!("{}- {i}\n", " ".repeat(i)))
                .collect()
        },
        100,
    ),
    (
        "render: nested quotes",
        |n| {
            format!(
                "{}x\n{}",
                "#+begin_quote\n".repeat(n),
                "#+end_quote\n".repeat(n)
            )
        },
        2_000,
    ),
    ("render: N footnotes", footnotes_with_links, 2_000),
    ("render: footnote chain of N", footnote_chain, 2_000),
    (
        "render: N links in one paragraph",
        |n| {
            (0..n)
                .map(|i| format!("[[id:t{}?rel=r&w=0.5#f][d]] [[https://x.org/{i}]] ", i % 50))
                .collect()
        },
        3_000,
    ),
    (
        "render: large table",
        |n| {
            let row: String = (0..10).map(|j| format!(" c{j} *b* |")).collect();
            format!("| h |\n|---|\n{}", format!("|{row}\n").repeat(n))
        },
        1_000,
    ),
    (
        "render: long paragraph",
        |n| "word *b* /i/ =v= x^2 \\alpha [[l][d]] ".repeat(n),
        4_000,
    ),
    (
        "render: N headlines, one title",
        |n| "* Same title\nbody\n".repeat(n),
        2_000,
    ),
    (
        "render: N radio links and targets",
        |n| format!("<<<ab>>> {}\n{}", "<<t>> ".repeat(n), "ab ".repeat(n)),
        5_000,
    ),
    (
        "render: N macros expanding plainly",
        |n| {
            format!(
                "#+MACRO: m <$1>\n#+MACRO: w {{{{{{m($1)}}}}}}\n{}\n",
                "{{{w(x)}}} {{{n}}} ".repeat(n)
            )
        },
        2_000,
    ),
    (
        // Review round 4 (B7): a template of thousands of words called
        // hundreds of times by another, called N times; fails within the
        // expansion budget, which is linear in the source.
        "render: the exponential macro shape",
        |n| {
            format!(
                "#+MACRO: b {}\n#+MACRO: a {}\n{}\n",
                "x ".repeat(3000),
                "{{{b}}}".repeat(255),
                "{{{a}}} ".repeat(n)
            )
        },
        25,
    ),
    (
        "render: N headline nodes with titles",
        |n| {
            (0..n)
                .map(|i| {
                    format!(
                        "* H{i} [[id:t{i}]]\n:PROPERTIES:\n:ID: n{i}\n:ALIASES: a{i}, b\n:END:\n"
                    )
                })
                .collect()
        },
        1_000,
    ),
];

/// A resolver for `t0`…`t49`.
fn resolver() -> Resolver {
    let mut r = Resolver::default();
    for i in 0..50 {
        let href = format!("/t{i}.html");
        r.targets.insert(format!("t{i}"), Resolved::new(href));
    }
    r
}

/// The minimum of up to seven runs of rendering `text` (to the tree, HTML
/// and hast JSON), parsed and projected once outside the clock.
#[cfg(not(debug_assertions))]
fn time_render(text: &str) -> Duration {
    let doc = parse(text, &ParseOptions::default());
    let sem = semantics(&doc);
    let (opts, r) = (RenderOptions::default(), resolver());
    let mut best = Duration::MAX;
    for _ in 0..7 {
        let t = Instant::now();
        let out = render(&doc, &sem, &opts, &r);
        let html = mae_org::to_html(&out.body);
        let hast = mae_org::to_hast(&out.body);
        let d = t.elapsed();
        std::hint::black_box((out, html, hast));
        best = best.min(d);
        if d > Duration::from_secs(2) {
            break;
        }
    }
    best
}

#[cfg(not(debug_assertions))]
fn time(text: &str) -> Duration {
    let opts = ParseOptions::default();
    (0..7)
        .map(|_| {
            let t = Instant::now();
            let doc = parse(text, &opts);
            let d = t.elapsed();
            assert_eq!(doc.syntax().text_range().len(), (text.len() as u32).into());
            d
        })
        .min()
        .unwrap_or_default()
}

/// Debug builds only: in release the timing test below is the only test in
/// this binary, so nothing runs beside it and skews its clock.
#[cfg(debug_assertions)]
#[test]
fn every_family_parses_and_round_trips_at_8n() {
    for (name, gen, base) in FAMILIES {
        let text = gen(base * 8);
        let doc = parse(&text, &ParseOptions::default());
        assert_eq!(doc.syntax().to_string(), text, "{name}");
    }
    for (name, gen, base) in SEMANTIC_FAMILIES {
        let text = gen(base * 8);
        let s = semantics(&parse(&text, &ParseOptions::default()));
        assert!(!s.nodes.is_empty() || name.contains("EXCLUDE"), "{name}");
    }
    for (name, gen, base) in SPLICE_FAMILIES {
        let text = gen(*base);
        let (popts, sopts) = (
            ParseOptions::default(),
            mae_org::SemanticsOptions::default(),
        );
        let doc = parse(&text, &popts);
        let sem = semantics(&doc);
        let out = mae_org::replace_body(&text, &doc, &sem.nodes[0], "new\n", &popts, &sopts);
        assert!(out.is_ok(), "{name}: {out:?}");
    }
    for (name, gen, base) in RENDER_FAMILIES {
        let text = gen(*base);
        let doc = parse(&text, &ParseOptions::default());
        let out = render(
            &doc,
            &semantics(&doc),
            &RenderOptions::default(),
            &resolver(),
        );
        assert!(
            !out.body.is_empty() || name.contains("exponential"),
            "{name}"
        );
    }
}

/// The minimum of up to seven runs of `semantics()` on `text`, parsed once
/// outside the clock. Stops early once a run passes two seconds: a family
/// that slow has failed the gate whatever the minimum is.
#[cfg(not(debug_assertions))]
fn time_semantics(text: &str) -> Duration {
    let doc = parse(text, &ParseOptions::default());
    let mut best = Duration::MAX;
    for _ in 0..7 {
        let t = Instant::now();
        let s = semantics(&doc);
        let d = t.elapsed();
        std::hint::black_box(s);
        best = best.min(d);
        if d > Duration::from_secs(2) {
            break;
        }
    }
    best
}

/// Parse and semantics families in one test, one after the other, so no
/// two timings ever share the machine.
#[cfg(not(debug_assertions))]
#[test]
fn parse_and_semantics_time_grow_roughly_linearly() {
    let mut failures = Vec::new();
    let parse_families = FAMILIES.iter().map(|f| (f, time as fn(&str) -> Duration));
    let sem_families = SEMANTIC_FAMILIES
        .iter()
        .map(|f| (f, time_semantics as fn(&str) -> Duration));
    let render_families = RENDER_FAMILIES
        .iter()
        .map(|f| (f, time_render as fn(&str) -> Duration));
    let splice_families = SPLICE_FAMILIES
        .iter()
        .map(|f| (f, time_splice as fn(&str) -> Duration));
    let families = parse_families
        .chain(sem_families)
        .chain(render_families)
        .chain(splice_families);
    for ((name, gen, base), time) in families {
        let (small, large) = (gen(*base), gen(base * 8));
        let (t1, t8) = (time(&small), time(&large));
        let bytes = large.len() as f64 / small.len() as f64;
        let ratio = t8.as_secs_f64() / t1.as_secs_f64().max(1e-6) * 8.0 / bytes;
        eprintln!(
            "//! - {name}: {:.2} ms -> {:.2} ms for x{bytes:.0} bytes, normalised x{ratio:.1}",
            t1.as_secs_f64() * 1e3,
            t8.as_secs_f64() * 1e3
        );
        // Below ~0.3 ms timer noise dominates; such a family is not a risk.
        if ratio >= 24.0 && t8 > Duration::from_micros(300) {
            failures.push(format!("{name}: x{ratio:.1}"));
        }
    }
    assert!(failures.is_empty(), "super-linear growth: {failures:?}");
}
