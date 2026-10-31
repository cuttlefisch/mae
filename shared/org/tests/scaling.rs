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
//! Measured on the development machine (Linux x86_64, release, 2026-10-31),
//! t(N) → t(8N), the byte ratio, and the normalised ratio (three runs agreed
//! within ±1.5; "blank lines" is linear per doubling above 100k lines, its
//! excess at this size is allocation, not complexity):
//! - *a x N (one line): 0.11 ms -> 0.90 ms for x8 bytes, normalised x7.9
//! - [[ x N: 0.17 ms -> 1.01 ms for x8 bytes, normalised x5.9
//! - nested same-name quotes: 0.75 ms -> 4.41 ms for x8 bytes, normalised x5.9
//! - nested distinct blocks (capped): 0.46 ms -> 3.34 ms for x9 bytes, normalised x6.9
//! - deep list: 0.15 ms -> 4.45 ms for x62 bytes, normalised x3.9
//! - one long line of words: 0.04 ms -> 0.34 ms for x8 bytes, normalised x7.9
//! - headings: 0.59 ms -> 5.22 ms for x9 bytes, normalised x8.1
//! - affiliated orphans: 1.46 ms -> 13.53 ms for x8 bytes, normalised x9.3
//! - table.el near-misses: 3.96 ms -> 38.32 ms for x8 bytes, normalised x9.7
//! - unterminated begins: 0.68 ms -> 5.75 ms for x8 bytes, normalised x8.4
//! - property-like lines: 0.54 ms -> 4.45 ms for x8 bytes, normalised x8.2
//! - drawer openers: 0.45 ms -> 3.84 ms for x8 bytes, normalised x8.5
//! - footnotes: 1.64 ms -> 15.35 ms for x8 bytes, normalised x9.3
//! - blank lines: 1.29 ms -> 17.41 ms for x8 bytes, normalised x13.5

use mae_org::{parse, ParseOptions};
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
];

fn nested_blocks(n: usize) -> String {
    let open: String = (0..n).map(|i| format!("#+begin_b{i}\n")).collect();
    let close: String = (0..n).rev().map(|i| format!("#+end_b{i}\n")).collect();
    format!("{open}core\n{close}")
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
}

#[cfg(not(debug_assertions))]
#[test]
fn parse_time_grows_roughly_linearly() {
    let mut failures = Vec::new();
    for (name, gen, base) in FAMILIES {
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
