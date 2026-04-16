use std::collections::HashMap;

use crate::keymap::{parse_key_seq, parse_key_seq_spaced, Key, KeyPress, Keymap};

use super::Editor;

impl Editor {
    /// Create the default vi-like keymaps.
    pub(crate) fn default_keymaps() -> HashMap<String, Keymap> {
        let mut maps = HashMap::new();

        let mut normal = Keymap::new("normal");
        // Movement
        normal.bind(parse_key_seq("h"), "move-left");
        normal.bind(parse_key_seq("j"), "move-down");
        normal.bind(parse_key_seq("k"), "move-up");
        normal.bind(parse_key_seq("l"), "move-right");
        normal.bind(vec![KeyPress::special(Key::Left)], "move-left");
        normal.bind(vec![KeyPress::special(Key::Down)], "move-down");
        normal.bind(vec![KeyPress::special(Key::Up)], "move-up");
        normal.bind(vec![KeyPress::special(Key::Right)], "move-right");
        normal.bind(parse_key_seq("0"), "move-to-line-start");
        normal.bind(parse_key_seq("$"), "move-to-line-end");
        normal.bind(parse_key_seq("G"), "move-to-last-line");
        normal.bind(parse_key_seq("gg"), "move-to-first-line");
        // Word motions
        normal.bind(parse_key_seq("w"), "move-word-forward");
        normal.bind(parse_key_seq("b"), "move-word-backward");
        normal.bind(parse_key_seq("e"), "move-word-end");
        normal.bind(parse_key_seq("W"), "move-big-word-forward");
        normal.bind(parse_key_seq("B"), "move-big-word-backward");
        normal.bind(parse_key_seq("E"), "move-big-word-end");
        normal.bind(parse_key_seq("%"), "move-matching-bracket");
        normal.bind(parse_key_seq("{"), "move-paragraph-backward");
        normal.bind(parse_key_seq("}"), "move-paragraph-forward");
        normal.bind(parse_key_seq("f"), "find-char-forward-await");
        normal.bind(parse_key_seq("F"), "find-char-backward-await");
        normal.bind(parse_key_seq("t"), "till-char-forward-await");
        normal.bind(parse_key_seq("T"), "till-char-backward-await");
        // Scroll
        normal.bind(parse_key_seq("C-u"), "scroll-half-up");
        normal.bind(parse_key_seq("C-d"), "scroll-half-down");
        normal.bind(parse_key_seq("C-f"), "scroll-page-down");
        normal.bind(parse_key_seq("C-b"), "scroll-page-up");
        normal.bind(parse_key_seq("zz"), "scroll-center");
        normal.bind(parse_key_seq("zt"), "scroll-top");
        normal.bind(parse_key_seq("zb"), "scroll-bottom");
        // Screen-relative cursor
        normal.bind(parse_key_seq("H"), "move-screen-top");
        normal.bind(parse_key_seq("M"), "move-screen-middle");
        normal.bind(parse_key_seq("L"), "move-screen-bottom");
        // Search
        normal.bind(parse_key_seq("/"), "search-forward-start");
        normal.bind(parse_key_seq("?"), "search-backward-start");
        normal.bind(parse_key_seq("n"), "search-next");
        normal.bind(parse_key_seq("N"), "search-prev");
        normal.bind(parse_key_seq("*"), "search-word-under-cursor");
        // Editing
        normal.bind(parse_key_seq("x"), "delete-char-forward");
        normal.bind(parse_key_seq("dd"), "delete-line");
        normal.bind(parse_key_seq("dw"), "delete-word-forward");
        normal.bind(parse_key_seq("d$"), "delete-to-line-end");
        normal.bind(parse_key_seq("d0"), "delete-to-line-start");
        // Text object operators
        normal.bind(parse_key_seq("di"), "delete-inner-object");
        normal.bind(parse_key_seq("da"), "delete-around-object");
        normal.bind(parse_key_seq("ci"), "change-inner-object");
        normal.bind(parse_key_seq("ca"), "change-around-object");
        normal.bind(parse_key_seq("yi"), "yank-inner-object");
        normal.bind(parse_key_seq("ya"), "yank-around-object");
        // Change operators
        normal.bind(parse_key_seq("cc"), "change-line");
        normal.bind(parse_key_seq("cw"), "change-word-forward");
        normal.bind(parse_key_seq("c$"), "change-to-line-end");
        normal.bind(parse_key_seq("C"), "change-to-line-end");
        normal.bind(parse_key_seq("c0"), "change-to-line-start");
        // Replace
        normal.bind(parse_key_seq("r"), "replace-char-await");
        // Marks (m<letter> sets, '<letter> jumps)
        normal.bind(parse_key_seq("m"), "set-mark-await");
        normal.bind(parse_key_seq("'"), "jump-mark-await");
        // Macros (q<letter> records, @<letter> replays, @@ repeats last).
        // @@ is handled by dispatch_char_motion: if the register char is '@',
        // last_macro_register is used. This avoids the keymap prefix conflict
        // that would arise from binding both "@" (exact) and "@@" (longer).
        normal.bind(parse_key_seq("q"), "start-recording-await");
        normal.bind(parse_key_seq("@"), "replay-macro-await");
        // Join, indent, dedent
        normal.bind(parse_key_seq("J"), "join-lines");
        normal.bind(parse_key_seq(">>"), "indent-line");
        normal.bind(parse_key_seq("<<"), "dedent-line");
        // Case change
        normal.bind(parse_key_seq("~"), "toggle-case");
        normal.bind(parse_key_seq_spaced("g U U"), "uppercase-line");
        normal.bind(parse_key_seq_spaced("g u u"), "lowercase-line");
        // LSP navigation (Phase 4a M2)
        normal.bind(parse_key_seq("gd"), "lsp-goto-definition");
        normal.bind(parse_key_seq("gr"), "lsp-find-references");
        normal.bind(parse_key_seq("K"), "lsp-hover");
        // LSP diagnostics (Phase 4a M3)
        normal.bind(parse_key_seq("]d"), "lsp-next-diagnostic");
        normal.bind(parse_key_seq("[d"), "lsp-prev-diagnostic");
        // Alternate file
        normal.bind(vec![KeyPress::ctrl('6')], "alternate-file");
        // Dot repeat
        normal.bind(parse_key_seq("."), "dot-repeat");
        // Yank/Paste
        normal.bind(parse_key_seq("yy"), "yank-line");
        normal.bind(parse_key_seq("yw"), "yank-word-forward");
        normal.bind(parse_key_seq("y$"), "yank-to-line-end");
        normal.bind(parse_key_seq("y0"), "yank-to-line-start");
        normal.bind(parse_key_seq("p"), "paste-after");
        normal.bind(parse_key_seq("P"), "paste-before");
        // Undo/Redo
        normal.bind(parse_key_seq("u"), "undo");
        normal.bind(parse_key_seq("C-r"), "redo");
        // Mode changes
        normal.bind(parse_key_seq("i"), "enter-insert-mode");
        normal.bind(parse_key_seq("a"), "enter-insert-mode-after");
        normal.bind(parse_key_seq("A"), "enter-insert-mode-eol");
        normal.bind(parse_key_seq("o"), "open-line-below");
        normal.bind(parse_key_seq("O"), "open-line-above");
        normal.bind(parse_key_seq(":"), "enter-command-mode");
        // Window management (Ctrl-W prefix, normal mode only)
        normal.bind(parse_key_seq_spaced("C-w v"), "split-vertical");
        normal.bind(parse_key_seq_spaced("C-w s"), "split-horizontal");
        normal.bind(parse_key_seq_spaced("C-w q"), "close-window");
        normal.bind(parse_key_seq_spaced("C-w h"), "focus-left");
        normal.bind(parse_key_seq_spaced("C-w j"), "focus-down");
        normal.bind(parse_key_seq_spaced("C-w k"), "focus-up");
        normal.bind(parse_key_seq_spaced("C-w l"), "focus-right");

        // Leader key (SPC) bindings — Doom Emacs style
        normal.bind(parse_key_seq_spaced("SPC SPC"), "command-palette");
        // +buffer
        normal.bind(parse_key_seq_spaced("SPC b s"), "save");
        normal.bind(parse_key_seq_spaced("SPC b b"), "switch-buffer");
        normal.bind(parse_key_seq_spaced("SPC b d"), "kill-buffer");
        normal.bind(parse_key_seq_spaced("SPC b n"), "next-buffer");
        normal.bind(parse_key_seq_spaced("SPC b p"), "prev-buffer");
        // +file
        normal.bind(parse_key_seq_spaced("SPC f f"), "find-file");
        normal.bind(parse_key_seq_spaced("SPC f s"), "save");
        // +window
        normal.bind(parse_key_seq_spaced("SPC w v"), "split-vertical");
        normal.bind(parse_key_seq_spaced("SPC w s"), "split-horizontal");
        normal.bind(parse_key_seq_spaced("SPC w q"), "close-window");
        normal.bind(parse_key_seq_spaced("SPC w h"), "focus-left");
        normal.bind(parse_key_seq_spaced("SPC w j"), "focus-down");
        normal.bind(parse_key_seq_spaced("SPC w k"), "focus-up");
        normal.bind(parse_key_seq_spaced("SPC w l"), "focus-right");
        // +ai
        normal.bind(parse_key_seq_spaced("SPC a a"), "ai-prompt");
        normal.bind(parse_key_seq_spaced("SPC a c"), "ai-cancel");
        // +help
        normal.bind(parse_key_seq_spaced("SPC h k"), "describe-key");
        normal.bind(parse_key_seq_spaced("SPC h c"), "describe-command");
        // +theme
        normal.bind(parse_key_seq_spaced("SPC t t"), "cycle-theme");
        normal.bind(parse_key_seq_spaced("SPC t s"), "set-theme");
        // +debug
        normal.bind(parse_key_seq_spaced("SPC d d"), "debug-start");
        normal.bind(parse_key_seq_spaced("SPC d s"), "debug-self");
        normal.bind(parse_key_seq_spaced("SPC d q"), "debug-stop");
        normal.bind(parse_key_seq_spaced("SPC d c"), "debug-continue");
        normal.bind(parse_key_seq_spaced("SPC d n"), "debug-step-over");
        normal.bind(parse_key_seq_spaced("SPC d i"), "debug-step-into");
        normal.bind(parse_key_seq_spaced("SPC d o"), "debug-step-out");
        normal.bind(parse_key_seq_spaced("SPC d b"), "debug-toggle-breakpoint");
        normal.bind(parse_key_seq_spaced("SPC d v"), "debug-inspect");
        // +quit
        normal.bind(parse_key_seq_spaced("SPC q q"), "quit");
        normal.bind(parse_key_seq_spaced("SPC q Q"), "force-quit");
        // +syntax (tree-sitter structural selection)
        normal.bind(parse_key_seq_spaced("SPC s s"), "syntax-select-node");
        normal.bind(parse_key_seq_spaced("SPC s e"), "syntax-expand-selection");
        normal.bind(parse_key_seq_spaced("SPC s c"), "syntax-contract-selection");

        // Group labels for which-key popup
        normal.set_group_name(parse_key_seq_spaced("SPC b"), "+buffer");
        normal.set_group_name(parse_key_seq_spaced("SPC f"), "+file");
        normal.set_group_name(parse_key_seq_spaced("SPC w"), "+window");
        normal.set_group_name(parse_key_seq_spaced("SPC a"), "+ai");
        normal.set_group_name(parse_key_seq_spaced("SPC t"), "+theme");
        normal.set_group_name(parse_key_seq_spaced("SPC d"), "+debug");
        normal.set_group_name(parse_key_seq_spaced("SPC h"), "+help");
        normal.set_group_name(parse_key_seq_spaced("SPC q"), "+quit");
        normal.set_group_name(parse_key_seq_spaced("SPC s"), "+syntax");

        let mut insert = Keymap::new("insert");
        insert.bind(vec![KeyPress::special(Key::Escape)], "enter-normal-mode");
        insert.bind(vec![KeyPress::special(Key::Left)], "move-left");
        insert.bind(vec![KeyPress::special(Key::Down)], "move-down");
        insert.bind(vec![KeyPress::special(Key::Up)], "move-up");
        insert.bind(vec![KeyPress::special(Key::Right)], "move-right");
        // Note: Enter, Backspace, and printable chars are handled specially
        // by the binary, not through the keymap, since they need arguments.

        // Visual mode: v/V enter from normal
        normal.bind(parse_key_seq("v"), "enter-visual-char");
        normal.bind(parse_key_seq("V"), "enter-visual-line");

        // Visual keymap: all normal movements plus operators
        let mut visual = Keymap::new("visual");
        // Movement (same as normal)
        visual.bind(parse_key_seq("h"), "move-left");
        visual.bind(parse_key_seq("j"), "move-down");
        visual.bind(parse_key_seq("k"), "move-up");
        visual.bind(parse_key_seq("l"), "move-right");
        visual.bind(vec![KeyPress::special(Key::Left)], "move-left");
        visual.bind(vec![KeyPress::special(Key::Down)], "move-down");
        visual.bind(vec![KeyPress::special(Key::Up)], "move-up");
        visual.bind(vec![KeyPress::special(Key::Right)], "move-right");
        visual.bind(parse_key_seq("0"), "move-to-line-start");
        visual.bind(parse_key_seq("$"), "move-to-line-end");
        visual.bind(parse_key_seq("G"), "move-to-last-line");
        visual.bind(parse_key_seq("gg"), "move-to-first-line");
        visual.bind(parse_key_seq("w"), "move-word-forward");
        visual.bind(parse_key_seq("b"), "move-word-backward");
        visual.bind(parse_key_seq("e"), "move-word-end");
        visual.bind(parse_key_seq("W"), "move-big-word-forward");
        visual.bind(parse_key_seq("B"), "move-big-word-backward");
        visual.bind(parse_key_seq("E"), "move-big-word-end");
        visual.bind(parse_key_seq("%"), "move-matching-bracket");
        visual.bind(parse_key_seq("{"), "move-paragraph-backward");
        visual.bind(parse_key_seq("}"), "move-paragraph-forward");
        visual.bind(parse_key_seq("f"), "find-char-forward-await");
        visual.bind(parse_key_seq("F"), "find-char-backward-await");
        visual.bind(parse_key_seq("t"), "till-char-forward-await");
        visual.bind(parse_key_seq("T"), "till-char-backward-await");
        // Marks
        visual.bind(parse_key_seq("m"), "set-mark-await");
        visual.bind(parse_key_seq("'"), "jump-mark-await");
        // Scroll
        visual.bind(parse_key_seq("C-u"), "scroll-half-up");
        visual.bind(parse_key_seq("C-d"), "scroll-half-down");
        visual.bind(parse_key_seq("C-f"), "scroll-page-down");
        visual.bind(parse_key_seq("C-b"), "scroll-page-up");
        visual.bind(parse_key_seq("zz"), "scroll-center");
        visual.bind(parse_key_seq("zt"), "scroll-top");
        visual.bind(parse_key_seq("zb"), "scroll-bottom");
        // Screen-relative cursor
        visual.bind(parse_key_seq("H"), "move-screen-top");
        visual.bind(parse_key_seq("M"), "move-screen-middle");
        visual.bind(parse_key_seq("L"), "move-screen-bottom");
        // Operators
        visual.bind(parse_key_seq("d"), "visual-delete");
        visual.bind(parse_key_seq("x"), "visual-delete");
        visual.bind(parse_key_seq("y"), "visual-yank");
        visual.bind(parse_key_seq("c"), "visual-change");
        // Text objects in visual mode
        visual.bind(parse_key_seq("i"), "visual-inner-object");
        visual.bind(parse_key_seq("a"), "visual-around-object");
        // Mode switches
        visual.bind(parse_key_seq("v"), "enter-visual-char");
        visual.bind(parse_key_seq("V"), "enter-visual-line");
        visual.bind(vec![KeyPress::special(Key::Escape)], "enter-normal-mode");
        // Tree-sitter structural expansion (Phase 4b M3)
        visual.bind(parse_key_seq_spaced("SPC s e"), "syntax-expand-selection");
        visual.bind(parse_key_seq_spaced("SPC s c"), "syntax-contract-selection");

        maps.insert("normal".to_string(), normal);
        maps.insert("insert".to_string(), insert);
        maps.insert("visual".to_string(), visual);
        maps.insert("command".to_string(), Keymap::new("command"));

        maps
    }
}
