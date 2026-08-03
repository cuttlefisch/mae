//! ADR-087 Rule 4 — `Window::cursor_col` is a byte offset.
//!
//! These tests exist to *falsify* the migration (CLAUDE.md principle #14), so
//! the oracle is never the stored number. Every assertion is expressed in
//! terms of what the user can see: which grapheme cluster the cursor sits on,
//! what display column it occupies, what text a selection covers, what the
//! buffer contains after an edit. A test that asserted `cursor_col == 3` would
//! pass equally well under a broken char-domain implementation on ASCII and
//! prove nothing.

use super::*;
use crate::grapheme::WidthPolicy;
use crate::VisualType;

/// The named nasty-string corpus ADR-087 enforcement item 4 calls for.
/// Hand-curated rather than generated: proptest's `String` generator
/// structurally cannot emit ZWJ sequences (category Cf is excluded), and a
/// named case reports *what* broke.
fn corpus() -> Vec<(&'static str, &'static str)> {
    vec![
        // Multi-byte but one char per cluster — byte/char disagree 3:1.
        ("cjk", "日本語のテキスト"),
        // One cluster, two chars: base + combining acute.
        ("combining", "cafe\u{0301} nai\u{0308}ve"),
        // One cluster, seven chars, 25 bytes.
        ("zwj-family", "a👨\u{200D}👩\u{200D}👧\u{200D}👦b"),
        // One cluster, two chars (base + modifier).
        ("skin-tone", "x👍\u{1F3FD}y"),
        // One cluster per flag, two chars each.
        ("regional-flags", "🇯🇵🇺🇸"),
        // Astral CJK: 4 bytes, 1 char, width 2.
        ("astral-cjk", "\u{20000}\u{2A6B2}"),
        // Devanagari virama cluster.
        ("virama", "\u{0915}\u{094D}\u{0937}a"),
        // Mixed, so a fix that only handles one shape is caught.
        ("mixed", "a日👨\u{200D}👩\u{200D}👧b\u{0301}c"),
    ]
}

/// Byte offsets of every grapheme-cluster boundary in `s`, including `s.len()`.
fn cluster_boundaries(s: &str) -> Vec<usize> {
    let mut out = vec![];
    let mut i = 0;
    while i < s.len() {
        out.push(i);
        i = crate::grapheme::next_grapheme_boundary(s, i);
    }
    out.push(s.len());
    out
}

/// The cluster the cursor is sitting on — the user-visible oracle.
fn cluster_at(editor: &Editor) -> String {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];
    let line = buf.line_text_no_newline(win.cursor_row);
    if win.cursor_col >= line.len() {
        return String::new();
    }
    let end = crate::grapheme::next_grapheme_boundary(&line, win.cursor_col);
    line[win.cursor_col..end].to_string()
}

fn col(editor: &Editor) -> usize {
    editor.window_mgr.focused_window().cursor_col
}

fn set_col(editor: &mut Editor, c: usize) {
    editor.window_mgr.focused_window_mut().cursor_col = c;
}

// -------------------------------------------------------------------------
// The invariant: a cursor column never splits a cluster, whatever you do.
// -------------------------------------------------------------------------

#[test]
fn every_cursor_column_the_editor_produces_is_a_grapheme_boundary() {
    let motions = [
        "move-right",
        "move-left",
        "move-to-line-end",
        "move-to-line-start",
        "move-word-forward",
        "move-word-backward",
        "move-word-end",
        "move-to-first-non-blank",
    ];
    for (name, text) in corpus() {
        for motion in motions {
            let mut editor = editor_with_text(text);
            let valid = cluster_boundaries(text);
            // Run the motion far more times than there are clusters, so the
            // ends of the line are exercised too.
            for step in 0..(text.len() + 4) {
                editor.dispatch_builtin(motion);
                assert!(
                    valid.contains(&col(&editor)),
                    "{name}/{motion}: step {step} left cursor_col at {} — not a \
                     grapheme boundary (valid: {valid:?})",
                    col(&editor)
                );
            }
        }
    }
}

#[test]
fn move_right_visits_every_cluster_exactly_once_then_stops() {
    for (name, text) in corpus() {
        let mut editor = editor_with_text(text);
        let expected: Vec<String> = text
            .char_indices()
            .fold(Vec::<usize>::new(), |mut acc, (i, _)| {
                if acc.last().is_none_or(|&last| {
                    crate::grapheme::next_grapheme_boundary(text, last) <= i
                }) && crate::grapheme::snap_to_grapheme_boundary(text, i) == i
                {
                    acc.push(i);
                }
                acc
            })
            .iter()
            .map(|&b| {
                let e = crate::grapheme::next_grapheme_boundary(text, b);
                text[b..e].to_string()
            })
            .collect();

        let mut seen = vec![cluster_at(&editor)];
        for _ in 0..expected.len() + 3 {
            editor.dispatch_builtin("move-right");
            let c = cluster_at(&editor);
            if c.is_empty() {
                break;
            }
            if seen.last() != Some(&c) {
                seen.push(c);
            }
        }
        assert_eq!(
            seen, expected,
            "{name}: move-right did not walk the clusters in order"
        );
    }
}

#[test]
fn left_then_right_returns_to_the_same_cluster() {
    // Round-trip property: for every reachable position, one step out and
    // back must land on the same visible character.
    for (name, text) in corpus() {
        for &start in &cluster_boundaries(text) {
            let mut editor = editor_with_text(text);
            set_col(&mut editor, start);
            let before = cluster_at(&editor);
            let before_col = col(&editor);
            editor.dispatch_builtin("move-right");
            // At end-of-line move-right is a no-op; move-left would then walk
            // backwards, so the round trip is only meaningful when it moved.
            if col(&editor) == before_col {
                continue;
            }
            editor.dispatch_builtin("move-left");
            assert_eq!(
                col(&editor),
                before_col,
                "{name}: right-then-left from byte {start} did not return"
            );
            assert_eq!(
                cluster_at(&editor),
                before,
                "{name}: right-then-left from byte {start} landed elsewhere"
            );
        }
    }
}

#[test]
fn a_mid_cluster_column_is_snapped_rather_than_slicing_utf8() {
    // The adversarial case: something upstream (a stale session, a protocol
    // message, arithmetic) hands the window a column inside a cluster. The
    // clamp chokepoint must repair it, and nothing may panic.
    for (name, text) in corpus() {
        for bad in 0..text.len() {
            let mut editor = editor_with_text(text);
            set_col(&mut editor, bad);
            let idx = editor.active_buffer_idx();
            let buf = &editor.buffers[idx];
            editor.window_mgr.focused_window_mut().clamp_cursor(buf);
            let c = col(&editor);
            assert!(
                cluster_boundaries(text).contains(&c),
                "{name}: column {bad} clamped to {c}, not a cluster boundary"
            );
            assert!(c <= bad, "{name}: clamp moved right from {bad} to {c}");
        }
    }
}

// -------------------------------------------------------------------------
// Editing: the buffer contents are the oracle.
// -------------------------------------------------------------------------

#[test]
fn backspace_at_each_cluster_removes_exactly_that_cluster() {
    for (name, text) in corpus() {
        for &start in cluster_boundaries(text).iter().filter(|&&b| b > 0) {
            let mut editor = editor_with_text(text);
            editor.set_mode(Mode::Insert);
            set_col(&mut editor, start);
            let idx = editor.active_buffer_idx();
            // One backspace deletes one *char*; a cluster may hold several.
            let prev = crate::grapheme::prev_grapheme_boundary(text, start);
            let chars_in_cluster = text[prev..start].chars().count();
            for _ in 0..chars_in_cluster {
                let win = editor.window_mgr.focused_window_mut();
                let buf = &mut editor.buffers[idx];
                buf.delete_char_backward(win);
            }
            let expected = format!("{}{}", &text[..prev], &text[start..]);
            assert_eq!(
                editor.buffers[idx].text().trim_end_matches('\n'),
                expected,
                "{name}: backspace over the cluster at {start} produced the wrong text"
            );
            assert_eq!(
                col(&editor),
                prev,
                "{name}: cursor did not land at the cluster start after backspace"
            );
        }
    }
}

#[test]
fn typing_a_multibyte_char_advances_the_cursor_by_its_utf8_length() {
    for (name, text) in corpus() {
        for ch in ['a', '日', '\u{20000}', 'é'] {
            let mut editor = editor_with_text(text);
            editor.set_mode(Mode::Insert);
            set_col(&mut editor, 0);
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].insert_char(win, ch);
            assert_eq!(
                col(&editor),
                ch.len_utf8(),
                "{name}: inserting {ch:?} advanced the byte column wrongly"
            );
            assert!(
                editor.buffers[idx].text().starts_with(ch),
                "{name}: inserted char is not at the start"
            );
        }
    }
}

#[test]
fn delete_char_forward_at_each_boundary_never_corrupts_the_text() {
    for (name, text) in corpus() {
        for &start in &cluster_boundaries(text) {
            let mut editor = editor_with_text(text);
            set_col(&mut editor, start);
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_char_forward(win);
            // The result must still be valid UTF-8 that round-trips, and the
            // cursor must still be on a boundary of the *new* text.
            let after = editor.buffers[idx].text();
            let line = after.trim_end_matches('\n');
            assert!(
                cluster_boundaries(line).contains(&col(&editor)),
                "{name}: after forward-delete at {start}, cursor {} is mid-cluster",
                col(&editor)
            );
        }
    }
}

// -------------------------------------------------------------------------
// Selection.
// -------------------------------------------------------------------------

#[test]
fn a_visual_selection_covers_the_clusters_it_visibly_spans() {
    for (name, text) in corpus() {
        let bounds = cluster_boundaries(text);
        if bounds.len() < 3 {
            continue;
        }
        for i in 0..bounds.len() - 1 {
            for j in i..bounds.len() - 1 {
                let mut editor = editor_with_text(text);
                set_col(&mut editor, bounds[i]);
                editor.set_mode(Mode::Visual(VisualType::Char));
                editor.vi.visual_anchor_row = 0;
                editor.vi.visual_anchor_col = bounds[i];
                set_col(&mut editor, bounds[j]);
                let (start_off, end_off) = editor.visual_selection_range();
                let idx = editor.active_buffer_idx();
                let selected = editor.buffers[idx].text_range(start_off, end_off);
                // vi's charwise visual is inclusive of the cursor cluster.
                let end = crate::grapheme::next_grapheme_boundary(text, bounds[j]);
                assert_eq!(
                    selected,
                    text[bounds[i]..end],
                    "{name}: selection {i}..{j} covered the wrong text"
                );
            }
        }
    }
}

// -------------------------------------------------------------------------
// Screen position: the cursor must land where the glyph is drawn.
// -------------------------------------------------------------------------

#[test]
fn the_cursor_screen_column_equals_the_width_of_the_text_before_it() {
    for (name, text) in corpus() {
        let mut editor = editor_with_text(text);
        let policy = editor.width_policy();
        for &b in &cluster_boundaries(text) {
            set_col(&mut editor, b);
            let idx = editor.active_buffer_idx();
            let screen = editor.buffers[idx].display_col_for_byte_col(0, b, policy);
            let expected = crate::grapheme::display_width_with(&text[..b], policy);
            assert_eq!(
                screen, expected,
                "{name}: byte col {b} maps to screen col {screen}, expected {expected}"
            );
        }
    }
}

#[test]
fn a_click_on_a_wide_glyph_selects_that_glyph_not_its_neighbour() {
    // CJK is two cells wide: clicking either cell must resolve to the same
    // character. The pre-migration `screen_col.min(line_len_in_chars)` idiom
    // resolved the right-hand cell to the *next* character.
    let text = "日本語abc";
    let editor = editor_with_text(text);
    let policy = editor.width_policy();
    let idx = editor.active_buffer_idx();
    let buf = &editor.buffers[idx];
    for (display_col, expected_cluster) in [
        (0, "日"),
        (1, "日"),
        (2, "本"),
        (3, "本"),
        (4, "語"),
        (5, "語"),
        (6, "a"),
        (7, "b"),
        (8, "c"),
    ] {
        let byte_col = buf.byte_col_for_display_col(0, display_col, policy);
        let end = crate::grapheme::next_grapheme_boundary(text, byte_col);
        assert_eq!(
            &text[byte_col..end],
            expected_cluster,
            "display col {display_col} resolved to the wrong cluster"
        );
    }
}

// -------------------------------------------------------------------------
// Rule 3: the width options must reach more than the status bar.
// -------------------------------------------------------------------------

#[test]
fn ambiguous_width_wide_changes_the_cursor_screen_column() {
    // ADR-087 follow-up (b): an East-Asian user setting `ambiguous_width=wide`
    // must see it everywhere, not only in the status bar.
    let text = "→→→abc";
    let mut editor = editor_with_text(text);
    let idx = editor.active_buffer_idx();

    editor.set_option("ambiguous_width", "narrow").unwrap();
    let narrow = editor.buffers[idx].display_col_for_byte_col(0, 9, editor.width_policy());
    editor.set_option("ambiguous_width", "wide").unwrap();
    let wide = editor.buffers[idx].display_col_for_byte_col(0, 9, editor.width_policy());

    assert_eq!(narrow, 3, "U+2192 is 1 cell under the narrow policy");
    assert_eq!(wide, 6, "U+2192 is 2 cells under the CJK policy");
    assert_ne!(
        narrow, wide,
        "the option must actually reach cursor positioning"
    );
}

#[test]
fn ambiguous_width_wide_changes_word_wrap_layout() {
    let mut editor = editor_with_text("→→→→→→→→→→");
    editor.set_option("ambiguous_width", "narrow").unwrap();
    let narrow_rows =
        crate::wrap::wrap_line_display_rows("→→→→→→→→→→", 10, false, 0, editor.width_policy());
    editor.set_option("ambiguous_width", "wide").unwrap();
    let wide_rows =
        crate::wrap::wrap_line_display_rows("→→→→→→→→→→", 10, false, 0, editor.width_policy());
    assert_eq!(narrow_rows, 1);
    assert_eq!(wide_rows, 2, "wide ambiguous width must reach word wrap");
}

#[test]
fn the_width_policy_is_read_from_options_not_hardcoded() {
    // Guards against a renderer that takes a `WidthPolicy` parameter but is
    // handed `WidthPolicy::default()` at the call site.
    let mut editor = editor_with_text("x");
    assert_eq!(editor.width_policy(), WidthPolicy::default());
    editor.set_option("ambiguous_width", "wide").unwrap();
    assert_ne!(
        editor.width_policy(),
        WidthPolicy::default(),
        "editor.width_policy() must track the live option"
    );
    editor.set_option("control_char_width", "1").unwrap();
    assert_eq!(editor.width_policy().control_char_width, 1);
}
