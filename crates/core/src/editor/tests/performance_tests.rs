//! Performance, degradation, and cache tests.

use super::*;

#[test]
fn should_degrade_features_small_buffer() {
    let ed = Editor::new();
    assert!(
        !ed.should_degrade_features(0),
        "empty buffer should not degrade"
    );
}

#[test]
fn should_degrade_features_large_buffer() {
    let mut ed = Editor::new();
    // Insert > 500K chars
    let text = "a".repeat(600_000);
    ed.buffers[0].insert_text_at(0, &text);
    assert!(
        ed.should_degrade_features(0),
        "600K char buffer should degrade"
    );
}

#[test]
fn should_degrade_features_long_line() {
    let mut ed = Editor::new();
    // Insert a line > 10K chars (small total chars)
    let text = "x".repeat(15_000);
    ed.buffers[0].insert_text_at(0, &text);
    assert!(
        ed.should_degrade_features(0),
        "15K char line should degrade"
    );
}

#[test]
fn should_degrade_features_normal_file() {
    let mut ed = Editor::new();
    // 1000 lines x 80 chars = 80K chars, max line 80 chars
    let text: String = (0..1000)
        .map(|i| format!("Line {:04}: {}\n", i, "x".repeat(70)))
        .collect();
    ed.buffers[0].insert_text_at(0, &text);
    assert!(
        !ed.should_degrade_features(0),
        "80K normal file should not degrade"
    );
}

#[test]
fn fold_end_at_basic() {
    let mut ed = Editor::new();
    ed.buffers[0].insert_text_at(0, "a\nb\nc\nd\ne\n");
    ed.buffers[0].folded_ranges.push((1, 4));
    assert_eq!(ed.buffers[0].fold_end_at(1), Some(4));
    assert_eq!(ed.buffers[0].fold_end_at(0), None);
    assert_eq!(ed.buffers[0].fold_end_at(2), None);
}

#[test]
fn code_block_cache_populated_after_set() {
    let mut ed = Editor::new();
    ed.buffers[0].insert_text_at(0, "```rust\nfn main() {}\n```\n");
    ed.buffers[0].set_file_path(std::path::PathBuf::from("test.md"));
    ed.syntax.set_language(0, crate::syntax::Language::Markdown);
    let flavor = ed.effective_markup_flavor(0);
    let gen = ed.buffers[0].generation;
    let lines = crate::detect_code_block_lines(&ed.buffers[0], flavor);
    ed.code_block_cache.insert(
        0,
        crate::syntax::ViewportCodeBlockCache {
            generation: gen,
            flavor,
            line_start: 0,
            line_end: ed.buffers[0].line_count(),
            lines: lines.clone(),
        },
    );
    let cached = ed.code_block_cache.get(&0).unwrap();
    assert_eq!(cached.generation, gen);
    assert_eq!(cached.lines, lines);
}

#[test]
fn viewport_local_markup_spans_match_full_buffer() {
    let mut ed = Editor::new();
    let text = "* Heading\n\nSome *bold* text.\n\n#+begin_src rust\nfn main() {}\n#+end_src\n\nMore /italic/ text.\n";
    ed.buffers[0].insert_text_at(0, text);
    let flavor = crate::syntax::MarkupFlavor::Org;
    // Full-buffer spans.
    let source: String = ed.buffers[0].rope().chars().collect();
    let full_spans = crate::compute_markup_spans(&source, flavor);
    // Viewport-local spans covering the same range.
    let rope = ed.buffers[0].rope().clone();
    let line_count = rope.len_lines();
    let (_, local_spans) = crate::compute_markup_spans_for_range(&rope, flavor, 0, line_count);
    assert_eq!(full_spans.len(), local_spans.len());
    for (f, l) in full_spans.iter().zip(local_spans.iter()) {
        assert_eq!(f.byte_start, l.byte_start);
        assert_eq!(f.byte_end, l.byte_end);
        assert_eq!(f.theme_key, l.theme_key);
    }
}

#[test]
fn viewport_local_code_blocks_match_full_buffer() {
    let mut ed = Editor::new();
    let text = "Line 1\n```rust\nfn main() {}\n```\nLine 5\n```\nmore code\n```\nLine 9\n";
    ed.buffers[0].insert_text_at(0, text);
    let flavor = crate::syntax::MarkupFlavor::Markdown;
    let full = crate::detect_code_block_lines(&ed.buffers[0], flavor);
    // Viewport-local for middle range (lines 2..7).
    let local = crate::detect_code_block_lines_for_range(&ed.buffers[0], flavor, 2, 7);
    assert_eq!(local.len(), 5);
    for (rel_idx, &flag) in local.iter().enumerate() {
        assert_eq!(flag, full[2 + rel_idx], "mismatch at line {}", 2 + rel_idx);
    }
}

#[test]
fn viewport_local_code_blocks_backward_scan() {
    let mut ed = Editor::new();
    // Code block starts at line 1, continues through line 3.
    let text = "Line 0\n#+begin_src rust\nfn foo() {}\n#+end_src\nLine 4\n";
    ed.buffers[0].insert_text_at(0, text);
    let flavor = crate::syntax::MarkupFlavor::Org;
    // Request only lines 2..4 — backward scan must detect we're inside a code block.
    let local = crate::detect_code_block_lines_for_range(&ed.buffers[0], flavor, 2, 4);
    assert_eq!(local.len(), 2);
    assert!(local[0], "line 2 should be inside code block");
    assert!(
        local[1],
        "line 3 (#+end_src) should be marked as code block"
    );
}

#[test]
fn markup_cache_covers_method() {
    let cache = crate::syntax::MarkupCache {
        generation: 5,
        flavor: crate::syntax::MarkupFlavor::Org,
        line_start: 100,
        line_end: 400,
        byte_offset: 0,
        spans: vec![],
    };
    assert!(cache.covers(5, crate::syntax::MarkupFlavor::Org, 150, 350));
    assert!(cache.covers(5, crate::syntax::MarkupFlavor::Org, 100, 400));
    assert!(!cache.covers(5, crate::syntax::MarkupFlavor::Org, 50, 200));
    assert!(!cache.covers(5, crate::syntax::MarkupFlavor::Org, 300, 500));
    assert!(!cache.covers(6, crate::syntax::MarkupFlavor::Org, 150, 350));
    assert!(!cache.covers(5, crate::syntax::MarkupFlavor::Markdown, 150, 350));
}

#[test]
fn viewport_local_syntax_spans() {
    use crate::syntax::SyntaxMap;
    let mut sm = SyntaxMap::new();
    sm.set_language(0, crate::syntax::Language::Rust);

    let source = "fn main() {\n    let x = 1;\n    let y = 2;\n}\nfn foo() {}\n";
    let rope = ropey::Rope::from_str(source);
    let gen = 1;

    // Full-buffer parse
    let spans_full = sm.spans_for(0, source, gen).map(|s| s.to_vec());
    assert!(spans_full.is_some());

    // Reset and do viewport-local parse for lines 0..3
    sm.set_language(0, crate::syntax::Language::Rust);
    let spans_vp = sm
        .spans_for_viewport(0, &rope, gen, 0, 3)
        .map(|s| s.to_vec());
    assert!(spans_vp.is_some());

    // Viewport spans should cover byte range of lines 0..padded_end.
    // With padding (range_size/3 = 1), padded_end = min(3+1, 5) = 4.
    let padded_end = (3 + (3 / 3)).min(rope.len_lines());
    let byte_end_padded = rope.line_to_byte(padded_end);
    let vp_spans = spans_vp.unwrap();
    assert!(
        vp_spans.iter().all(|s| s.byte_end <= byte_end_padded),
        "viewport spans should be within padded range 0..{padded_end}"
    );
}

#[test]
fn viewport_covers_tracks_range() {
    use crate::syntax::SyntaxMap;
    let mut sm = SyntaxMap::new();
    sm.set_language(0, crate::syntax::Language::Rust);

    let source = "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\nfn e() {}\n";
    let rope = ropey::Rope::from_str(source);

    sm.spans_for_viewport(0, &rope, 1, 1, 3);
    assert!(sm.viewport_covers(0, 1, 3));
    assert!(!sm.viewport_covers(0, 0, 3)); // 0 < viewport_line_start=1
    assert!(!sm.viewport_covers(0, 1, 5)); // 5 > viewport_line_end=3
}

// --- Visual rows cache separation tests ---

#[test]
fn visual_rows_cache_survives_scroll_shift() {
    let mut editor = Editor::new();
    let idx = editor.active_buffer_idx();
    // Insert 200 lines.
    let content: String = (0..200).map(|i| format!("line {i}\n")).collect();
    editor.buffers[idx].insert_text_at(0, &content);
    editor.viewport_height = 50;
    editor.word_wrap = true;
    editor.text_area_width = 80;
    // Initial populate.
    editor.populate_visual_rows_cache(idx, 10, 60);
    assert!(editor.buffers[idx].visual_rows_cache.is_some());
    let gen1 = editor.buffers[idx]
        .visual_rows_cache
        .as_ref()
        .unwrap()
        .line_start;
    // Shift by 1 — should NOT recompute (padding absorbs it).
    editor.populate_visual_rows_cache(idx, 11, 61);
    let gen2 = editor.buffers[idx]
        .visual_rows_cache
        .as_ref()
        .unwrap()
        .line_start;
    assert_eq!(gen1, gen2, "cache should survive single-line shift");
}

#[test]
fn visual_rows_cache_recomputes_on_large_shift() {
    let mut editor = Editor::new();
    let idx = editor.active_buffer_idx();
    let content: String = (0..200).map(|i| format!("line {i}\n")).collect();
    editor.buffers[idx].insert_text_at(0, &content);
    editor.viewport_height = 50;
    editor.word_wrap = true;
    editor.text_area_width = 80;
    editor.populate_visual_rows_cache(idx, 10, 60);
    // Jump 100 lines — must recompute.
    editor.populate_visual_rows_cache(idx, 110, 160);
    let cache = editor.buffers[idx].visual_rows_cache.as_ref().unwrap();
    assert!(cache.line_start <= 110, "cache must cover needed_start=110");
    assert!(
        cache.line_start + cache.rows.len() >= 160,
        "cache must cover needed_end=160"
    );
}

#[test]
fn visual_rows_cache_invalidates_on_width_change() {
    let mut editor = Editor::new();
    let idx = editor.active_buffer_idx();
    let content: String = (0..100).map(|i| format!("line {i}\n")).collect();
    editor.buffers[idx].insert_text_at(0, &content);
    editor.viewport_height = 50;
    editor.word_wrap = true;
    editor.text_area_width = 80;
    editor.populate_visual_rows_cache(idx, 10, 60);
    let w1 = editor.buffers[idx]
        .visual_rows_cache
        .as_ref()
        .unwrap()
        .text_width;
    // Change width and repopulate.
    editor.text_area_width = 60;
    editor.populate_visual_rows_cache(idx, 10, 60);
    let w2 = editor.buffers[idx]
        .visual_rows_cache
        .as_ref()
        .unwrap()
        .text_width;
    assert_ne!(w1, w2, "cache must recompute on width change");
}
