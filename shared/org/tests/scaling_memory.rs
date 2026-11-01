//! Gate 3, memory (ADR-112 D5): hostile inputs that grow 8x must not make
//! the parse's PEAK heap grow more than linearly. A counting global
//! allocator (this test binary only) records the high-water mark of live
//! bytes during one parse. With B the byte ratio of the two inputs, the
//! bound is peak(8N)/peak(N) / B < 2: linear gives ~1, and memory that
//! grows as S^1.5 (the review's radio-hit list: 193 KB took 780 MB, 1 MB
//! would take ~9 GB) gives ~2.8 per 8x, with every doubling after that worse.
//! The inputs stay small enough that a regression shows here as a failed
//! ratio, not as an OOM kill of the test process.
//!
//! A ratio alone passes memory that is linear with a huge constant: the
//! review's mixed-target families were x1.0 here at 540-650 B per input
//! byte (2.4 MB of input took 1.33 GB). So every family is also held to an
//! ABSOLUTE ceiling, [`MAX_BYTES_PER_BYTE`], at both sizes.
//!
//! Measured on the development machine (2026-11-01, release, after the fix
//! round), peak heap at N -> 8N and bytes per input byte at 8N (the radio
//! families failed before the linear matcher: nested targets 18.6 MB ->
//! 303 MB for 12 KB -> 94 KB, normalised x2.08; the mixed-target ones
//! before the fix round: case variants 540, nested blank-start 650 and
//! 515, distinct targets on one node 736 B/byte):
//! - nested radio targets: 697 KB -> 5434 KB for 12 KB -> 94 KB, x1.00, 57 B/byte
//! - blank-start radio target: 239 KB -> 1891 KB for 7 KB -> 62 KB, x0.99, 30 B/byte
//! - nested tab radio targets: 697 KB -> 5434 KB for 12 KB -> 94 KB, x1.00, 57 B/byte
//! - nested mixed radio targets: 838 KB -> 6527 KB for 17 KB -> 141 KB, x0.99, 46 B/byte
//! - plain words: 85 KB -> 669 KB for 19 KB -> 156 KB, x0.98, 4 B/byte
//! - case-variant blank-start mixed targets: 1827 KB -> 14597 KB for 37 KB -> 301 KB, x1.00, 48 B/byte
//! - nested blank-start mixed targets, lead fails: 959 KB -> 7387 KB for 16 KB -> 126 KB, x0.99, 58 B/byte
//! - nested blank-start mixed targets, lead matches: 831 KB -> 6363 KB for 20 KB -> 157 KB, x0.98, 40 B/byte
//! - distinct mixed targets on one node: 1491 KB -> 20441 KB for 27 KB -> 395 KB, x0.96, 52 B/byte
//! - target of N distinct ascending chars: 1033 KB -> 8251 KB for 23 KB -> 187 KB, x1.00, 44 B/byte
//! - split radio regexps (glossary): 7429 KB -> 59907 KB for 100 KB -> 841 KB, x0.96, 71 B/byte
//! - realistic document: 1625 KB -> 12978 KB for 44 KB -> 363 KB, x0.98, 36 B/byte

use mae_org::{parse, ParseOptions};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged to the system allocator.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was allocated by `alloc` above with this layout.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Peak live bytes above the baseline while parsing `text`.
fn peak(text: &str) -> usize {
    let opts = ParseOptions::default();
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let doc = parse(text, &opts);
    let p = PEAK.load(Ordering::Relaxed) - base;
    assert_eq!(doc.syntax().text_range().len(), (text.len() as u32).into());
    drop(doc);
    p
}

/// Radio targets `a`, `a a`, … (k ≈ √n of them), then `a `×n: every word
/// starts a match of every target (review finding 3).
fn nested_radio_targets(n: usize) -> String {
    let k = (n as f64).sqrt() as usize;
    let mut s = String::new();
    for j in 1..=k {
        s.push_str(&format!("<<<{}>>>\n", vec!["a"; j].join(" ")));
    }
    s.push('\n');
    s.push_str(&"a ".repeat(n));
    s.push('\n');
    s
}

/// A radio target that starts with a no-break space, then a long run of
/// them before text that fails the target (review finding 2).
fn blank_start_target(n: usize) -> String {
    format!("<<<\u{a0} x>>>\n\n{}xa\n", "\u{a0}".repeat(n))
}

/// The same with tabs between nested targets: non-simple targets.
fn nested_tab_targets(n: usize) -> String {
    let k = (n as f64).sqrt() as usize;
    let mut s = String::new();
    for j in 1..=k {
        s.push_str(&format!("<<<{}>>>\n", vec!["a"; j].join("\t")));
    }
    s.push('\n');
    s.push_str(&"a\t".repeat(n));
    s.push('\n');
    s
}

/// A tab next to a space in every stretch: mixed targets, whose exact
/// matches remember their failed states (within the work budget).
fn nested_mixed_targets(n: usize) -> String {
    let k = (n as f64).sqrt() as usize;
    let mut s = String::new();
    for j in 1..=k {
        s.push_str(&format!("<<<{}>>>\n", vec!["a"; j].join("\t ")));
    }
    s.push('\n');
    s.push_str(&"a\t ".repeat(n));
    s.push('\n');
    s
}

/// Review F2: many case variants of one blank-start mixed target on one
/// trie node, then a long text of near-misses (2.4 MB took 1.33 GB).
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

/// Review F2: nested blank-start mixed targets (4.1 MB took 2.6 GB).
fn nested_blank_start(n: usize, unit: &str) -> String {
    let k = (n as f64).sqrt() as usize;
    let targets: String = (1..=k)
        .map(|j| format!("<<<\u{a0}{}>>>\n", " a".repeat(j)))
        .collect();
    format!("{targets}\n{}\n", unit.repeat(n))
}

/// Distinct mixed targets on one node over text holding tabs.
fn distinct_mixed_on_one_node(n: usize) -> String {
    let targets: String = (3..3 + n / 50)
        .map(|i| format!("<<<x {}y>>>\n", "\t".repeat(i)))
        .collect();
    format!("{targets}\n{}\n", "x \t\ty ".repeat(n))
}

/// One target of N distinct chars in ascending order.
fn distinct_char_target(n: usize) -> String {
    let t: String = (0..n as u32)
        .filter_map(|i| char::from_u32(0x4E00 + i))
        .collect();
    format!("<<<{t}>>>\n\n{t}\n")
}

/// N glossary terms (past Org's regexp limit, so split), then text.
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

/// A realistic document: headings, lists, links, emphasis, a table.
fn realistic(n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "* Heading {i} :tag:\n:PROPERTIES:\n:ID: id-{i}\n:END:\n\
                 Some *bold* and /italic/ text with [[id:id-{}][a link]] and\n\
                 https://example.org/{i}, a footnote[fn:{i}].\n\
                 - item one\n- item =two=\n| a | b |\n|---+---|\n| 1 | 2 |\n\n\
                 [fn:{i}] The note.\n",
                i / 2
            )
        })
        .collect()
}

type Family = (&'static str, fn(usize) -> String, usize);

const FAMILIES: &[Family] = &[
    ("nested radio targets", nested_radio_targets, 4_000),
    ("blank-start radio target", blank_start_target, 4_000),
    ("nested tab radio targets", nested_tab_targets, 4_000),
    ("nested mixed radio targets", nested_mixed_targets, 4_000),
    ("plain words", |n| "word ".repeat(n), 4_000),
    (
        "case-variant blank-start mixed targets",
        case_variant_targets,
        4_000,
    ),
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
    (
        "distinct mixed targets on one node",
        distinct_mixed_on_one_node,
        4_000,
    ),
    (
        "target of N distinct ascending chars",
        distinct_char_target,
        4_000,
    ),
    ("split radio regexps (glossary)", glossary, 2_000),
    ("realistic document", realistic, 200),
];

/// The absolute ceiling on peak heap per input byte, for every family at
/// every size measured. A realistic document measures about 33 B/byte
/// (the tree, its index and the inline tables); the radio matcher's own
/// tables add about 20 B per text byte at most, and its memo of failed
/// states is capped at one entry per two text bytes. A ratio gate alone
/// would pass memory that is linear with a huge constant (the review's
/// 560-650 B/byte); this bounds the constant.
const MAX_BYTES_PER_BYTE: f64 = 100.0;

#[test]
fn peak_heap_grows_linearly() {
    let mut failures = Vec::new();
    for (name, gen, base) in FAMILIES {
        let (small, large) = (gen(*base), gen(base * 8));
        let (p1, p8) = (peak(&small), peak(&large));
        let bytes = large.len() as f64 / small.len() as f64;
        let ratio = p8 as f64 / p1.max(1) as f64 / bytes;
        eprintln!(
            "//! - {name}: {} KB -> {} KB peak for {} KB -> {} KB input, normalised x{ratio:.2}",
            p1 / 1024,
            p8 / 1024,
            small.len() / 1024,
            large.len() / 1024
        );
        if ratio >= 2.0 {
            failures.push(format!("{name}: x{ratio:.2}"));
        }
        for (peak, input) in [(p1, small.len()), (p8, large.len())] {
            let per_byte = peak as f64 / input as f64;
            eprintln!(
                "//!   {name}: {per_byte:.1} B per input byte at {} KB",
                input / 1024
            );
            if per_byte >= MAX_BYTES_PER_BYTE {
                failures.push(format!(
                    "{name}: {per_byte:.0} B/byte at {} KB",
                    input / 1024
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "super-linear or oversized peak memory: {failures:?}"
    );
}
