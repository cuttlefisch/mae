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
//! Measured on the development machine (2026-10-31), peak heap at N -> 8N:
//! see `peak_heap_grows_linearly`'s output (`--nocapture`).

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

type Family = (&'static str, fn(usize) -> String, usize);

const FAMILIES: &[Family] = &[
    ("nested radio targets", nested_radio_targets, 4_000),
    ("blank-start radio target", blank_start_target, 4_000),
    ("nested tab radio targets", nested_tab_targets, 4_000),
    ("nested mixed radio targets", nested_mixed_targets, 4_000),
    ("plain words", |n| "word ".repeat(n), 4_000),
];

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
    }
    assert!(
        failures.is_empty(),
        "super-linear peak memory: {failures:?}"
    );
}
